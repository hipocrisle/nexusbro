//! Подписки и ссылки на узлы: поднимаем для вкладки свой xray.
//!
//! ⛔ Внутри программы лежит только ДВИЖОК (xray). Никаких наших узлов,
//! ключей и подписок в ней нет и быть не должно: человек вставляет своё.
//!
//! Работает так: разобрали ссылку → собрали настройку xray с местным
//! SOCKS5 на свободном порту → запустили → наш прокси ходит в этот порт.

use serde_json::{json, Value};
use std::collections::HashMap;
use std::process::Child;

#[derive(Debug, Clone, PartialEq)]
pub struct Uzel {
    pub imya: String,
    pub ssylka: String,
}

/// Похоже ли это на ссылку узла или подписки.
pub fn eto_podpiska(s: &str) -> bool {
    let s = s.trim().to_ascii_lowercase();
    s.starts_with("vless://")
        || s.starts_with("vmess://")
        || s.starts_with("trojan://")
        || s.starts_with("ss://")
        // ссылка подписки: обычный http(s), но не веб-прокси —
        // у веб-прокси в адресе есть порт и нет пути
        || ((s.starts_with("http://") || s.starts_with("https://")) && s.contains('/') && {
            let hvost = s.splitn(4, '/').nth(3).unwrap_or("");
            !hvost.is_empty()
        })
}

/// Достать список узлов из того, что отдала подписка.
///
/// ⛔ Форматов три, и их надо различать: обычный список строк,
/// тот же список в base64 и JSON-массив (так отдаёт Happ).
pub fn razobrat_podpisku(telo: &str) -> Vec<Uzel> {
    let telo = telo.trim();
    // JSON-массив
    if telo.starts_with('[') {
        if let Ok(Value::Array(a)) = serde_json::from_str::<Value>(telo) {
            let mut r = Vec::new();
            for e in a {
                // Happ кладёт имя в remarks, ссылку в разные поля
                let imya = e.get("remarks").and_then(|v| v.as_str()).unwrap_or("узел");
                for klyuch in ["link", "url", "uri", "config"] {
                    if let Some(s) = e.get(klyuch).and_then(|v| v.as_str()) {
                        r.push(Uzel { imya: imya.to_string(), ssylka: s.to_string() });
                        break;
                    }
                }
            }
            if !r.is_empty() {
                return r;
            }
        }
    }
    // base64 — раскрываем и разбираем как список
    let raskrytoe = raskryt_base64(telo);
    let istochnik = raskrytoe.as_deref().unwrap_or(telo);
    istochnik
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty() && l.contains("://"))
        .map(|l| Uzel { imya: imya_iz_ssylki(l), ssylka: l.to_string() })
        .collect()
}

fn imya_iz_ssylki(s: &str) -> String {
    match s.split_once('#') {
        Some((_, hvost)) => raskodirovat(hvost),
        None => "узел".to_string(),
    }
}

fn raskryt_base64(s: &str) -> Option<String> {
    const A: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let chistyj_0: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    let chistyj = chistyj_0.replace('-', "+").replace('_', "/");
    let bez = chistyj.trim_end_matches('=');
    if bez.is_empty() || !bez.bytes().all(|b| A.contains(&b)) {
        return None;
    }
    let mut bity = 0u32;
    let mut skolko = 0u8;
    let mut vyhod = Vec::new();
    for b in bez.bytes() {
        let z = A.iter().position(|x| *x == b)? as u32;
        bity = (bity << 6) | z;
        skolko += 6;
        if skolko >= 8 {
            skolko -= 8;
            vyhod.push((bity >> skolko) as u8);
        }
    }
    String::from_utf8(vyhod).ok().filter(|t| t.contains("://"))
}

fn raskodirovat(s: &str) -> String {
    let b = s.as_bytes();
    let mut r = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                r.push(v);
                i += 3;
                continue;
            }
        }
        r.push(if b[i] == b'+' { b' ' } else { b[i] });
        i += 1;
    }
    String::from_utf8_lossy(&r).to_string()
}

/// Собрать настройку xray: местный SOCKS5 → заданный узел.
pub fn nastrojka_xray(ssylka: &str, port_socks: u16) -> anyhow::Result<String> {
    let vyhod = vyhod_iz_ssylki(ssylka)?;
    let nastrojka = json!({
        "log": { "loglevel": "warning" },
        "inbounds": [{
            "listen": "127.0.0.1",
            "port": port_socks,
            "protocol": "socks",
            // ⛔ UDP через SOCKS5 не пускаем: он всё равно не ходит,
            // а видео из-за этого валится вместо перехода на TCP.
            "settings": { "udp": false, "auth": "noauth" }
        }],
        "outbounds": [vyhod, { "protocol": "freedom", "tag": "pryamo" }]
    });
    Ok(serde_json::to_string_pretty(&nastrojka)?)
}

