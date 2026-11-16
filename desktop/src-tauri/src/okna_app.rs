//! Дополнительные окна оболочки; страницы остаются в CEF-процессе своей Super.
use std::collections::HashMap;
use std::sync::{Mutex,Arc,atomic::{AtomicU64,Ordering}};
use tauri::{Emitter,Manager};
use crate::{Sostoyanie,sessiya::{Komanda,Sessiya},okna::{Mesto,sistema}};

pub struct Okno {
    pub super_id:String,
    pub engine:u64,
    pub hwnd:Option<u32>,
    pub mesto:Option<Mesto>,
    pub sessiya:Option<Sessiya>,
    pub hidden:bool,
    pub closing:bool,
    pub privatnaya:bool,
    pub generation:Arc<AtomicU64>,
}
#[derive(Default)]
pub struct Okna(pub Mutex<HashMap<String,Okno>>);

pub fn kanal(s:&Sostoyanie,label:&str,id:&str)->Result<std::sync::mpsc::SyncSender<Komanda>,String>{
    if !label.is_empty()&&label!="main"&&!s.okna.0.lock().unwrap().get(label).is_some_and(|w|w.super_id==id){return Err("Окно не принадлежит этой супер-вкладке".into());}
    s.vkladki.lock().unwrap().get(id).and_then(|v|v.kanal.clone()).ok_or("Движок ещё не запущен".into())
}
pub fn obolochka(app:&tauri::AppHandle,s:&Sostoyanie,label:&str){
    let Some(window)=app.get_webview_window(label)else{return};
    let (hwnd,mesto,hidden,generation,ready)={let map=s.okna.0.lock().unwrap();let Some(w)=map.get(label)else{return};(w.hwnd,w.mesto,w.hidden,w.generation.clone(),w.sessiya.as_ref().is_some_and(crate::stranica_gotova))};
    let show=!hidden&&ready&&mesto.is_some();
    if let Some(hwnd)=hwnd{if show{if let Some(m)=mesto{let _=sistema::peremestit(hwnd,m);}}let _=sistema::pokazat(hwnd,show);}
    #[cfg(target_os="windows")]{let seq=generation.fetch_add(1,Ordering::SeqCst)+1;let ui=window.clone();let _=window.run_on_main_thread(move||{if generation.load(Ordering::SeqCst)!=seq{return;}if let Ok(h)=ui.hwnd(){let _=crate::win32_oblast::primenit(windows::Win32::Foundation::HWND(h.0),if show{mesto}else{None});}});}
    #[cfg(not(target_os="windows"))]let _=(window,generation);
}
pub fn sobytie(app:&tauri::AppHandle,s:&Sostoyanie,super_id:&str,data:&serde_json::Value)->bool{
    let label=data["label"].as_str().unwrap_or("");
    if data["tip"]=="okno-closed"{
        if let Some(window)=app.get_webview_window(label){let _=window.destroy();}
        return true;
    }
    if !["okno-ready","okno-session"].contains(&data["tip"].as_str().unwrap_or("")){return false;}
    let session={let mut map=s.okna.0.lock().unwrap();let Some(w)=map.get_mut(label).filter(|w|w.super_id==super_id)else{return true};
        if let Some(hwnd)=data["hwnd"].as_u64(){w.hwnd=Some(hwnd as u32);}
        if let Ok(session)=serde_json::from_value::<Sessiya>(data["sessiya"].clone()){w.sessiya=Some(session);}
        w.sessiya.clone()};
    obolochka(app,s,label);
    if let Some(session)=session{for tab in &session.vkladki{crate::biblioteka::poseshchenie(super_id,tab);}let _=app.emit_to(label,"obychnye-izmenilis",serde_json::json!({"id":super_id,"sessiya":session}));}
    true
}
pub fn zapros_okna(app:tauri::AppHandle,id:Option<String>,privatnaya:bool){
    // Отдельная короткая задача разрывает цикл opaque Future Send:
    // запуск движка -> reader callback -> новое окно -> запуск движка.
    std::thread::spawn(move||{
        if let Err(e)=tauri::async_runtime::block_on(novoe_okno(app.clone(),app.state(),id,privatnaya)){
            let _=app.emit("oshibka-dvizhka",e);
            if let Some(w)=app.get_webview_window("main"){let _=w.show();let _=w.set_focus();}
        }
    });
}
#[tauri::command]
pub async fn novoe_okno(app:tauri::AppHandle,s:tauri::State<'_,Sostoyanie>,id:Option<String>,mut privatnaya:bool)->Result<(),String>{
    let id=id.or_else(||s.aktivnaya.lock().unwrap().clone()).ok_or("Сначала откройте супер-вкладку")?;
    privatnaya|=s.vkladki.lock().unwrap().get(&id).is_some_and(|v|v.nastrojki.privatnaya);
    let running=s.vkladki.lock().unwrap().get(&id).is_some_and(|v|v.proc.is_some());
    if !running{let result=crate::zapustit_vnutri(&app,&s,&id).await;if !result.ok{return Err(result.soobshchenie);}}
    static NEXT:AtomicU64=AtomicU64::new(1);
    let label=format!("browser-{}",NEXT.fetch_add(1,Ordering::Relaxed));
    let mut query=reqwest::Url::parse("http://localhost/index.html").map_err(|e|e.to_string())?;
    query.query_pairs_mut().append_pair("okno",&label).append_pair("super",&id);
    let url=format!("index.html?{}",query.query().unwrap_or(""));
    let title=s.vkladki.lock().unwrap().get(&id).map(|v|format!("NexusBro — {}{}",v.nastrojki.nazvanie,if privatnaya{" · приватное окно"}else{""})).unwrap_or_else(||"NexusBro".into());
    s.okna.0.lock().unwrap().insert(label.clone(),Okno{super_id:id,engine:0,hwnd:None,mesto:None,sessiya:None,hidden:false,closing:false,privatnaya,generation:Default::default()});
    match tauri::WebviewWindowBuilder::new(&app,&label,tauri::WebviewUrl::App(url.into())).title(title).inner_size(1200.,800.).build(){
        Ok(_)=>Ok(()),Err(e)=>{s.okna.0.lock().unwrap().remove(&label);Err(e.to_string())}
    }
}
#[tauri::command]
pub async fn okno_gotovo(app:tauri::AppHandle,window:tauri::WebviewWindow,s:tauri::State<'_,Sostoyanie>,)->Result<(),String>{
    let label=window.label().to_owned();
    let id=s.okna.0.lock().unwrap().get(&label).map(|w|w.super_id.clone()).ok_or("Окно не найдено")?;
    // Команда может прийти, пока CEF ещё запускается. Ждём канал без блокировки UI.
    let deadline=std::time::Instant::now()+std::time::Duration::from_secs(25);
    while kanal(&s,&label,&id).is_err(){if std::time::Instant::now()>=deadline{return Err("Движок не ответил при открытии окна".into());}tokio::time::sleep(std::time::Duration::from_millis(100)).await;}
    podklyuchit(&app,&s,&label)?;Ok(())
}
fn podklyuchit(app:&tauri::AppHandle,s:&Sostoyanie,label:&str)->Result<(),String>{
    let window=app.get_webview_window(label).ok_or("Окно уже закрыто")?;
    let parent=crate::nash_descriptor(&window).ok_or("Не найден дескриптор окна")?;
    let (id,private,session,mesto)={let map=s.okna.0.lock().unwrap();let w=map.get(label).ok_or("Окно уже закрыто")?;(w.super_id.clone(),w.privatnaya,w.sessiya.clone(),w.mesto.unwrap_or(Mesto{x:0,y:100,w:1200,h:700}))};
    let engine=s.vkladki.lock().unwrap().get(&id).map(|v|v.zapusk_id).ok_or("Супер-вкладка закрыта")?;
    kanal(s,label,&id)?.try_send(Komanda::OknoNovoe{label:label.into(),parent,x:mesto.x,y:mesto.y,w:mesto.w,h:mesto.h,privatnaya:private,sessiya:session}).map_err(|e|e.to_string())?;
    if let Some(w)=s.okna.0.lock().unwrap().get_mut(label){w.engine=engine;w.hwnd=None;}
    obolochka(app,s,label);Ok(())
}
pub fn vosstanovit(app:&tauri::AppHandle,s:&Sostoyanie,id:&str,engine:u64){
    let labels=s.okna.0.lock().unwrap().iter().filter(|(_,w)|w.super_id==id&&w.engine!=engine).map(|(label,_)|label.clone()).collect::<Vec<_>>();
    for label in labels{if let Err(e)=podklyuchit(app,s,&label){let _=app.emit_to(&label,"oshibka-dvizhka",e);}}
}
pub fn zakryt_super(app:&tauri::AppHandle,s:&Sostoyanie,id:&str){
    let labels=s.okna.0.lock().unwrap().iter().filter(|(_,w)|w.super_id==id).map(|(label,_)|label.clone()).collect::<Vec<_>>();
    for label in labels{if let Some(window)=app.get_webview_window(&label){let _=window.close();}}
}
// Сначала CEF завершает BrowserHost, затем уничтожаем родителя Tauri.
// Обратный порядок мог пересоздать последнюю вкладку в уже удалённом HWND.
pub fn nachat_zakrytie(app:&tauri::AppHandle,label:&str)->bool{
    let s:tauri::State<Sostoyanie>=app.state();
    let (id,closing)={let map=s.okna.0.lock().unwrap();let Some(w)=map.get(label)else{return false};(w.super_id.clone(),w.closing)};
    if closing{return true;}
    let Ok(channel)=kanal(&s,"main",&id)else{return false};
    match channel.try_send(Komanda::OknoZakryt{label:label.into()}){
        Ok(())=>{
            if let Some(w)=s.okna.0.lock().unwrap().get_mut(label){w.closing=true;}
            if let Some(window)=app.get_webview_window(label){let _=window.hide();}
            true
        }
        Err(std::sync::mpsc::TrySendError::Full(_))=>{let _=app.emit_to(label,"oshibka-dvizhka","Движок занят. Повторите закрытие окна.");true}
        Err(_)=>false,
    }
}
pub fn zakryt(app:&tauri::AppHandle,label:&str){
    let s:tauri::State<Sostoyanie>=app.state();
    if let Some(w)=s.okna.0.lock().unwrap().remove(label){
        if let Ok(k)=kanal(&s,"main",&w.super_id){let _=k.try_send(Komanda::OknoZakryt{label:label.into()});}
    }
    // Главная оболочка скрывается, пока открыты другие окна. Последнее окно завершает приложение.
    if s.okna.0.lock().unwrap().is_empty()&&app.get_webview_window("main").is_some_and(|w|!w.is_visible().unwrap_or(true)){app.exit(0);}
}
