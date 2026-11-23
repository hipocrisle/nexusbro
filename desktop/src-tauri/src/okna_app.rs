//! Дополнительные окна оболочки; страницы остаются в CEF-процессе своей Super.
use std::collections::{HashMap,HashSet};
use std::sync::{Mutex,Arc,atomic::{AtomicU64,Ordering}};
use tauri::{Emitter,Manager};
use crate::{Sostoyanie,sessiya::{Komanda,Sessiya},okna::{Mesto,sistema}};

#[derive(Clone)]
pub struct Vid{pub hwnd:Option<u32>,pub sessiya:Option<Sessiya>,pub engine:u64}
pub struct Okno {
    pub others:HashMap<String,Vid>,
    pub pending_close:HashSet<String>,
    pub super_id:String,
    pub engine:u64,
    pub hwnd:Option<u32>,
    pub mesto:Option<Mesto>,
    pub sessiya:Option<Sessiya>,
    // First ACK of this native attachment, immutable across reorder/focus updates.
    pub initial_session:Option<Sessiya>,
    pub hidden:bool,
    pub closing:bool,
    pub privatnaya:bool,
    pub generation:Arc<AtomicU64>,
    pub recovery_key:String,
    pub recovery_lazy:bool,
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
    let show=!crate::browser_lock::locked_global()&&!hidden&&ready&&mesto.is_some();
    if let Some(hwnd)=hwnd{if show{if let Some(m)=mesto{let _=sistema::peremestit(hwnd,m);}}let _=sistema::pokazat(hwnd,show);}
    #[cfg(target_os="windows")]{let seq=generation.fetch_add(1,Ordering::SeqCst)+1;let ui=window.clone();let _=window.run_on_main_thread(move||{if generation.load(Ordering::SeqCst)!=seq{return;}if let Ok(h)=ui.hwnd(){let _=crate::win32_oblast::primenit(windows::Win32::Foundation::HWND(h.0),if show{mesto}else{None});}});}
    #[cfg(not(target_os="windows"))]let _=(window,generation);
}
pub fn sobytie(app:&tauri::AppHandle,s:&Sostoyanie,super_id:&str,data:&serde_json::Value)->bool{
    let label=data["label"].as_str().unwrap_or("");
    if data["tip"]=="okno-closed"{
        let done={let mut map=s.okna.0.lock().unwrap();if let Some(w)=map.get_mut(label){if w.closing{w.pending_close.remove(super_id);w.pending_close.is_empty()}else{false}}else{false}};
        if done{if let Some(window)=app.get_webview_window(label){let _=window.destroy();}}
        return true;
    }
    if !["okno-ready","okno-session"].contains(&data["tip"].as_str().unwrap_or("")){return false;}
    let session={let mut map=s.okna.0.lock().unwrap();let Some(w)=map.get_mut(label)else{return true};
        if w.super_id!=super_id{if let Some(v)=w.others.get_mut(super_id){if let Some(h)=data["hwnd"].as_u64(){v.hwnd=Some(h as u32);let _=sistema::pokazat(h as u32,false);}if let Ok(session)=serde_json::from_value::<Sessiya>(data["sessiya"].clone()){v.sessiya=Some(session);}}drop(map);persist(app,s);return true;}
        if let Some(hwnd)=data["hwnd"].as_u64(){w.hwnd=Some(hwnd as u32);}
        if let Ok(session)=serde_json::from_value::<Sessiya>(data["sessiya"].clone()){
            if data["tip"]=="okno-ready"{w.initial_session=Some(session.clone());}
            w.sessiya=Some(session);
        }
        w.sessiya.clone()};
    persist(app,s);
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
pub async fn pokazat_glavnoe(app:tauri::AppHandle,s:tauri::State<'_,Sostoyanie>)->Result<(),String>{
    let channels=s.vkladki.lock().unwrap().values().filter_map(|v|v.kanal.clone()).collect::<Vec<_>>();
    for channel in channels{let _=channel.try_send(Komanda::GlavnoePokazat);}
    let window=app.get_webview_window("main").ok_or("Окно управления недоступно")?;
    window.show().map_err(|e|e.to_string())?;
    crate::fokus_obolochke(window).await
}
#[tauri::command]
pub async fn novoe_okno(app:tauri::AppHandle,s:tauri::State<'_,Sostoyanie>,id:Option<String>,privatnaya:bool)->Result<(),String>{
    let id=id.or_else(||s.aktivnaya.lock().unwrap().clone()).ok_or("Сначала откройте супер-вкладку")?;
    let running=s.vkladki.lock().unwrap().get(&id).is_some_and(|v|v.proc.is_some());
    if !running{let result=crate::zapustit_vnutri(&app,&s,&id).await;if !result.ok{return Err(result.soobshchenie);}}
    create_window(&app,&s,id,privatnaya,None)
}
fn create_window(app:&tauri::AppHandle,s:&Sostoyanie,id:String,privatnaya:bool,recovery:Option<crate::window_recovery::WindowSession>)->Result<(),String>{
    static NEXT:AtomicU64=AtomicU64::new(1);
    let label=loop{let label=format!("browser-{}",NEXT.fetch_add(1,Ordering::Relaxed));if app.get_webview_window(&label).is_none(){break label;}};
    let recovering=recovery.is_some();
    let recovery_key=recovery.as_ref().map(|w|w.key.clone()).unwrap_or_else(||format!("window-{}-{}",std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos(),NEXT.fetch_add(1,Ordering::Relaxed)));
    let mut session=None;let mut others=HashMap::new();
    if let Some(recovery)=recovery{for view in recovery.views{if view.super_id==id{session=Some(view.session);}else{others.insert(view.super_id,Vid{hwnd:None,sessiya:Some(view.session),engine:0});}}}
    let mut query=reqwest::Url::parse("http://localhost/index.html").map_err(|e|e.to_string())?;
    query.query_pairs_mut().append_pair("okno",&label).append_pair("super",&id);
    let url=format!("index.html?{}",query.query().unwrap_or(""));
    let title=zagolovok(&s,&id,privatnaya);
    let main=app.get_webview_window("main");let monitor=main.as_ref().and_then(|w|w.current_monitor().ok().flatten());
    let (limit_w,limit_h)=monitor.as_ref().map(|m|{let k=m.scale_factor().max(0.1);let area=m.work_area();let w=if area.size.width>0{area.size.width}else{m.size().width};let h=if area.size.height>0{area.size.height}else{m.size().height};((w as f64/k-40.).max(320.),(h as f64/k-80.).max(240.))}).unwrap_or((1200.,800.));
    let preferred=main.as_ref().and_then(|w|Some((w.inner_size().ok()?,w.scale_factor().ok()?))).map(|(size,k)|(size.width as f64/k,size.height as f64/k)).unwrap_or((1200.,800.));
    let width=preferred.0.min(1200.).min(limit_w).max(900_f64.min(limit_w));let height=preferred.1.min(800.).min(limit_h).max(560_f64.min(limit_h));
    let theme=main.as_ref().and_then(|w|w.theme().ok());let maximized=main.as_ref().is_some_and(|w|w.is_maximized().unwrap_or(false));
    s.okna.0.lock().unwrap().insert(label.clone(),Okno{others,pending_close:Default::default(),super_id:id,engine:0,hwnd:None,mesto:None,sessiya:session,initial_session:None,hidden:false,closing:false,privatnaya,generation:Default::default(),recovery_key,recovery_lazy:recovering});
    match tauri::WebviewWindowBuilder::new(app,&label,tauri::WebviewUrl::App(url.into())).title(title).focused(!recovering).inner_size(width,height).min_inner_size(900_f64.min(limit_w),560_f64.min(limit_h)).theme(theme).disable_drag_drop_handler().enable_clipboard_access().build(){
        Ok(window)=>{crate::browser_lock::install_shortcut(&window);if let Some(m)=monitor{let area=m.work_area();let pos=if area.size.width>0{area.position}else{*m.position()};let _=window.set_position(tauri::PhysicalPosition::new(pos.x+20,pos.y+20));}if maximized{let _=window.maximize();}persist(app,s);Ok(())},Err(e)=>{s.okna.0.lock().unwrap().remove(&label);Err(e.to_string())}
    }
}
#[tauri::command]
pub async fn okno_gotovo(app:tauri::AppHandle,window:tauri::WebviewWindow,s:tauri::State<'_,Sostoyanie>,)->Result<(),String>{
    let label=window.label().to_owned();
    let id=s.okna.0.lock().unwrap().get(&label).map(|w|w.super_id.clone()).ok_or("Окно не найдено")?;
    if s.okna.0.lock().unwrap().get(&label).is_some_and(|w|w.recovery_lazy){return Ok(());}
    // BrowserGuard remounts the shell after a master-password unlock. Keep the
    // existing native container/session: the utility already owns this label.
    let existing = {
        let map=s.okna.0.lock().unwrap();
        map.get(&label).and_then(|w|w.hwnd.map(|_|w.engine))
    };
    let current=s.vkladki.lock().unwrap().get(&id).map(|v|v.zapusk_id);
    if existing.is_some() && existing==current {
        obolochka(&app,&s,&label);
        return Ok(());
    }
    // Команда может прийти, пока CEF ещё запускается. Ждём канал без блокировки UI.
    let deadline=std::time::Instant::now()+std::time::Duration::from_secs(25);
    while kanal(&s,&label,&id).is_err(){if std::time::Instant::now()>=deadline{return Err("Движок не ответил при открытии окна".into());}tokio::time::sleep(std::time::Duration::from_millis(100)).await;}
    podklyuchit(&app,&s,&label)?;Ok(())
}
fn podklyuchit(app:&tauri::AppHandle,s:&Sostoyanie,label:&str)->Result<(),String>{
    let window=app.get_webview_window(label).ok_or("Окно уже закрыто")?;
    let parent=crate::nash_descriptor(&window).ok_or("Не найден дескриптор окна")?;
    let (id,private,session,mesto)={let map=s.okna.0.lock().unwrap();let w=map.get(label).ok_or("Окно уже закрыто")?;if w.closing{return Err("Окно уже закрывается".into());}(w.super_id.clone(),w.privatnaya,w.sessiya.clone(),w.mesto.unwrap_or(Mesto{x:0,y:100,w:1200,h:700}))};
    let engine=s.vkladki.lock().unwrap().get(&id).map(|v|v.zapusk_id).ok_or("Супер-вкладка закрыта")?;
    kanal(s,label,&id)?.try_send(Komanda::OknoNovoe{label:label.into(),parent,x:mesto.x,y:mesto.y,w:mesto.w,h:mesto.h,privatnaya:private,sessiya:session}).map_err(|e|e.to_string())?;
    if let Some(w)=s.okna.0.lock().unwrap().get_mut(label){w.engine=engine;w.hwnd=None;w.initial_session=None;}
    obolochka(app,s,label);Ok(())
}
pub fn vosstanovit(app:&tauri::AppHandle,s:&Sostoyanie,id:&str,engine:u64){
    let closed=s.okna.0.lock().unwrap().iter().filter(|(_,w)|w.super_id==id&&w.closing).map(|(label,_)|label.clone()).collect::<Vec<_>>();
    for label in closed{sobytie(app,s,id,&serde_json::json!({"tip":"okno-closed","label":label}));}
    let labels=s.okna.0.lock().unwrap().iter().filter(|(_,w)|w.super_id==id&&w.engine!=engine&&!w.closing&&!w.recovery_lazy).map(|(label,_)|label.clone()).collect::<Vec<_>>();
    for label in labels{if let Err(e)=podklyuchit(app,s,&label){let _=app.emit_to(&label,"oshibka-dvizhka",e);}}
}
pub fn zakryt_super(app:&tauri::AppHandle,s:&Sostoyanie,id:&str){
 let labels={let mut map=s.okna.0.lock().unwrap();let mut labels=Vec::new();for(label,w) in map.iter_mut(){w.others.remove(id);if w.super_id==id{if let Some(h)=w.hwnd.take(){let _=sistema::pokazat(h,false);}w.super_id.clear();w.sessiya=None;w.engine=0;labels.push(label.clone());}}labels};
 for label in labels{obolochka(app,s,&label);}
 persist(app,s);
}
// Сначала CEF завершает BrowserHost, затем уничтожаем родителя Tauri.
// Обратный порядок мог пересоздать последнюю вкладку в уже удалённом HWND.
pub fn nachat_zakrytie(app:&tauri::AppHandle,label:&str)->bool{
    let s:tauri::State<Sostoyanie>=app.state();
    let ids={let mut map=s.okna.0.lock().unwrap();let Some(w)=map.get_mut(label)else{return false};if w.closing{return true;}
        let mut ids=w.others.keys().cloned().collect::<HashSet<_>>();if !w.super_id.is_empty(){ids.insert(w.super_id.clone());}w.pending_close=ids.clone();w.closing=true;ids};
    persist(app,&s);
    if let Some(window)=app.get_webview_window(label){let _=window.hide();}
    if ids.is_empty(){if let Some(window)=app.get_webview_window(label){let _=window.destroy();}return true;}
    for id in ids {
        let channel=s.vkladki.lock().unwrap().get(&id).and_then(|v|v.kanal.clone());
        if let Some(channel)=channel {
            let app=app.clone();let label=label.to_owned();
            // send выполняется вне UI: закрытие родителя ждёт ACK всех CEF-процессов.
            std::thread::spawn(move||{if channel.send(Komanda::OknoZakryt{label:label.clone()}).is_err(){let s:tauri::State<Sostoyanie>=app.state();sobytie(&app,&s,&id,&serde_json::json!({"tip":"okno-closed","label":label}));}});
        }else{sobytie(app,&s,&id,&serde_json::json!({"tip":"okno-closed","label":label}));}
    }
    true
}

pub fn zakryt(app:&tauri::AppHandle,label:&str){
    let s:tauri::State<Sostoyanie>=app.state();
    if let Some(w)=s.okna.0.lock().unwrap().remove(label){
        if let Ok(k)=kanal(&s,"main",&w.super_id){let _=k.try_send(Komanda::OknoZakryt{label:label.into()});}
    }
    persist(app,&s);
    // Главная оболочка скрывается, пока открыты другие окна. Последнее окно завершает приложение.
    if s.okna.0.lock().unwrap().is_empty()&&app.get_webview_window("main").is_some_and(|w|!w.is_visible().unwrap_or(true)){app.exit(0);}
}

/// Полная оболочка в каждом окне; CEF-виды других Super сохраняются скрытыми.
pub async fn vybrat(app:tauri::AppHandle,s:tauri::State<'_,Sostoyanie>,window:tauri::WebviewWindow,id:String)->Result<crate::Otvet,String>{
    let label=window.label().to_owned();
    let running=s.vkladki.lock().unwrap().get(&id).is_some_and(|v|v.proc.is_some());
    if !running{let previous=s.aktivnaya.lock().unwrap().clone();let result=crate::zapustit_vnutri(&app,&s,&id).await;if let Some(previous)=previous{crate::pereklyuchit_vnutri(&s,&previous);}if !result.ok{return Ok(result);}}
    let engine=s.vkladki.lock().unwrap().get(&id).map(|v|v.zapusk_id).unwrap_or(0);
    let cached={let mut map=s.okna.0.lock().unwrap();let w=map.get_mut(&label).ok_or("Окно уже закрыто")?;if w.closing{return Err("Окно закрывается".into());}
        if w.super_id!=id{let old=Vid{hwnd:w.hwnd.take(),sessiya:w.sessiya.take(),engine:w.engine};if let Some(h)=old.hwnd{let _=sistema::pokazat(h,false);}w.others.insert(w.super_id.clone(),old);
            let next=w.others.remove(&id);w.super_id=id.clone();w.hwnd=next.as_ref().and_then(|v|v.hwnd);w.sessiya=next.as_ref().and_then(|v|v.sessiya.clone());w.engine=next.as_ref().map(|v|v.engine).unwrap_or(0);
        }
        w.hidden=false;
        w.recovery_lazy=false;
        w.engine==engine&&w.hwnd.is_some_and(sistema::okno_zhivo)
    };
    if cached{obolochka(&app,&s,&label);let session=s.okna.0.lock().unwrap().get(&label).and_then(|w|w.sessiya.clone());if let Some(session)=session{let _=app.emit_to(&label,"obychnye-izmenilis",serde_json::json!({"id":id,"sessiya":session}));}}
    else{podklyuchit(&app,&s,&label)?;}
    let private=s.okna.0.lock().unwrap().get(&label).is_some_and(|w|w.privatnaya);let _=window.set_title(&zagolovok(&s,&id,private));
    persist(&app,&s);
    Ok(crate::da())
}

fn zagolovok(s:&Sostoyanie,id:&str,private:bool)->String{
 let (name,private)=s.vkladki.lock().unwrap().get(id).map(|v|(Some(v.nastrojki.nazvanie.clone()),private||v.nastrojki.privatnaya)).unwrap_or((None,private));
 format!("NexusBro{}{}",name.map(|n|format!(" — {n}")).unwrap_or_default(),if private{" · приватное окно"}else{""})
}

#[derive(Default)]
struct RecoveryState {
    initialized:bool,
    restoring:bool,
    shutting_down:bool,
    pending:HashMap<String,crate::window_recovery::WindowSession>,
    committed:HashMap<String,crate::window_recovery::WindowSession>,
}
fn recovery_state()->&'static Mutex<RecoveryState>{
    static STATE:std::sync::OnceLock<Mutex<RecoveryState>>=std::sync::OnceLock::new();
    STATE.get_or_init(||Mutex::new(RecoveryState::default()))
}
fn recovery_root()->std::path::PathBuf{
    crate::brauzer::papka_profilej().parent().unwrap_or_else(||std::path::Path::new(".")).to_path_buf()
}
fn ordinary_supers(s:&Sostoyanie)->HashSet<String>{
    s.vkladki.lock().unwrap().iter().filter(|(id,v)|!v.nastrojki.privatnaya&&!crate::privatnost::eto_id(id)).map(|(id,_)|id.clone()).collect()
}
fn persist(app:&tauri::AppHandle,s:&Sostoyanie){
    let mut state=recovery_state().lock().unwrap_or_else(|e|e.into_inner());
    if !state.initialized||state.restoring||state.shutting_down{return;}
    let allowed=ordinary_supers(s);
    state.pending.retain(|_,window|allowed.contains(&window.active_super));
    let mut windows=state.committed.clone();
    windows.extend(state.pending.clone());
    {
        let map=s.okna.0.lock().unwrap();
        let live=map.values().filter(|w|!w.closing&&!w.privatnaya&&allowed.contains(&w.super_id)).map(|w|w.recovery_key.clone()).collect::<HashSet<_>>();
        windows.retain(|key,window|allowed.contains(&window.active_super)&&(live.contains(key)||state.pending.contains_key(key)));
        for w in map.values(){
            if w.closing{windows.remove(&w.recovery_key);continue;}
            if w.privatnaya||!allowed.contains(&w.super_id){continue;}
            let mut views=w.others.iter().filter(|(id,_)|allowed.contains(*id)).filter_map(|(id,v)|v.sessiya.clone().map(|session|crate::window_recovery::SuperSession{super_id:id.clone(),session})).collect::<Vec<_>>();
            if let Some(session)=w.sessiya.clone(){views.push(crate::window_recovery::SuperSession{super_id:w.super_id.clone(),session});}
            views.sort_by(|a,b|a.super_id.cmp(&b.super_id));
            if views.iter().any(|v|v.super_id==w.super_id){windows.insert(w.recovery_key.clone(),crate::window_recovery::WindowSession{key:w.recovery_key.clone(),active_super:w.super_id.clone(),views,privatnaya:false});}
        }
    }
    let committed=windows.clone();
    let mut windows=windows.into_values().collect::<Vec<_>>();windows.sort_by(|a,b|a.key.cmp(&b.key));
    if crate::window_recovery::save_allowed(&recovery_root(),windows,&allowed).is_err(){let _=app.emit("oshibka-dvizhka","Не удалось сохранить окна для восстановления");}
    else{state.committed=committed;}
}