fn vyhod_iz_ssylki(ssylka: &str) -> anyhow::Result<Value> {
    let s = ssylka.trim();
    anyhow::ensure!(s.starts_with("vless://"), "пока умеем только vless://");
    let telo = &s[8..];
    let telo = telo.split('#').next().unwrap_or(telo);
    let (uchetka, ostatok) = telo
        .split_once('@')
        .ok_or_else(|| anyhow::anyhow!("в ссылке нет знака @"))?;
    let (adres, zapros) = match ostatok.split_once('?') {
        Some((a, z)) => (a, z),
        None => (ostatok, ""),
    };
    let (host, port) = adres
        .rsplit_once(':')
        .ok_or_else(|| anyhow::anyhow!("в ссылке нет порта"))?;
    let port: u16 = port.parse()?;

    let mut p: HashMap<&str, String> = HashMap::new();
    for para in zapros.split('&').filter(|x| !x.is_empty()) {
        if let Some((k, v)) = para.split_once('=') {
            p.insert(k, raskodirovat(v));
        }
    }
    let vzyat = |k: &str, po_umolchaniyu: &str| -> String {
        p.get(k).cloned().unwrap_or_else(|| po_umolchaniyu.to_string())
    };

    let set = vzyat("type", "tcp");
    let zashchita = vzyat("security", "none");
    let mut potok = json!({ "network": set, "security": zashchita });

    if zashchita == "reality" {
        potok["realitySettings"] = json!({
            "serverName": vzyat("sni", host),
            // ⛔ Отпечаток chrome в РФ душат — по умолчанию firefox.
            "fingerprint": vzyat("fp", "firefox"),
            "publicKey": vzyat("pbk", ""),
            "shortId": vzyat("sid", ""),
            "spiderX": vzyat("spx", "/"),
        });
    } else if zashchita == "tls" {
        potok["tlsSettings"] = json!({
            "serverName": vzyat("sni", host),
            "fingerprint": vzyat("fp", "firefox"),
            "allowInsecure": false,
        });
    }
    match set.as_str() {
        "ws" => {
            potok["wsSettings"] = json!({
                "path": vzyat("path", "/"),
                "headers": { "Host": vzyat("host", host) }
            })
        }
        "grpc" => potok["grpcSettings"] = json!({ "serviceName": vzyat("serviceName", "") }),
        "xhttp" | "splithttp" => {
            potok["xhttpSettings"] = json!({
                "path": vzyat("path", "/"),
                "host": vzyat("host", host),
                "mode": vzyat("mode", "auto"),
            })
        }
        _ => {}
    }

    Ok(json!({
        "tag": "vyhod",
        "protocol": "vless",
        "settings": { "vnext": [{
            "address": host,
            "port": port,
            "users": [{
                "id": uchetka,
                "encryption": "none",
                "flow": vzyat("flow", ""),
            }]
        }]},
        "streamSettings": potok
        // ⛔ mux здесь НЕ включаем: на наших схемах он только мешает.
    }))
}

/// Скачать подписку.
pub async fn skachat_podpisku(url: &str) -> anyhow::Result<Vec<Uzel>> {
    let otvet = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(25))
        .user_agent("NexusBro")
        .build()?
        .get(url)
        .send()
        .await?;
    anyhow::ensure!(otvet.status().is_success(), "подписка ответила {}", otvet.status());
    let telo = otvet.text().await?;
    let uzly = razobrat_podpisku(&telo);
    anyhow::ensure!(!uzly.is_empty(), "в подписке не нашлось ни одного узла");
    Ok(uzly)
}

#[cfg(test)]
mod proverki {
    use super::*;

    #[test]
    fn otlichaem_podpisku_ot_veb_proksi() {
        assert!(eto_podpiska("vless://abc@1.2.3.4:443?security=reality"));
        assert!(eto_podpiska("https://moy.server.ru/sub/abcdef"));
        // ⛔ Веб-прокси подпиской считать НЕЛЬЗЯ — он пишется без пути.
        assert!(!eto_podpiska("http://proxy.firma.ru:3128"));
        assert!(!eto_podpiska("socks5://1.2.3.4:1080"));
        assert!(!eto_podpiska(""));
    }

    #[test]
    fn razbiraem_spisok_strokami_i_v_base64() {
        let prostoj = "vless://a@1.1.1.1:443#Первый\nvless://b@2.2.2.2:443#Второй";
        let u = razobrat_podpisku(prostoj);
        assert_eq!(u.len(), 2);
        assert_eq!(u[0].imya, "Первый");

        // та же строка, но в base64 — так отдаёт большинство подписок
        let v_base64 = "dmxlc3M6Ly9hQDEuMS4xLjE6NDQzI9Cf0LXRgNCy0YvQuQ==";
        let u = razobrat_podpisku(v_base64);
        assert_eq!(u.len(), 1, "base64 не раскрылся");
        assert!(u[0].ssylka.starts_with("vless://a@"));
    }

