//! Read-only snapshots; searching never launches an engine or visits a page.
use crate::{sessiya::{Komanda,Sessiya},Sostoyanie};
use serde::{Serialize,Deserialize};
use tauri::{Manager,State};
#[derive(Clone,Serialize,Deserialize)]
pub struct Entry {
 pub super_id:String,pub super_name:String,pub window:String,pub tab_id:Option<u64>,
 pub title:String,pub address:String,pub sleeping:bool,
 #[serde(default)] pub route_label:String,
 pub index:usize,pub route:crate::tab_routes::RouteChoice,
}
fn rows(out:&mut Vec<Entry>,id:&str,name:&str,label:&str,s:&Sessiya,sleeping:bool,connections:&std::collections::HashMap<String,String>){
 for (index,t) in s.vkladki.iter().enumerate(){
  // Global search intentionally excludes private browsing, even within ordinary Super.
  if t.privatnaya{continue;}
  if out.len()>=10_000{break;}
  out.push(Entry{super_id:id.into(),super_name:name.into(),window:label.into(),tab_id:Some(t.id),title:t.zagolovok.clone(),address:t.adres.clone(),sleeping:sleeping||t.otlozhena,index,route_label:match &t.route{crate::tab_routes::RouteChoice::Inherit=>"Путь супервкладки".into(),crate::tab_routes::RouteChoice::Direct=>"Прямое соединение".into(),crate::tab_routes::RouteChoice::Connection{id}=>connections.get(id.as_str()).cloned().unwrap_or_else(||"Подключение недоступно".into()),_=>"Путь недоступен".into()},route:t.route.clone()});
 }
}
#[tauri::command]
pub async fn tab_search_snapshot(s:State<'_,Sostoyanie>)->Result<Vec<Entry>,String>{
 let connections=crate::nastrojki::zagruzit().vyhody.into_iter().map(|v|(v.id,v.nazvanie)).collect::<std::collections::HashMap<_,_>>();
 let definitions={let all=s.vkladki.lock().map_err(|_|"Список занят")?;all.values().filter(|v|!v.nastrojki.privatnaya&&!crate::privatnost::eto_id(&v.nastrojki.id)).map(|v|(v.nastrojki.clone(),v.obychnye.clone(),v.proc.is_none())).collect::<Vec<_>>()};
 let mut out=Vec::new();
 for (n,live,sleeping) in definitions{
  let session=if let Some(live)=live{live}else if !n.pustaya{let id=n.id.clone();tauri::async_runtime::spawn_blocking(move||Sessiya::zagruzit(&crate::brauzer::papka_profilya(&id))).await.map_err(|_|"Не удалось прочитать сессию")?}else{Sessiya::novaya(&n.adres)};
  rows(&mut out,&n.id,&n.nazvanie,"main",&session,sleeping,&connections);
 }
 let names=s.vkladki.lock().map_err(|_|"Список занят")?.iter().map(|(id,v)|(id.clone(),(v.nastrojki.nazvanie.clone(),v.nastrojki.privatnaya))).collect::<std::collections::HashMap<_,_>>();
 {let windows=s.okna.0.lock().map_err(|_|"Список окон занят")?;for(label,w)in windows.iter(){
  if w.privatnaya||w.closing{continue;}
  for(id,session)in std::iter::once((&w.super_id,w.sessiya.as_ref())).chain(w.others.iter().map(|(id,v)|(id,v.sessiya.as_ref()))){
   if let(Some((name,false)),Some(session))=(names.get(id),session){rows(&mut out,id,name,label,session,false,&connections);}
  }
 }}
 for n in crate::zagruzit_sohranennye(){if out.len()>=10_000{break;}if !n.privatnaya&&!crate::privatnost::eto_id(&n.id)&&!names.contains_key(&n.id){out.push(Entry{super_id:n.id,super_name:n.nazvanie.clone(),window:"main".into(),tab_id:None,title:n.nazvanie,address:String::new(),sleeping:true,index:0,route_label:String::new(),route:crate::tab_routes::RouteChoice::Inherit});}}
 out.sort_by(|a,b|(&a.super_name,&a.window,a.index).cmp(&(&b.super_name,&b.window,b.index)));Ok(out)
}
#[tauri::command]
pub async fn tab_search_activate(app:tauri::AppHandle,s:State<'_,Sostoyanie>,entry:Entry)->Result<(),String>{
 if entry.window!="main"&&!s.okna.0.lock().map_err(|_|"Список окон занят")?.get(&entry.window).is_some_and(|w|!w.privatnaya&&!w.closing){return Err("Окно уже закрыто".into());}
 let target=app.get_webview_window(&entry.window).ok_or("Окно уже закрыто")?;
 if entry.tab_id.is_none(){
  let saved=crate::zagruzit_sohranennye().into_iter().find(|n|n.id==entry.super_id).ok_or("Супер-вкладка уже удалена")?;
  if saved.privatnaya||crate::privatnost::eto_id(&saved.id){return Err("Приватная сессия недоступна в общем поиске".into());}
 }
 if entry.tab_id.is_none(){let result=crate::vosstanovit(app.clone(),target.clone(),s.clone(),entry.super_id.clone()).await?;if !result.ok{return Err(result.soobshchenie);}}
 // Revalidate membership before any side effect; no arbitrary profile paths from IPC.
 {let all=s.vkladki.lock().map_err(|_|"Список занят")?;let v=all.get(&entry.super_id).ok_or("Супер-вкладка уже закрыта")?;if v.nastrojki.privatnaya{return Err("Приватная сессия недоступна в общем поиске".into());}}
 let result=crate::pereklyuchit(app.clone(),target.clone(),s.clone(),entry.super_id.clone()).await?;if !result.ok{return Err(result.soobshchenie);}
 if entry.tab_id.is_some(){
  let deadline=std::time::Instant::now()+std::time::Duration::from_secs(25);
  let tab=loop{
   if crate::browser_lock::locked_global(){return Err("Браузер заблокирован".into());}
   let session=if entry.window=="main"{s.vkladki.lock().map_err(|_|"Список занят")?.get(&entry.super_id).and_then(|v|v.obychnye.clone())}else{s.okna.0.lock().map_err(|_|"Список окон занят")?.get(&entry.window).and_then(|w|w.sessiya.clone())};
   if let Some(session)=session{let tab=session.vkladki.get(entry.index).filter(|t|!t.privatnaya&&t.adres==entry.address&&t.route==entry.route).ok_or("Список вкладок изменился. Повторите поиск.")?;break tab.id;}
   if std::time::Instant::now()>=deadline{return Err("Движок не ответил. Повторите поиск.".into());}tokio::time::sleep(std::time::Duration::from_millis(100)).await;
  };
  let cmd=Komanda::Vybrat{id:tab};let cmd=if entry.window=="main"{cmd}else{let ready=crate::okna_app::ready_command(&app,&s,&entry.window,cmd).await?;Komanda::OknoKomanda{label:entry.window.clone(),komanda:Box::new(ready)}};
  crate::okna_app::kanal(&s,&entry.window,&entry.super_id)?.try_send(cmd).map_err(|_|"Движок занят. Повторите поиск.")?;
 }
 target.show().map_err(|e|e.to_string())?;target.set_focus().map_err(|e|e.to_string())?;Ok(())
}
