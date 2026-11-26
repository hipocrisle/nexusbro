//! Состояние текущего выхода. Проверка IP всегда идёт через его локальный прокси.
use serde_json::{json,Value};
#[tauri::command]
pub async fn set_status(s:tauri::State<'_,crate::Sostoyanie>,id:String)->Result<Value,String>{
    let (mode,node,running,starting,tunnel,port)={let v=s.vkladki.lock().map_err(|e|e.to_string())?;let vk=v.get(&id).ok_or("Супер-вкладка закрыта")?;
    let cfg=&vk.nastrojki;
    let mode=if cfg.proxy.is_empty(){"Напрямую"}else if crate::vpn::eto_podpiska(&cfg.proxy){"VPN / подписка"}else{"Прокси"};
    let node=if !cfg.proxy_uzel.is_empty(){crate::vpn::imya_iz_ssylki(&cfg.proxy_uzel)}else if cfg.proxy.starts_with("vless://"){crate::vpn::imya_iz_ssylki(&cfg.proxy)}else if crate::vpn::eto_podpiska(&cfg.proxy){"Первый узел подписки / конфигурация".into()}else{String::new()};
    (mode.to_owned(),node,vk.proc.is_some(),vk.zapusk_idet,vk.tunnel.as_ref().is_some_and(|p|crate::okna::sistema::pid_zhiv(p.id())),vk.nash_proxy.as_ref().map(|p|p.port))};
    let reachable=if let Some(port)=port{Some(proverit_marshrut(port).await)}else if starting{None}else{Some(false)};
    Ok(json!({"mode":mode,"node":node,"running":running,"starting":starting,"tunnel_running":tunnel,"proxy_running":port.is_some(),"reachable":reachable}))
}
#[tauri::command]
pub async fn set_proverit_ip(s:tauri::State<'_,crate::Sostoyanie>,id:String)->Result<Value,String>{
    let port={let v=s.vkladki.lock().map_err(|e|e.to_string())?;v.get(&id).and_then(|v|v.nash_proxy.as_ref()).map(|p|p.port).ok_or("Сначала откройте супер-вкладку")?};
    proverit_ip_port(port).await
}

pub(crate) async fn proverit_ip_port(port:u16)->Result<Value,String>{
    let client=reqwest::Client::builder().no_proxy().proxy(reqwest::Proxy::all(format!("http://127.0.0.1:{port}")).map_err(|_|"Ошибка прокси")?).timeout(std::time::Duration::from_secs(8)).build().map_err(|_|"Ошибка проверки")?;
    for url in ["https://api.ipify.org?format=json","https://api64.ipify.org?format=json"]{
        let start=std::time::Instant::now();
        if let Ok(r)=client.get(url).send().await{if r.status().is_success(){if let Ok(text)=r.text().await{if let Ok(v)=serde_json::from_str::<Value>(&text){if let Some(ip)=v["ip"].as_str().and_then(|s|s.parse::<std::net::IpAddr>().ok()){return Ok(json!({"ip":ip.to_string(),"ms":start.elapsed().as_millis(),"checked":crate::biblioteka::vremya()}));}}}}}
    }
    Err("Не удалось проверить IP через выбранный выход. Прямое подключение не использовалось.".into())
}

// Одна проверка на локальный выход за TTL, даже если открыто несколько окон.
async fn proverit_marshrut(port:u16)->bool{
 use std::{collections::HashMap,sync::OnceLock,time::{Instant,Duration}};
 static CACHE:OnceLock<tokio::sync::Mutex<HashMap<u16,(Instant,bool)>>>=OnceLock::new();
 let mut cache=CACHE.get_or_init(Default::default).lock().await;
 if let Some((time,ok))=cache.get(&port){if time.elapsed()<Duration::from_secs(if *ok{30}else{5}){return *ok;}}
 cache.retain(|_,(t,_)|t.elapsed()<Duration::from_secs(120));
 let mut ok=false;
 if let Ok(client)=reqwest::Client::builder().no_proxy().proxy(reqwest::Proxy::all(format!("http://127.0.0.1:{port}")).unwrap()).timeout(Duration::from_secs(3)).build(){
  for url in ["https://www.gstatic.com/generate_204","https://www.cloudflare.com/cdn-cgi/trace"]{
   if let Ok(r)=client.head(url).send().await{if r.status().is_success(){ok=true;break;}}
  }
 }
 cache.insert(port,(Instant::now(),ok));ok
}