    #[test]
    fn razbiraem_json_kak_u_happ() {
        // ⛔ Happ отдаёт ПРОСТОЙ JSON-массив, не base64. Имя в remarks.
        let j = r#"[{"remarks":"DE-1","link":"vless://x@3.3.3.3:443"},
                    {"remarks":"FR-1","link":"vless://y@4.4.4.4:443"}]"#;
        let u = razobrat_podpisku(j);
        assert_eq!(u.len(), 2);
        assert_eq!(u[1].imya, "FR-1");
        assert!(u[1].ssylka.contains("4.4.4.4"));
    }

    #[test]
    fn sobiraem_nastrojku_dlya_reality() {
        let s = "vless://11111111-2222-3333-4444-555555555555@1.2.3.4:443\
                 ?security=reality&sni=www.example.com&pbk=KLYUCH&sid=ab12\
                 &flow=xtls-rprx-vision&type=tcp#Узел";
        let n = nastrojka_xray(s, 10808).unwrap();
        let v: serde_json::Value = serde_json::from_str(&n).unwrap();
        assert_eq!(v["inbounds"][0]["port"], 10808);
        assert_eq!(v["inbounds"][0]["settings"]["udp"], false);
        let o = &v["outbounds"][0];
        assert_eq!(o["protocol"], "vless");
        assert_eq!(o["settings"]["vnext"][0]["address"], "1.2.3.4");
        assert_eq!(o["settings"]["vnext"][0]["users"][0]["flow"], "xtls-rprx-vision");
        let r = &o["streamSettings"]["realitySettings"];
        assert_eq!(r["serverName"], "www.example.com");
        assert_eq!(r["publicKey"], "KLYUCH");
        // ⛔ отпечаток chrome в РФ душат — по умолчанию должен быть firefox
        assert_eq!(r["fingerprint"], "firefox");
        // ⛔ mux не должен появиться
        assert!(o.get("mux").is_none());
    }

    #[test]
    fn ponyatnaya_oshibka_na_chuzhom_vide() {
        assert!(nastrojka_xray("vmess://чтототам", 1080).is_err());
        assert!(nastrojka_xray("vless://bez-adresa", 1080).is_err());
    }
}


/// Поднять туннель для вкладки: записать настройку, запустить xray,
/// дождаться, пока он начнёт слушать. Возвращает процесс и порт SOCKS5.
pub async fn podnyat_tunnel(
    papka: &std::path::Path,
    ssylka: &str,
) -> anyhow::Result<(Child, u16)> {
    let xray = crate::brauzer::najti_xray().ok_or_else(|| {
        anyhow::anyhow!("xray не найден: положи его рядом с программой")
    })?;
    // Свободный порт берём у системы и сразу отпускаем.
    let port = {
        let s = std::net::TcpListener::bind("127.0.0.1:0")?;
        s.local_addr()?.port()
    };
    let nastrojka = nastrojka_xray(ssylka, port)?;
    std::fs::create_dir_all(papka)?;
    let fajl = papka.join("xray.json");
    std::fs::write(&fajl, nastrojka)?;

    let ditya = crate::brauzer::zapustit_sputnik(
        &xray,
        &["run".into(), "-c".into(), fajl.display().to_string()],
    )?;

    // ⛔ Ждём, пока туннель ОТКРОЕТ порт. Без ожидания движок стартует
    // первым, утыкается в закрытую дверь и показывает ошибку сети.
    let srok = std::time::Instant::now();
    while srok.elapsed().as_secs() < 12 {
        if tokio::net::TcpStream::connect(("127.0.0.1", port)).await.is_ok() {
            return Ok((ditya, port));
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
    Err(anyhow::anyhow!("туннель не поднялся за 12 секунд"))
}

#[cfg(test)]
mod proverka_tunnelya {
    use super::*;

    /// ⛔ Проверяем САМУ связку: настройка → запуск xray → открытый порт.
    /// Узел нарочно несуществующий: нам важно, что туннель поднялся и
    /// слушает, а не что он куда-то дошёл.
    #[tokio::test]
    async fn tunnel_podnimaetsya_i_slushaet() {
        if crate::brauzer::najti_xray().is_none() {
            eprintln!("xray не найден — проверку пропускаю");
            return;
        }
        let papka = std::env::temp_dir().join("nexusbro_proba_tunnel");
        let _ = std::fs::remove_dir_all(&papka);
        let ssylka = "vless://11111111-2222-3333-4444-555555555555@198.51.100.7:443\
                      ?security=reality&sni=www.example.com&pbk=AAAA&sid=ab&type=tcp#Проба";
        let (mut ditya, port) = podnyat_tunnel(&papka, ssylka)
            .await
            .expect("туннель должен подняться");
        assert!(port > 1024);
        // порт действительно принимает соединения
        assert!(tokio::net::TcpStream::connect(("127.0.0.1", port)).await.is_ok());
        let _ = ditya.kill();
        let _ = ditya.wait();
        let _ = std::fs::remove_dir_all(&papka);
    }
}
