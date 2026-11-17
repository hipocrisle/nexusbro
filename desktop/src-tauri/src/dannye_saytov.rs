//! Очистка текущего профиля подтверждается CEF callbacks, а не отправкой команды.
use cef::*;
use serde::Serialize;
use std::{collections::HashMap,sync::{Arc,Mutex,OnceLock},sync::mpsc::Sender};
use crate::cef_vkladki::Sobytie;

#[derive(Clone,Serialize)]
pub struct Otchet { pub cookies:bool,pub cache:bool }
type Pending=HashMap<(String,String),tokio::sync::oneshot::Sender<Result<Otchet,String>>>;
static PENDING:OnceLock<Mutex<Pending>>=OnceLock::new();

pub fn otvet(super_id:&str,data:&serde_json::Value)->bool{
 if data["tip"]!="ochistka-zavershena"{return false;}
 let token=data["token"].as_str().unwrap_or("").to_string();
 if let Some(sender)=PENDING.get_or_init(Default::default).lock().unwrap().remove(&(super_id.into(),token)){
  let result=if let Some(error)=data["error"].as_str(){Err(error.to_string())}
   else{Ok(Otchet{cookies:data["cookies"]==true,cache:data["cache"]==true})};
  let _=sender.send(result);
 }
 true
}

#[tauri::command]
pub async fn ochistit_dannye_saytov(window:tauri::WebviewWindow,s:tauri::State<'_,crate::Sostoyanie>,id:String,cookies:bool,cache:bool)->Result<Otchet,String>{
 if !cookies&&!cache{return Ok(Otchet{cookies:false,cache:false});}
 let kanal=crate::okna_app::kanal(&s,window.label(),&id)?;
 let token=format!("clear-{}-{}",std::process::id(),std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_err(|e|e.to_string())?.as_nanos());
 let (send,recv)=tokio::sync::oneshot::channel();
 let key=(id.clone(),token.clone());
 PENDING.get_or_init(Default::default).lock().unwrap().insert(key.clone(),send);
 let cmd=crate::sessiya::Komanda::OchistitDannye{token,cookies,cache};
 let cmd=if window.label()=="main"{cmd}else{crate::sessiya::Komanda::OknoKomanda{label:window.label().into(),komanda:Box::new(cmd)}};
 if kanal.try_send(cmd).is_err(){PENDING.get().unwrap().lock().unwrap().remove(&key);return Err("Движок недоступен. Данные сайтов не очищены.".into());}
 let result=tokio::time::timeout(std::time::Duration::from_secs(30),recv).await;
 PENDING.get().unwrap().lock().unwrap().remove(&key);
 match result {Ok(Ok(result))=>result,_=>Err("Не удалось подтвердить завершение очистки. Часть данных уже могла быть удалена.".into())}
}

struct Rabota {ostalos:usize,error:Option<String>,token:String,label:String,cookies:bool,cache:bool,tx:Sender<Sobytie>}
impl Rabota{
 fn got(&mut self,error:Option<&str>){
  if let Some(error)=error{self.error=Some(error.into());}
  self.ostalos=self.ostalos.saturating_sub(1);
  if self.ostalos==0{let _=self.tx.send(Sobytie::Sluzhebnoe(serde_json::json!({"tip":"ochistka-zavershena","label":self.label,"token":self.token,"cookies":self.cookies,"cache":self.cache,"error":self.error})));}
 }
}
wrap_completion_callback!{
 struct KeshGotov{job:Arc<Mutex<Rabota>>}
 impl CompletionCallback{fn on_complete(&self){self.job.lock().unwrap().got(None);}}
}
wrap_delete_cookies_callback!{
 struct CookiesGotovy{job:Arc<Mutex<Rabota>>}
 impl DeleteCookiesCallback{fn on_complete(&self,num_deleted:i32){self.job.lock().unwrap().got(if num_deleted<0{Some("Не удалось удалить cookies. Остальные выбранные данные могли быть удалены.")}else{None});}}
}

pub fn ochistit(mut contexts:Vec<RequestContext>,token:String,label:String,cookies:bool,cache:bool,tx:Sender<Sobytie>){
 // Вектор поступает с UI-потока CEF; normal и shared private контексты текущей Super.
 let mut unique:Vec<RequestContext>=Vec::new();
 for c in contexts.drain(..){if !unique.iter_mut().any(|old|c.is_same(Some(old))!=0){unique.push(c);}}
 let count=unique.len()*(cookies as usize+cache as usize);
 let job=Arc::new(Mutex::new(Rabota{ostalos:count.max(1),error:None,token,label,cookies,cache,tx}));
 if count==0{job.lock().unwrap().got(Some("Контекст страницы недоступен; очистка не выполнена."));return;}
 for context in unique{
  if cookies{
   if let Some(manager)=context.cookie_manager(None){
    let mut cb=CookiesGotovy::new(job.clone());
    if manager.delete_cookies(None,None,Some(&mut cb))==0{job.lock().unwrap().got(Some("CEF не принял удаление cookies."));}
   }else{job.lock().unwrap().got(Some("Хранилище cookies недоступно."));}
  }
  if cache{context.clear_http_cache(Some(&mut KeshGotov::new(job.clone())));}
 }
}
