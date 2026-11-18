//! История и закладки. Автоматическая запись приватных страниц запрещена.
use serde::{Serialize,Deserialize};
use tauri::Emitter;
use std::sync::{Mutex,OnceLock};
use std::collections::HashMap;
use std::path::PathBuf;
static IO:Mutex<()>=Mutex::new(());
static SEEN:OnceLock<Mutex<HashMap<(String,u64),String>>>=OnceLock::new();
#[derive(Clone,Serialize,Deserialize)]
pub struct Zapis {#[serde(default,skip_serializing_if="Option::is_none")]pub super_id:Option<String>,pub id:String,pub adres:String,pub nazvanie:String,pub vremya:u64,#[serde(default)]pub papka:String}
fn fajl(name:&str)->PathBuf{crate::brauzer::papka_profilej().parent().unwrap().join(name)}
fn chitat(name:&str)->Vec<Zapis>{std::fs::read(fajl(name)).ok().and_then(|s|serde_json::from_slice(&s).ok()).unwrap_or_default()}
fn zapisat(name:&str,items:&[Zapis])->Result<(),String>{
    let p=fajl(name);if let Some(d)=p.parent(){std::fs::create_dir_all(d).map_err(|e|e.to_string())?;}
    let tmp=p.with_extension("tmp");std::fs::write(&tmp,serde_json::to_vec(items).map_err(|e|e.to_string())?).map_err(|e|e.to_string())?;
    std::fs::rename(tmp,p).map_err(|e|e.to_string())
}
pub fn vremya()->u64{std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs()}
fn network(a:&str)->bool{reqwest::Url::parse(a).ok().is_some_and(|u|matches!(u.scheme(),"http"|"https"))}
pub fn poseshchenie(super_id:&str,tab:&crate::sessiya::Obychnaya){
    if crate::privatnost::eto_id(super_id)||tab.privatnaya||tab.otlozhena||tab.zagruzka||!network(&tab.adres){return;}
    let mut seen=SEEN.get_or_init(Default::default).lock().unwrap();let key=(super_id.into(),tab.id);
    if seen.get(&key)==Some(&tab.adres){return;}seen.insert(key,tab.adres.clone());drop(seen);
    let _lock=IO.lock().unwrap();let mut items=chitat("istoriya.json");items.retain(|i|i.adres!=tab.adres||i.super_id.as_deref()!=Some(super_id));
    items.insert(0,Zapis{super_id:Some(super_id.into()),id:format!("{}-{}",vremya(),tab.id),adres:tab.adres.clone(),nazvanie:tab.zagolovok.clone(),vremya:vremya(),papka:String::new()});items.truncate(5000);
    let _=zapisat("istoriya.json",&items);
}
#[tauri::command]
pub async fn biblioteka_spisok(razdel:String)->Result<Vec<Zapis>,String>{
    let _lock=IO.lock().unwrap();match razdel.as_str(){"istoriya"=>Ok(chitat("istoriya.json")),"zakladki"=>Ok(chitat("zakladki.json")),_=>Err("Неизвестный раздел".into())}
}
#[tauri::command]
pub async fn biblioteka_udalit(app:tauri::AppHandle,razdel:String,id:Option<String>)->Result<(),String>{
    let name=match razdel.as_str(){"istoriya"=>"istoriya.json","zakladki"=>"zakladki.json",_=>return Err("Неизвестный раздел".into())};
    let _lock=IO.lock().unwrap();let mut items=chitat(name);
    if razdel=="zakladki"{for item in items.iter().filter(|i|id.as_ref().is_none_or(|id|&i.id==id)){crate::browser_sync::record_delete("bookmark",&item.id)?;}}
    match id{Some(id)=>items.retain(|i|i.id!=id),None=>items.clear()};zapisat(name,&items)?;if razdel=="zakladki"{let _=app.emit("zakladki-izmenilis",());}Ok(())
}
#[tauri::command]
pub async fn zakladki_sohranit(app:tauri::AppHandle,adres:String,nazvanie:String,papka:Option<String>)->Result<(),String>{
    if !network(&adres){return Err("В закладки можно добавить HTTP(S)-страницу".into());}
    let _lock=IO.lock().unwrap();let mut items=chitat("zakladki.json");
    if let Some(i)=items.iter_mut().find(|i|i.adres==adres){i.nazvanie=nazvanie;i.papka=papka.unwrap_or_default();}
    else{items.push(Zapis{super_id:None,id:format!("b-{}-{}",vremya(),items.len()),adres,nazvanie,vremya:vremya(),papka:papka.unwrap_or_default()});}
    zapisat("zakladki.json",&items)?;let _=app.emit("zakladki-izmenilis",());Ok(())
}
#[tauri::command]
pub async fn zakladki_import(app:tauri::AppHandle,zapisi:Vec<Zapis>)->Result<usize,String>{let n=import_zapisi(zapisi)?;let _=app.emit("zakladki-izmenilis",());Ok(n)}
pub fn import_zapisi(zapisi:Vec<Zapis>)->Result<usize,String>{
    if zapisi.len()>20000{return Err("Слишком большой файл закладок".into());}
    let _lock=IO.lock().unwrap();let mut items=chitat("zakladki.json");let mut count=0;
    for mut i in zapisi{if network(&i.adres)&&!items.iter().any(|old|old.adres==i.adres){i.id=format!("b-{}-{}",vremya(),items.len());items.push(i);count+=1;}}
    zapisat("zakladki.json",&items)?;Ok(count)
}

#[tauri::command]
pub async fn zakladki_pereklyuchit(app:tauri::AppHandle,adres:String,nazvanie:String)->Result<bool,String>{
 use tauri::Emitter;
 if !network(&adres){return Err("В закладки можно добавить HTTP(S)-страницу".into());}
 let added={let _lock=IO.lock().map_err(|e|e.to_string())?;let mut items=chitat("zakladki.json");
 let exists=items.iter().any(|i|i.adres==adres);
 if exists{for item in items.iter().filter(|i|i.adres==adres){crate::browser_sync::record_delete("bookmark",&item.id)?;}items.retain(|i|i.adres!=adres);}else{items.push(Zapis{super_id:None,id:format!("b-{}-{}",vremya(),items.len()),adres,nazvanie,vremya:vremya(),papka:String::new()});}
 zapisat("zakladki.json",&items)?;!exists};
 let _=app.emit("zakladki-izmenilis",());Ok(added)
}

#[derive(Clone,Serialize)]
pub struct Podskazka{pub adres:String,pub nazvanie:String,pub istochnik:String,pub vremya:u64}
fn adres_bez_shemy(value:&str)->&str{
 let without_scheme=value.strip_prefix("https://").or_else(||value.strip_prefix("http://")).unwrap_or(value);
 without_scheme.strip_prefix("www.").unwrap_or(without_scheme)
}
fn podskazki(history:Vec<Zapis>,bookmarks:Vec<Zapis>,query:&str,limit:usize)->Vec<Podskazka>{
 let query=query.trim().to_lowercase();if query.is_empty()||query.chars().count()>512{return vec![];}
 let needle=adres_bez_shemy(&query);
 let mut unique:HashMap<String,Podskazka>=HashMap::new();
 for (source,items) in [("istoriya",history),("zakladki",bookmarks)]{
  for item in items{if !network(&item.adres){continue;}
   match unique.get_mut(&item.adres){
    Some(old)=>{old.vremya=old.vremya.max(item.vremya);if source=="zakladki"{old.istochnik=source.into();if !item.nazvanie.is_empty(){old.nazvanie=item.nazvanie;}}},
    None=>{unique.insert(item.adres.clone(),Podskazka{adres:item.adres,nazvanie:item.nazvanie,istochnik:source.into(),vremya:item.vremya});}
   }
  }
 }
 let mut results=unique.into_values().filter_map(|item|{
  let address=item.adres.to_lowercase();let address=adres_bez_shemy(&address);
  let title=item.nazvanie.to_lowercase();
  let rank=if address==needle{0}else if address.starts_with(needle){1}else if title.starts_with(&query){2}else if address.contains(needle)||title.contains(&query){3}else{return None;};
  Some((rank,item))
 }).collect::<Vec<_>>();
 results.sort_by(|(ar,a),(br,b)|ar.cmp(br).then_with(||b.vremya.cmp(&a.vremya)).then_with(||a.adres.cmp(&b.adres)));
 results.into_iter().take(limit.clamp(1,20)).map(|(_,i)|i).collect()
}
/// Suggestions are drawn exclusively from already persisted, non-private history/bookmarks.
#[tauri::command]
pub async fn biblioteka_podskazki(query:String,limit:Option<usize>)->Result<Vec<Podskazka>,String>{
 let _lock=IO.lock().map_err(|e|e.to_string())?;
 Ok(podskazki(chitat("istoriya.json"),chitat("zakladki.json"),&query,limit.unwrap_or(8)))
}
fn nachalo_perioda(period:&str,now:u64)->Result<Option<u64>,String>{
 let seconds=match period{"hour"=>3600,"day"=>86400,"week"=>7*86400,"month"=>28*86400,"all"=>return Ok(None),_=>return Err("Неизвестный период очистки".into())};
 Ok(Some(now.saturating_sub(seconds)))
}
#[derive(Serialize)]
pub struct Obem{pub istoriya:usize,pub istoriya_vsego:usize,pub zagruzki:usize,pub legacy:usize}
#[tauri::command]
pub async fn dannye_obem(period:String,super_id:Option<String>)->Result<Obem,String>{
 let since=nachalo_perioda(&period,vremya())?;
 let (selected,total,legacy)={let _lock=IO.lock().map_err(|e|e.to_string())?;let items=chitat("istoriya.json");let scoped=items.iter().filter(|i|super_id.as_ref().is_none_or(|id|i.super_id.as_ref()==Some(id))).collect::<Vec<_>>();(scoped.iter().filter(|i|since.is_none_or(|s|i.vremya>=s)).count(),scoped.len(),items.iter().filter(|i|i.super_id.is_none()).count())};
 let downloads=crate::zagruzki::zagruzki_spisok().await.into_iter().filter(|d|super_id.as_ref().is_none_or(|id|&d.super_id==id)&&!["Загружается","Приостановлена"].contains(&d.status.as_str())).count();
 Ok(Obem{istoriya:selected,istoriya_vsego:total,zagruzki:downloads,legacy})
}
#[derive(Serialize)]
pub struct Ochistka{pub istoriya:usize,pub zagruzki:usize}
#[tauri::command]
pub async fn dannye_ochistit(app:tauri::AppHandle,period:String,istoriya:bool,zagruzki:bool,super_id:Option<String>,legacy_history:Option<bool>)->Result<Ochistka,String>{
 let since=nachalo_perioda(&period,vremya())?;
 let removed=if istoriya{
  let _lock=IO.lock().map_err(|e|e.to_string())?;let mut items=chitat("istoriya.json");let before=items.len();
  items.retain(|i|!(super_id.as_ref().is_none_or(|id|i.super_id.as_ref()==Some(id))||(legacy_history==Some(true)&&i.super_id.is_none()))||since.is_some_and(|s|i.vremya<s));zapisat("istoriya.json",&items)?;before-items.len()
 }else{0};
 if istoriya{let _=app.emit("istoriya-izmenilas",());}
 // Deleting download records deliberately leaves files and live downloads intact.
 let downloads=if zagruzki{crate::zagruzki::ochistit_scope(None,super_id.as_deref())?}else{0};
 Ok(Ochistka{istoriya:removed,zagruzki:downloads})
}

#[cfg(test)]
mod tests{
 use super::*;
 fn item(address:&str,title:&str,time:u64)->Zapis{Zapis{super_id:None,id:address.into(),adres:address.into(),nazvanie:title.into(),vremya:time,papka:String::new()}}
 #[test]fn address_prefix_outweighs_recent_title_match_and_deduplicates(){
  let result=podskazki(vec![item("https://example.test/path","Old",1),item("https://other.test/","example",99)],vec![item("https://example.test/path","Saved",2)],"exa",8);
  assert_eq!(result.len(),2);assert_eq!(result[0].adres,"https://example.test/path");assert_eq!(result[0].istochnik,"zakladki");assert_eq!(result[0].nazvanie,"Saved");
 }
 #[test]fn suggestions_reject_non_web_and_empty_queries(){
  assert!(podskazki(vec![item("file:///private","secret",1)],vec![],"secret",8).is_empty());
  assert!(podskazki(vec![item("https://example.test/","example",1)],vec![]," ",8).is_empty());
 }
 #[test]fn cleanup_period_is_bounded_and_rejects_unknown(){assert_eq!(nachalo_perioda("hour",7200).unwrap(),Some(3600));assert_eq!(nachalo_perioda("all",7200).unwrap(),None);assert!(nachalo_perioda("unknown",7200).is_err());}
}