/// Run once after successful master-password unlock (or normal unlocked startup).
/// Recreate shells with lazy session snapshots, but start NO CEF engines here.
pub fn restore_windows(app:&tauri::AppHandle){
    if crate::browser_lock::locked_global(){return;}
    let s:tauri::State<Sostoyanie>=app.state();
    let allowed=ordinary_supers(&s);
    let pending={
        let mut state=recovery_state().lock().unwrap_or_else(|e|e.into_inner());
        if state.initialized{return;}
        state.initialized=true;state.restoring=true;
        for mut window in crate::window_recovery::load(&recovery_root()){
            window.views.retain(|view|allowed.contains(&view.super_id));
            if allowed.contains(&window.active_super)&&window.views.iter().any(|v|v.super_id==window.active_super){state.pending.insert(window.key.clone(),window);}
        }
        state.committed=state.pending.clone();
        state.pending.values().cloned().collect::<Vec<_>>()
    };
    for window in pending{
        let key=window.key.clone();let id=window.active_super.clone();
        match create_window(app,&s,id,false,Some(window)){
            Ok(())=>{recovery_state().lock().unwrap_or_else(|e|e.into_inner()).pending.remove(&key);},
            Err(_)=>{let _=app.emit("oshibka-dvizhka","Не удалось восстановить одно из окон. Его сессия сохранена для следующего запуска.");},
        }
    }
    recovery_state().lock().unwrap_or_else(|e|e.into_inner()).restoring=false;
    persist(app,&s);
}

