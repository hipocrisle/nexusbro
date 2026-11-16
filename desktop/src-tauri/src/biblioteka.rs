//! История и закладки. Автоматическая запись приватных страниц запрещена.
use serde::{Serialize,Deserialize};
use std::sync::{Mutex,OnceLock};
use std::collections::HashMap;
use std::path::PathBuf;
static IO:Mutex<()>=Mutex::new(());
static SEEN:OnceLock<Mutex<HashMap<(String,u64),String>>>=OnceLock::new();
#[derive(Clone,Serialize,Deserialize)]
pub struct Zapis {pub id:String,pub adres:String,pub nazvanie:String,pub vremya:u64,#[serde(default)]pub papka:String}
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
    let _lock=IO.lock().unwrap();let mut items=chitat("istoriya.json");items.retain(|i|i.adres!=tab.adres);
    items.insert(0,Zapis{id:format!("{}-{}",vremya(),tab.id),adres:tab.adres.clone(),nazvanie:tab.zagolovok.clone(),vremya:vremya(),papka:String::new()});items.truncate(5000);
    let _=zapisat("istoriya.json",&items);
}
#[tauri::command]
pub async fn biblioteka_spisok(razdel:String)->Result<Vec<Zapis>,String>{
    let _lock=IO.lock().unwrap();match razdel.as_str(){"istoriya"=>Ok(chitat("istoriya.json")),"zakladki"=>Ok(chitat("zakladki.json")),_=>Err("Неизвестный раздел".into())}
}
#[tauri::command]
pub async fn biblioteka_udalit(razdel:String,id:Option<String>)->Result<(),String>{
    let name=match razdel.as_str(){"istoriya"=>"istoriya.json","zakladki"=>"zakladki.json",_=>return Err("Неизвестный раздел".into())};
    let _lock=IO.lock().unwrap();let mut items=chitat(name);
    match id{Some(id)=>items.retain(|i|i.id!=id),None=>items.clear()};zapisat(name,&items)
}
#[tauri::command]
pub async fn zakladki_sohranit(adres:String,nazvanie:String,papka:Option<String>)->Result<(),String>{
    if !network(&adres){return Err("В закладки можно добавить HTTP(S)-страницу".into());}
    let _lock=IO.lock().unwrap();let mut items=chitat("zakladki.json");
    if let Some(i)=items.iter_mut().find(|i|i.adres==adres){i.nazvanie=nazvanie;i.papka=papka.unwrap_or_default();}
    else{items.push(Zapis{id:format!("b-{}-{}",vremya(),items.len()),adres,nazvanie,vremya:vremya(),papka:papka.unwrap_or_default()});}
    zapisat("zakladki.json",&items)
}
#[tauri::command]
pub async fn zakladki_import(zapisi:Vec<Zapis>)->Result<usize,String>{
    if zapisi.len()>20000{return Err("Слишком большой файл закладок".into());}
    let _lock=IO.lock().unwrap();let mut items=chitat("zakladki.json");let mut count=0;
    for mut i in zapisi{if network(&i.adres)&&!items.iter().any(|old|old.adres==i.adres){i.id=format!("b-{}-{}",vremya(),items.len());items.push(i);count+=1;}}
    zapisat("zakladki.json",&items)?;Ok(count)
}