/// Sync touches only bookmark records, never the history/profile files.
pub fn sync_bookmarks()->Result<Vec<Zapis>,String>{let _lock=IO.lock().map_err(|_|"История занята")?;match std::fs::read(fajl("zakladki.json")){Ok(raw)=>serde_json::from_slice(&raw).map_err(|_|"Повреждены закладки; синхронизация остановлена".into()),Err(e) if e.kind()==std::io::ErrorKind::NotFound=>Ok(vec![]),Err(_)=>Err("Не удалось прочитать закладки".into())}}
pub fn sync_apply_bookmark(app:&tauri::AppHandle,id:&str,value:Option<Zapis>,expected:Option<&str>)->Result<(),String>{
 let _lock=IO.lock().map_err(|_|"Закладки заняты")?;
 let mut items=match std::fs::read(fajl("zakladki.json")){Ok(raw)=>serde_json::from_slice::<Vec<Zapis>>(&raw).map_err(|_|"Повреждены закладки")?,Err(e) if e.kind()==std::io::ErrorKind::NotFound=>vec![],Err(_)=>return Err("Не удалось прочитать закладки".into())};
 let current=items.iter().find(|i|i.id==id).map(serde_json::to_string).transpose().map_err(|_|"Некорректная закладка")?;
 if current.as_deref()!=expected{return Err("Закладка изменилась во время синхронизации".into());}
 items.retain(|i|i.id!=id);if let Some(mut item)=value{item.super_id=None;items.push(item);}zapisat("zakladki.json",&items)?;let _=app.emit("zakladki-izmenilis",());Ok(())
}
