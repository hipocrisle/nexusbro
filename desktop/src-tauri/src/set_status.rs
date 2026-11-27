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

// IP and country come from the same request through the confirmed tab route.
fn trace_exit(text: &str) -> Option<(std::net::IpAddr, Option<String>)> {
    if text.len()>4096 {return None;}
    let (mut ip,mut country)=(None,None);
    let (mut saw_ip,mut saw_country)=(false,false);
    for line in text.lines() {
        if let Some(value)=line.strip_prefix("ip=") {
            if saw_ip{return None;} saw_ip=true;ip=Some(value.parse().ok()?);
        } else if let Some(value)=line.strip_prefix("loc=") {
            if saw_country{return None;} saw_country=true;
            if value.len()==2 && value.bytes().all(|b|b.is_ascii_uppercase()) && value!="XX" {country=Some(value.to_owned());}
        }
    }
    Some((ip?,country))
}
async fn small_body(mut response:reqwest::Response)->Option<String>{
    if !response.status().is_success() || response.content_length().is_some_and(|n|n>4096){return None;}
    let mut bytes=Vec::new();
    while let Some(chunk)=response.chunk().await.ok()? {
        if bytes.len()+chunk.len()>4096{return None;} bytes.extend_from_slice(&chunk);
    }
    String::from_utf8(bytes).ok()
}
pub(crate) async fn proverit_ip_port(port:u16)->Result<Value,String>{
    let client=reqwest::Client::builder().no_proxy().proxy(reqwest::Proxy::all(format!("http://127.0.0.1:{port}")).map_err(|_|"Ошибка прокси")?).timeout(std::time::Duration::from_secs(8)).build().map_err(|_|"Ошибка проверки")?;
    let start=std::time::Instant::now();
    if let Ok(response)=client.get("https://www.cloudflare.com/cdn-cgi/trace").timeout(std::time::Duration::from_secs(3)).send().await {
        if let Some(text)=small_body(response).await {
            if let Some((ip,country))=trace_exit(&text){return Ok(json!({"ip":ip.to_string(),"country":country,"ms":start.elapsed().as_millis(),"checked":crate::biblioteka::vremya()}));}
        }
    }
    for url in ["https://api.ipify.org?format=json","https://api64.ipify.org?format=json"]{
        let start=std::time::Instant::now();
        if let Ok(response)=client.get(url).send().await {
            if let Some(text)=small_body(response).await {
                if let Ok(value)=serde_json::from_str::<Value>(&text){
                    if let Some(ip)=value["ip"].as_str().and_then(|v|v.parse::<std::net::IpAddr>().ok()){
                        return Ok(json!({"ip":ip.to_string(),"country":null,"ms":start.elapsed().as_millis(),"checked":crate::biblioteka::vremya()}));
                    }
                }
            }
        }
    }
    Err("Не удалось проверить IP через выбранный выход. Прямое подключение не использовалось.".into())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn trace_pairs_ip_with_country_and_supports_ipv6(){
        let (ip,country)=trace_exit("ip=2001:db8::1\ncolo=FRA\nloc=DE\n").unwrap();
        assert!(ip.is_ipv6());assert_eq!(country.as_deref(),Some("DE"));
    }
    #[test] fn country_is_not_inferred_from_datacenter_or_malformed_codes(){
        for code in ["XX","de","DE-extra",""]{
            assert_eq!(trace_exit(&format!("ip=192.0.2.1\ncolo=FRA\nloc={code}\n")).unwrap().1,None);
        }
    }
    #[test] fn invalid_or_ambiguous_trace_never_supplies_a_confirmed_exit(){
        for text in ["ip=not-an-ip\nloc=DE", "loc=DE", "ip=192.0.2.1\nip=192.0.2.2", "ip=192.0.2.1\nloc=DE\nloc=RU"]{assert!(trace_exit(text).is_none());}
    }
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
