//! Мастер переноса закладок. Источник только читается; пароли и cookies не импортируются.
use std::{path::{Path,PathBuf},sync::{Arc,Mutex,OnceLock,atomic::{AtomicBool,AtomicU64,Ordering}},collections::HashMap};
use serde::Serialize;
use crate::biblioteka::Zapis;
#[derive(Clone,Serialize)]pub struct Istochnik{pub id:String,pub browser:String,pub profile:String,#[serde(skip)]path:PathBuf,#[serde(skip)]firefox:bool}
fn sources()->Vec<Istochnik>{
 let mut roots:Vec<(String,PathBuf,bool)>=Vec::new();
 let fixture=std::env::var_os("NEXUSBRO_TEST_DATA").and_then(|_|std::env::var_os("NEXUSBRO_TEST_IMPORT_ROOT"));
 if let Some(root)=fixture{let r=PathBuf::from(root);for name in ["Chrome","Edge","Brave","Chromium"]{roots.push((name.into(),r.join(name).join("User Data"),false));}roots.push(("Firefox".into(),r.join("Firefox/Profiles"),true));}
 else{
  #[cfg(target_os="windows")]{let local=dirs::data_local_dir().unwrap_or_default();for(name,path)in [("Chrome","Google/Chrome/User Data"),("Edge","Microsoft/Edge/User Data"),("Brave","BraveSoftware/Brave-Browser/User Data"),("Chromium","Chromium/User Data"),("Яндекс Браузер","Yandex/YandexBrowser/User Data")]{roots.push((name.into(),local.join(path),false));}if let Some(roaming)=dirs::config_dir(){roots.push(("Firefox".into(),roaming.join("Mozilla/Firefox/Profiles"),true));}}
  #[cfg(not(target_os="windows"))]{let config=dirs::config_dir().unwrap_or_default();for(name,path)in [("Chrome","google-chrome"),("Edge","microsoft-edge"),("Brave","BraveSoftware/Brave-Browser"),("Chromium","chromium")]{roots.push((name.into(),config.join(path),false));}if let Some(home)=dirs::home_dir(){roots.push(("Firefox".into(),home.join(".mozilla/firefox"),true));}}
 }
 let mut result=Vec::new();
 for(browser,root,firefox)in roots{if let Ok(entries)=std::fs::read_dir(root){for entry in entries.flatten(){let dir=entry.path();let path=dir.join(if firefox{"places.sqlite"}else{"Bookmarks"});if !path.is_file(){continue;}use std::hash::{Hash,Hasher};let mut hash=std::collections::hash_map::DefaultHasher::new();path.hash(&mut hash);result.push(Istochnik{id:format!("{:x}",hash.finish()),browser:browser.clone(),profile:entry.file_name().to_string_lossy().into_owned(),path,firefox});}}}
 result.sort_by(|a,b|(&a.browser,&a.profile).cmp(&(&b.browser,&b.profile)));result
}
#[derive(Clone,Serialize)]pub struct Progress{pub id:u64,pub status:String,pub read:usize,pub imported:usize,pub skipped:usize,pub error:Option<String>}
struct Task{progress:Progress,cancel:Arc<AtomicBool>}
static TASKS:OnceLock<Mutex<HashMap<u64,Task>>>=OnceLock::new();
fn tasks()->&'static Mutex<HashMap<u64,Task>>{TASKS.get_or_init(Default::default)}
fn progress(id:u64,count:usize){if let Some(t)=tasks().lock().unwrap().get_mut(&id){t.progress.read=count;}}
#[tauri::command]pub fn import_istochniki()->Vec<Istochnik>{sources()}
#[tauri::command]pub fn import_status(id:u64)->Result<Progress,String>{{let m=tasks().lock().unwrap();let id=if id==0{m.keys().copied().max().unwrap_or(0)}else{id};m.get(&id).map(|t|t.progress.clone()).ok_or("Задача не найдена".into())}}
#[tauri::command]pub fn import_otmenit(id:u64){if let Some(t)=tasks().lock().unwrap().get(&id){t.cancel.store(true,Ordering::Relaxed);}}
#[tauri::command]pub fn import_nachat(app:tauri::AppHandle,source:String)->Result<u64,String>{
 let source=sources().into_iter().find(|s|s.id==source).ok_or("Профиль браузера больше недоступен")?;
 static NEXT:AtomicU64=AtomicU64::new(1);let id=NEXT.fetch_add(1,Ordering::Relaxed);let cancel=Arc::new(AtomicBool::new(false));
 {let mut m=tasks().lock().unwrap();if m.values().any(|t|t.progress.status=="Выполняется"){return Err("Дождитесь завершения предыдущего переноса".into());}if m.len()>20{m.clear();}m.insert(id,Task{progress:Progress{id,status:"Выполняется".into(),read:0,imported:0,skipped:0,error:None},cancel:cancel.clone()});}
 std::thread::spawn(move||{
  let result=read(&source,&cancel,id).and_then(|rows|{if cancel.load(Ordering::Relaxed){return Err("Перенос отменён".into());}let total=rows.len();crate::biblioteka::import_zapisi(rows).map(|n|(n,total.saturating_sub(n)))});
  {let mut m=tasks().lock().unwrap();if let Some(t)=m.get_mut(&id){match result{Ok((n,skipped))=>{t.progress.imported=n;t.progress.skipped=skipped;t.progress.status="Готово".into();},Err(e)=>{t.progress.status=if cancel.load(Ordering::Relaxed){"Отменено"}else{"Ошибка"}.into();t.progress.error=Some(e);}}}}
  use tauri::Emitter;let _=app.emit("zakladki-izmenilis",());
 });Ok(id)
}
fn read(source:&Istochnik,cancel:&AtomicBool,id:u64)->Result<Vec<Zapis>,String>{
 let mut out=Vec::new();
 if source.firefox{firefox(&source.path,cancel,id,&mut out)?;}else{
  let meta=std::fs::metadata(&source.path).map_err(|_|"Не удалось прочитать профиль браузера")?;if meta.len()>32*1024*1024{return Err("Файл закладок превышает 32 МБ".into());}
  let data=std::fs::read(&source.path).map_err(|_|"Нет доступа к закладкам выбранного браузера")?;let value:serde_json::Value=serde_json::from_slice(&data).map_err(|_|"Некорректный файл закладок браузера")?;
  let roots=value["roots"].as_object().ok_or("В профиле отсутствует список закладок")?;for(key,root)in roots{if key=="bookmark_bar"{if let Some(children)=root["children"].as_array(){for child in children{chrome(child,"",0,cancel,id,&mut out)?;}}}else{chrome(root,"",0,cancel,id,&mut out)?;}}
 }
 Ok(out)
}
fn add(out:&mut Vec<Zapis>,url:&str,name:&str,folder:&str)->Result<(),String>{if out.len()>=20000{return Err("В профиле больше 20000 закладок: перенос остановлен без изменений".into());}out.push(Zapis{id:String::new(),adres:url.into(),nazvanie:name.into(),papka:folder.into(),vremya:crate::biblioteka::vremya()});Ok(())}
fn chrome(v:&serde_json::Value,parent:&str,depth:usize,cancel:&AtomicBool,id:u64,out:&mut Vec<Zapis>)->Result<(),String>{
 if cancel.load(Ordering::Relaxed){return Err("Перенос отменён".into());}if depth>64{return Err("Слишком глубокая структура папок".into());}
 let name=v["name"].as_str().unwrap_or("");if v["type"]=="url"{add(out,v["url"].as_str().unwrap_or(""),name,parent)?;progress(id,out.len());}
 if let Some(children)=v["children"].as_array(){let folder=if parent.is_empty(){name.into()}else{format!("{parent}/{name}")};for child in children{chrome(child,&folder,depth+1,cancel,id,out)?;}}Ok(())
}
fn firefox(path:&Path,cancel:&AtomicBool,id:u64,out:&mut Vec<Zapis>)->Result<(),String>{
 use rusqlite::{Connection,OpenFlags};
 let connection=Connection::open_with_flags(path,OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(|_|"Не удалось открыть закладки Firefox")?;connection.busy_timeout(std::time::Duration::from_secs(3)).map_err(|_|"Не удалось настроить чтение Firefox")?;
 // Одна read-транзакция видит согласованный снимок, включая WAL открытого Firefox.
 connection.execute_batch("BEGIN").map_err(|_|"Firefox занят. Повторите перенос позже")?;
 let mut folders=HashMap::<i64,(i64,String,String)>::new();
 {let mut q=connection.prepare("SELECT id,parent,COALESCE(title,''),guid FROM moz_bookmarks WHERE type=2").map_err(|_|"Неизвестный формат закладок Firefox")?;let rows=q.query_map([],|r|Ok((r.get::<_,i64>(0)?,(r.get(1)?,r.get(2)?,r.get(3)?)))).map_err(|_|"Не удалось прочитать папки Firefox")?;for row in rows{let(k,v)=row.map_err(|_|"Некорректная папка Firefox")?;folders.insert(k,v);}}
 let mut q=connection.prepare("SELECT b.parent,COALESCE(b.title,''),p.url FROM moz_bookmarks b JOIN moz_places p ON p.id=b.fk WHERE b.type=1 ORDER BY b.position").map_err(|_|"Не удалось прочитать закладки Firefox")?;
 let rows=q.query_map([],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?))).map_err(|_|"Не удалось прочитать записи Firefox")?;
 for row in rows{if cancel.load(Ordering::Relaxed){return Err("Перенос отменён".into());}let(mut parent,name,url)=row.map_err(|_|"Некорректная запись Firefox")?;let mut parts=Vec::new();let mut tags=false;for _ in 0..64{let Some((next,title,guid))=folders.get(&parent)else{break};if guid=="tags________"{tags=true;break;}if !title.is_empty(){parts.push(title.clone());}if *next==parent{break;}parent=*next;}if tags{continue;}parts.reverse();add(out,&url,&name,&parts.join("/"))?;progress(id,out.len());}
 Ok(())
}
