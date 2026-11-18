//! Read-only counters. Cookies include HttpOnly and memory contexts; no values leave CEF.
use cef::*;
use serde::Serialize;
use std::{collections::HashMap,sync::{Arc,Mutex,OnceLock,atomic::{AtomicBool,AtomicU64,Ordering}},sync::mpsc::Sender};
use crate::cef_vkladki::Sobytie;
#[derive(Clone,Serialize,serde::Deserialize)]
pub struct Obem{pub cookies:Option<u64>,pub cache_bytes:Option<u64>,pub cache_memory:bool}
type Pending=HashMap<(String,String),tokio::sync::oneshot::Sender<Obem>>;
static PENDING:OnceLock<Mutex<Pending>>=OnceLock::new();
struct Ozhidanie((String,String));
impl Drop for Ozhidanie{fn drop(&mut self){if let Ok(mut p)=PENDING.get_or_init(Default::default).lock(){p.remove(&self.0);}}}
pub fn otvet(id:&str,data:&serde_json::Value)->bool{
 if data["tip"]!="dannye-obem"{return false;}
 let key=(id.into(),data["token"].as_str().unwrap_or("").into());
 if let Some(tx)=PENDING.get_or_init(Default::default).lock().unwrap().remove(&key){
  if let Ok(v)=serde_json::from_value(data.clone()){let _=tx.send(v);}
 }true
}
#[tauri::command]
pub async fn dannye_saytov_statistika(window:tauri::WebviewWindow,s:tauri::State<'_,crate::Sostoyanie>,id:String)->Result<Obem,String>{
 let channel=crate::okna_app::kanal(&s,window.label(),&id)?;
 let token=format!("stats-{}",std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_err(|e|e.to_string())?.as_nanos());
 let (tx,rx)=tokio::sync::oneshot::channel();let key=(id.clone(),token.clone());
 PENDING.get_or_init(Default::default).lock().unwrap().insert(key.clone(),tx);let _guard=Ozhidanie(key);
 let cmd=crate::sessiya::Komanda::DannyeStatistika{token};
 let cmd=if window.label()=="main"{cmd}else{crate::sessiya::Komanda::OknoKomanda{label:window.label().into(),komanda:Box::new(cmd)}};
 channel.try_send(cmd).map_err(|_|"Движок недоступен".to_string())?;
 tokio::time::timeout(std::time::Duration::from_secs(15),rx).await.map_err(|_|"Не удалось получить размер данных сайтов".to_string())?.map_err(|_|"Движок завершился до подсчёта данных".to_string())
}
struct Job{remaining:usize,cookies:Option<u64>,bytes:Option<u64>,memory:bool,token:String,label:String,tx:Sender<Sobytie>}
impl Job{
 fn done(&mut self,cookies:Option<Option<u64>>,cache:Option<(Option<u64>,bool)>){
  if let Some(n)=cookies{self.cookies=self.cookies.zip(n).map(|(a,b)|a+b);}
  if let Some((n,memory))=cache{self.bytes=self.bytes.zip(n).map(|(a,b)|a+b);self.memory|=memory;}
  self.remaining=self.remaining.saturating_sub(1);
  if self.remaining==0{let _=self.tx.send(Sobytie::Sluzhebnoe(serde_json::json!({"tip":"dannye-obem","token":self.token,"label":self.label,"cookies":self.cookies,"cache_bytes":self.bytes,"cache_memory":self.memory})));}
 }
}
struct CookieCount{job:Arc<Mutex<Job>>,total:AtomicU64,failed:AtomicBool}
impl Drop for CookieCount{fn drop(&mut self){self.job.lock().unwrap().done(Some((!self.failed.load(Ordering::Relaxed)).then(||self.total.load(Ordering::Relaxed))),None);}}
wrap_cookie_visitor!{
 struct CountVisitor{state:Arc<CookieCount>}
 impl CookieVisitor{
  fn visit(&self,_cookie:Option<&Cookie>,_count:i32,total:i32,_delete_cookie:Option<&mut i32>)->i32{
   self.state.total.store(total.max(0) as u64,Ordering::Relaxed);0
  }
 }
}
fn disk_bytes(root:&std::path::Path)->Result<u64,()>{
 let mut pending=vec![root.to_path_buf()];let mut bytes=0u64;let mut files=0usize;let deadline=std::time::Instant::now()+std::time::Duration::from_secs(4);
 while let Some(path)=pending.pop(){
  if std::time::Instant::now()>deadline||files>200000{return Err(());}
  let entries=match std::fs::read_dir(path){Ok(v)=>v,Err(e) if e.kind()==std::io::ErrorKind::NotFound=>continue,Err(_)=>return Err(())};
  for entry in entries{if files>200000||std::time::Instant::now()>deadline{return Err(());}let entry=entry.map_err(|_|())?;let kind=entry.file_type().map_err(|_|())?;files+=1;
   if kind.is_dir(){pending.push(entry.path());}else if kind.is_file(){match entry.metadata(){Ok(m)=>bytes=bytes.saturating_add(m.len()),Err(e) if e.kind()==std::io::ErrorKind::NotFound=>{},Err(_)=>return Err(())}}
  }
 }Ok(bytes)
}
pub fn izmerit(contexts:Vec<RequestContext>,token:String,label:String,tx:Sender<Sobytie>){
 let mut unique:Vec<RequestContext>=vec![];
 for c in contexts{if !unique.iter_mut().any(|old|c.is_same(Some(old))!=0){unique.push(c);}}
 let count=unique.len();let job=Arc::new(Mutex::new(Job{remaining:(count*2).max(1),cookies:Some(0),bytes:Some(0),memory:false,token,label,tx}));
 if count==0{job.lock().unwrap().done(Some(None),Some((None,false)));return;}
 for context in unique{
  let state=Arc::new(CookieCount{job:job.clone(),total:AtomicU64::new(0),failed:AtomicBool::new(false)});
  if let Some(manager)=context.cookie_manager(None){
   let mut visitor=CountVisitor::new(state.clone());
   if manager.visit_all_cookies(Some(&mut visitor))==0{state.failed.store(true,Ordering::Relaxed);}
  }else{state.failed.store(true,Ordering::Relaxed);}
  // Visitor destruction is completion even for an empty store (no visit callbacks).
  drop(state);
  let path=CefString::from(&context.cache_path()).to_string();let job=job.clone();
  if path.is_empty(){job.lock().unwrap().done(None,Some((Some(0),true)));}
  else{std::thread::spawn(move||{let bytes=disk_bytes(&std::path::Path::new(&path).join("Cache")).ok();job.lock().unwrap().done(None,Some((bytes,false)));});}
 }
}
