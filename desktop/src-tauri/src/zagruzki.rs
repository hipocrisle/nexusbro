//! Менеджер загрузок: CEF сохраняет файл, оболочка управляет прогрессом.
use cef::*;
use crate::{cef_vkladki::Sobytie,sessiya::Komanda};
use serde::{Serialize,Deserialize};
use serde_json::{json,Value};
use std::{collections::HashMap,sync::{Mutex,OnceLock,mpsc::Sender},cell::RefCell};
thread_local!{static CALLBACKS:RefCell<HashMap<u32,(DownloadItemCallback,bool)>>=RefCell::new(HashMap::new());}
wrap_download_handler!{
    struct Zagruzka{id:u64,tx:Sender<Sobytie>,privatnaya:bool}
    impl DownloadHandler{
        fn can_download(&self,_browser:Option<&mut Browser>,_url:Option<&CefString>,_request_method:Option<&CefString>)->i32{1}
        fn on_before_download(&self,_browser:Option<&mut Browser>,_download_item:Option<&mut DownloadItem>,suggested_name:Option<&CefString>,callback:Option<&mut BeforeDownloadCallback>)->i32{
            if let Some(c)=callback{
                let name=suggested_name.map(|s|s.to_string()).unwrap_or_else(||"download".into());
                let name=std::path::Path::new(&name).file_name().unwrap_or_else(||std::ffi::OsStr::new("download"));
                let path=dirs::download_dir().unwrap_or_else(std::env::temp_dir).join(name);
                let test_dir=std::env::var_os("NEXUSBRO_TEST_DATA").and_then(|_|std::env::var_os("NEXUSBRO_TEST_DOWNLOAD_DIR"));
                let (path,dialog)=if let Some(dir)=test_dir{let dir=std::path::PathBuf::from(dir);let _=std::fs::create_dir_all(&dir);(dir.join(name),0)}else{(path,1)};
                c.cont(Some(&CefString::from(path.to_string_lossy().as_ref())),dialog);return 1;
            }0
        }
        fn on_download_updated(&self,_browser:Option<&mut Browser>,download_item:Option<&mut DownloadItem>,callback:Option<&mut DownloadItemCallback>){
            if let Some(d)=download_item{
                let id=d.id();
                let status=if d.is_complete()!=0{"Готово"}else if d.is_canceled()!=0{"Отменена"}else if d.is_in_progress()!=0{if d.is_paused()!=0{"Приостановлена"}else{"Загружается"}}else{"Ошибка"};
                CALLBACKS.with(|m|{let mut m=m.borrow_mut();if d.is_in_progress()!=0{if let Some(c)=callback{m.insert(id,(c.clone(),self.privatnaya));}}else{m.remove(&id);}});
                let data=json!({"tip":"download","tab_id":self.id,"download_id":id,"engine_pid":std::process::id(),"name":CefString::from(&d.suggested_file_name()).to_string(),"url":CefString::from(&d.url()).to_string(),"path":CefString::from(&d.full_path()).to_string(),"percent":d.percent_complete(),"received":d.received_bytes(),"total":d.total_bytes(),"speed":d.current_speed(),"status":status,"privatnaya":self.privatnaya});
                let _=self.tx.send(Sobytie::Sluzhebnoe(data));
            }
        }
    }
}
pub fn handler(id:u64,tx:Sender<Sobytie>,privatnaya:bool)->DownloadHandler{Zagruzka::new(id,tx,privatnaya)}
pub fn upravlyat(id:u32,action:&str){let callback=CALLBACKS.with(|m|m.borrow().get(&id).map(|(c,_)|c.clone()));if let Some(c)=callback{match action{"cancel"=>c.cancel(),"pause"=>c.pause(),"resume"=>c.resume(),_=>{}}}}
pub fn zakryt_privatnye(){let callbacks=CALLBACKS.with(|m|m.borrow().values().filter(|(_,p)|*p).map(|(c,_)|c.clone()).collect::<Vec<_>>());for c in callbacks{c.cancel();}}
#[derive(Clone,Serialize,Deserialize)]
pub struct Zapis{pub id:String,pub super_id:String,pub download_id:u32,pub engine_pid:u32,pub tab_id:u64,pub name:String,pub url:String,pub path:String,pub percent:i32,pub received:i64,pub total:i64,pub speed:i64,pub status:String,pub privatnaya:bool}
static SPISOK:OnceLock<Mutex<HashMap<String,Zapis>>>=OnceLock::new();
fn put()->std::path::PathBuf{crate::brauzer::papka_profilej().parent().unwrap().join("zagruzki.json")}
fn spisok()->&'static Mutex<HashMap<String,Zapis>>{SPISOK.get_or_init(||{
    let mut items:Vec<Zapis>=std::fs::read(put()).ok().and_then(|b|serde_json::from_slice(&b).ok()).unwrap_or_default();
    for i in &mut items{if ["Загружается","Приостановлена"].contains(&i.status.as_str()){i.status="Прервана".into();}}
    Mutex::new(items.into_iter().filter(|i|!i.privatnaya).map(|i|(i.id.clone(),i)).collect())
})}
fn zapisat(m:&HashMap<String,Zapis>){
    let items:Vec<_>=m.values().filter(|i|!i.privatnaya).collect();let path=put();if let Some(p)=path.parent(){let _=std::fs::create_dir_all(p);}let tmp=path.with_extension("tmp");
    if let Ok(b)=serde_json::to_vec(&items){if std::fs::write(&tmp,b).is_ok(){let _=std::fs::rename(tmp,path);}}
}
pub fn obnovit(super_id:&str,data:&Value){
    let mut data=data.clone();let Some(dl)=data["download_id"].as_u64()else{return};let Some(pid)=data["engine_pid"].as_u64()else{return};
    data["super_id"]=json!(super_id);data["id"]=json!(format!("{super_id}:{pid}:{dl}"));
    if let Ok(item)=serde_json::from_value::<Zapis>(data){let mut m=spisok().lock().unwrap();let persist=!m.contains_key(&item.id)||!["Загружается","Приостановлена"].contains(&item.status.as_str());m.insert(item.id.clone(),item);if persist{zapisat(&m);}}
}
pub fn ochistit_privatnye(super_id:&str){let mut m=spisok().lock().unwrap();m.retain(|_,i|!(i.privatnaya&&i.super_id==super_id));}
#[tauri::command]
pub async fn zagruzki_spisok()->Vec<Zapis>{let mut items:Vec<_>=spisok().lock().unwrap().values().cloned().collect();items.sort_by(|a,b|b.id.cmp(&a.id));items}
#[tauri::command]
pub async fn zagruzka_deystvie(app:tauri::AppHandle,s:tauri::State<'_,crate::Sostoyanie>,super_id:String,id:String,action:String)->Result<(),String>{
    let item=spisok().lock().unwrap().get(&id).filter(|i|i.super_id==super_id).cloned().ok_or("Загрузка не найдена")?;
    if action=="folder"{
        let path=std::path::PathBuf::from(&item.path);let parent=path.parent().filter(|p|p.exists()).ok_or("Папка не найдена")?;
        #[cfg(target_os="windows")]std::process::Command::new("explorer.exe").arg(parent).spawn().map_err(|e|e.to_string())?;
        #[cfg(target_os="linux")]std::process::Command::new("xdg-open").arg(parent).spawn().map_err(|e|e.to_string())?;
        return Ok(());
    }
    if action=="retry"{
        let result=crate::zapustit_vnutri(&app,&s,&super_id).await;if !result.ok{return Err(result.soobshchenie);}
        let channel=s.vkladki.lock().unwrap().get(&super_id).and_then(|v|v.kanal.clone()).ok_or("Супер-вкладка закрыта")?;
        return channel.try_send(Komanda::Skachat{adres:item.url,privatnaya:item.privatnaya}).map_err(|_|"Движок занят".into());
    }
    if !["cancel","pause","resume"].contains(&action.as_str()){return Err("Неизвестное действие".into());}
    let channel=s.vkladki.lock().unwrap().get(&super_id).filter(|v|v.proc.as_ref().is_some_and(|p|p.id()==item.engine_pid)).and_then(|v|v.kanal.clone()).ok_or("Процесс загрузки уже завершён")?;
    channel.try_send(Komanda::ZagruzkaUpravlenie{id:item.download_id,action}).map_err(|_|"Движок занят".into())
}