/// Call on app-wide ExitRequested before window destruction. Explicit user
/// window close is persisted by nachat_zakrytie; teardown must not erase others.
pub fn before_exit(app:&tauri::AppHandle){
    let s:tauri::State<Sostoyanie>=app.state();
    persist(app,&s);
    recovery_state().lock().unwrap_or_else(|e|e.into_inner()).shutting_down=true;
}

/// Prepare an ordinary command for a lazily restored secondary window. Waiting
/// for the native session ACK is mandatory: old disk IDs are not browser IDs.
pub async fn ready_command(app:&tauri::AppHandle,s:&Sostoyanie,label:&str,command:Komanda)->Result<Komanda,String>{
    if crate::browser_lock::locked_global(){return Err("Браузер заблокирован".into());}
    static SERIAL:tokio::sync::Mutex<()>=tokio::sync::Mutex::const_new(());
    let _serial=SERIAL.lock().await;
    let (id,before,lazy)={let map=s.okna.0.lock().unwrap();let w=map.get(label).filter(|w|!w.closing).ok_or("Окно уже закрыто")?;(w.super_id.clone(),w.sessiya.clone(),w.recovery_lazy)};
    if lazy{
        if !crate::window_recovery::user_command_can_wake(&command){return Err("Окно восстановлено и ожидает выбора вкладки".into());}
        if let Some(w)=s.okna.0.lock().unwrap().get_mut(label){
            if let Komanda::Vybrat{id}= &command {
                let session=w.sessiya.as_mut().ok_or("Сессия окна недоступна")?;
                if !session.vkladki.iter().any(|v|v.id==*id){return Err("Вкладка уже закрыта".into());}
                session.aktivnaya=*id;
            }
            w.recovery_lazy=false;
        }
        let running=s.vkladki.lock().unwrap().get(&id).is_some_and(|v|v.proc.is_some()&&!v.zapusk_idet);
        if !running{
            let previous=s.aktivnaya.lock().unwrap().clone();
            let result=crate::zapustit_vnutri(app,&app.state(),&id).await;
            if let Some(previous)=previous{crate::pereklyuchit_vnutri(&app.state(),&previous);}
            if !result.ok{if let Some(w)=s.okna.0.lock().unwrap().get_mut(label){w.recovery_lazy=true;}return Err(result.soobshchenie);}
        }
        let engine=s.vkladki.lock().unwrap().get(&id).map(|v|v.zapusk_id).ok_or("Супер-вкладка уже закрыта")?;
        let needs=s.okna.0.lock().unwrap().get(label).is_some_and(|w|w.engine!=engine);
        if needs{podklyuchit(app,s,label)?;}
    }
    let deadline=std::time::Instant::now()+std::time::Duration::from_secs(25);
    let after=loop{
        if crate::browser_lock::locked_global(){return Err("Браузер заблокирован".into());}
        let session={let map=s.okna.0.lock().unwrap();let w=map.get(label).filter(|w|!w.closing&&w.super_id==id).ok_or("Окно уже изменилось")?;if w.hwnd.is_some(){w.sessiya.clone()}else{None}};
        if let Some(session)=session{break session;}
        if std::time::Instant::now()>=deadline{return Err("Движок не подтвердил восстановление окна. Повторите действие.".into());}
        tokio::time::sleep(std::time::Duration::from_millis(40)).await;
    };
    persist(app,s);
    match before{Some(before)=>crate::window_recovery::remap_command(&before,&after,command),None=>Ok(command)}
}


