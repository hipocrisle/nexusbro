//! Состояние текущего выхода. Проверка IP всегда идёт через его локальный прокси.
use serde_json::{json,Value};
#[tauri::command]
pub async fn set_status(s:tauri::State<'_,crate::Sostoyanie>,id:String)->Result<Value,String>{
    let v=s.vkladki.lock().map_err(|e|e.to_string())?;let vk=v.get(&id).ok_or("Супер-вкладка закрыта")?;
    let cfg=&vk.nastrojki;
    let mode=if cfg.proxy.is_empty(){"Напрямую"}else if crate::vpn::eto_podpiska(&cfg.proxy){"VPN / подписка"}else{"Прокси"};
    let node=if !cfg.proxy_uzel.is_empty(){crate::vpn::imya_iz_ssylki(&cfg.proxy_uzel)}else if cfg.proxy.starts_with("vless://"){crate::vpn::imya_iz_ssylki(&cfg.proxy)}else if crate::vpn::eto_podpiska(&cfg.proxy){"Первый узел подписки / конфигурация".into()}else{String::new()};
    Ok(json!({"mode":mode,"node":node,"running":vk.proc.is_some(),"starting":vk.zapusk_idet,"tunnel_running":vk.tunnel.as_ref().is_some_and(|p|crate::okna::sistema::pid_zhiv(p.id())),"proxy_running":vk.nash_proxy.is_some()}))
}
#[tauri::command]
pub async fn set_proverit_ip(s:tauri::State<'_,crate::Sostoyanie>,id:String)->Result<Value,String>{
    let port={let v=s.vkladki.lock().map_err(|e|e.to_string())?;v.get(&id).and_then(|v|v.nash_proxy.as_ref()).map(|p|p.port).ok_or("Сначала откройте супер-вкладку")?};
    let client=reqwest::Client::builder().no_proxy().proxy(reqwest::Proxy::all(format!("http://127.0.0.1:{port}")).map_err(|_|"Ошибка прокси")?).timeout(std::time::Duration::from_secs(8)).build().map_err(|_|"Ошибка проверки")?;
    for url in ["https://api.ipify.org?format=json","https://api64.ipify.org?format=json"]{
        let start=std::time::Instant::now();
        if let Ok(r)=client.get(url).send().await{if r.status().is_success(){if let Ok(text)=r.text().await{if let Ok(v)=serde_json::from_str::<Value>(&text){if let Some(ip)=v["ip"].as_str().and_then(|s|s.parse::<std::net::IpAddr>().ok()){return Ok(json!({"ip":ip.to_string(),"ms":start.elapsed().as_millis(),"checked":crate::biblioteka::vremya()}));}}}}}
    }
    Err("Не удалось проверить IP через выбранный выход. Прямое подключение не использовалось.".into())
}