/// Select a search target before attaching a cold secondary CEF view. Selecting
/// the previous active page and switching afterwards would make an unwanted
/// network request (and could expose the wrong account route).
pub async fn prepare_search(
    app: &tauri::AppHandle,
    s: &Sostoyanie,
    window: &tauri::WebviewWindow,
    entry: &crate::tab_search::Entry,
) -> Result<(u64, u64), String> {
    let changed = "Список вкладок изменился. Повторите поиск.";
    let target = entry.tab_id.ok_or(changed)?;
    let (running, engine_before) = {
        let all = s.vkladki.lock().map_err(|_| "Список занят")?;
        let v = all
            .get(&entry.super_id)
            .filter(|v| {
                !v.nastrojki.privatnaya
                    && !crate::privatnost::eto_id(&v.nastrojki.id)
                    && v.zapusk_id == entry.engine_generation
                    && crate::tab_search::profile_guard(&v.nastrojki)==entry.profile_guard
            })
            .ok_or(changed)?;
        (
            (v.proc.is_some() || v.zapas_pid.is_some()) && !v.zapusk_idet,
            v.zapusk_id,
        )
    };
    let before = {
        let mut windows = s.okna.0.lock().map_err(|_| "Список окон занят")?;
        let w = windows
            .get_mut(&entry.window)
            .filter(|w| !w.closing && !w.privatnaya)
            .ok_or(changed)?;
        let (session, view, attached, lazy) = if w.super_id == entry.super_id {
            (
                w.sessiya.as_ref(),
                w.engine,
                w.hwnd.is_some_and(sistema::okno_zhivo),
                w.recovery_lazy,
            )
        } else {
            let v = w.others.get(&entry.super_id).ok_or(changed)?;
            (
                v.sessiya.as_ref(),
                v.engine,
                v.hwnd.is_some_and(sistema::okno_zhivo),
                false,
            )
        };
        let before = session.ok_or(changed)?.clone();
        if view != entry.view_generation
            || (lazy || !attached || view != engine_before) != entry.cold
        {
            return Err(changed.into());
        }
        crate::tab_search::validate(&before, entry)?;
        if w.super_id != entry.super_id {
            let old_id = w.super_id.clone();
            let old = Vid {
                hwnd: w.hwnd.take(),
                sessiya: w.sessiya.take(),
                engine: w.engine,
            };
            if let Some(h) = old.hwnd {
                let _ = sistema::pokazat(h, false);
            }
            w.others.insert(old_id, old);
            let next = w.others.remove(&entry.super_id).ok_or(changed)?;
            w.super_id = entry.super_id.clone();
            w.hwnd = next.hwnd;
            w.sessiya = next.sessiya;
            w.engine = next.engine;
            w.initial_session = None;
        }
        if entry.cold {
            w.sessiya.as_mut().ok_or(changed)?.aktivnaya = target;
            // Prevent the engine startup callback from attaching this view with
            // an old active tab before this explicit search preparation finishes.
            w.recovery_lazy = true;
        }
        w.hidden = false;
        before
    };
    let _=app.emit_to(&entry.window,"vkladki-izmenilis",&entry.super_id);
    if !running {
        let previous = s.aktivnaya.lock().map_err(|_| "Список занят")?.clone();
        let result = crate::zapustit_vnutri(app, &app.state(), &entry.super_id).await;
        if let Some(previous) = previous {
            crate::pereklyuchit_vnutri(&app.state(), &previous);
        }
        if !result.ok {
            return Err(result.soobshchenie);
        }
    }
    if crate::browser_lock::locked_global() {
        return Err("Браузер заблокирован".into());
    }
    let engine = {let all=s.vkladki.lock().map_err(|_|"Список занят")?;
        let v=all.get(&entry.super_id).filter(|v|crate::tab_search::profile_guard(&v.nastrojki)==entry.profile_guard).ok_or(changed)?;
        v.zapusk_id};
    if !entry.cold && engine != engine_before {
        return Err(changed.into());
    }
    let attach = {
        let mut windows = s.okna.0.lock().map_err(|_| "Список окон занят")?;
        let w = windows
            .get_mut(&entry.window)
            .filter(|w| !w.closing && !w.privatnaya && w.super_id == entry.super_id)
            .ok_or(changed)?;
        crate::tab_search::validate(w.sessiya.as_ref().ok_or(changed)?, entry)?;
        let attach = w.engine != engine || !w.hwnd.is_some_and(sistema::okno_zhivo);
        if attach {
            if !entry.cold {
                return Err(changed.into());
            }
            w.sessiya.as_mut().ok_or(changed)?.aktivnaya = target;
        }
        w.recovery_lazy = false;
        attach
    };
    if attach {
        podklyuchit(app, s, &entry.window)?;
    } else {
        obolochka(app, s, &entry.window);
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(25);
    let after = loop {
        if crate::browser_lock::locked_global() {
            return Err("Браузер заблокирован".into());
        }
        let session = {
            let windows = s.okna.0.lock().map_err(|_| "Список окон занят")?;
            let w = windows
                .get(&entry.window)
                .filter(|w| !w.closing && !w.privatnaya && w.super_id == entry.super_id)
                .ok_or(changed)?;
            if w.engine == engine && w.hwnd.is_some() {
                if entry.cold {
                    w.initial_session.clone()
                } else {
                    w.sessiya.clone()
                }
            } else {
                None
            }
        };
        if let Some(session) = session {
            break session;
        }
        if std::time::Instant::now() >= deadline {
            return Err("Движок не подтвердил окно. Повторите поиск.".into());
        }
        tokio::time::sleep(std::time::Duration::from_millis(40)).await;
    };
    let selected = if entry.cold {
        match crate::window_recovery::remap_command(
            &before,
            &after,
            Komanda::Vybrat { id: target },
        )? {
            Komanda::Vybrat { id } => id,
            _ => return Err(changed.into()),
        }
    } else {
        crate::tab_search::validate(&after, entry)?;
        target
    };
    // A later live reorder is harmless: selection now uses the native ID from
    // the first attachment ACK, not a URL/index guess against a later snapshot.
    let current = {
        let windows = s.okna.0.lock().map_err(|_| "Список окон занят")?;
        windows
            .get(&entry.window)
            .and_then(|w| w.sessiya.clone())
            .ok_or(changed)?
    };
    if !current.vkladki.iter().any(|t| {
        t.id == selected && !t.privatnaya && t.route == entry.route
    }) {
        return Err(changed.into());
    }
    let private = s
        .okna
        .0
        .lock()
        .map_err(|_| "Список окон занят")?
        .get(&entry.window)
        .is_some_and(|w| w.privatnaya);
    let _ = window.set_title(&zagolovok(s, &entry.super_id, private));
    persist(app, s);
    Ok((selected, engine))
}
