//! Свой прокси — по одному на супер-вкладку.
//!
//! Делает две вещи сразу:
//!   * **режет рекламу** — узел из списка не пускаем вовсе;
//!   * **уводит в нужный выход** — напрямую или через SOCKS5 (наш VPN).
//!
//! ⛔ Почему не расширением: новые правила Chromium урезали блокировщики.
//! В прокси мы не ограничены ничем, и это работает во всех вкладках сразу.
//!
//! ⛔ Содержимое страниц мы НЕ расшифровываем: режем по имени узла на
//! уровне запроса соединения. Поэтому не нужен свой корневой сертификат
//! в системе — а это как раз то, за что ругают другие блокировщики.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::collections::VecDeque;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use crate::spisok_reklamy::SPISOK;

/// Куда уводить трафик вкладки.
#[derive(Clone, Debug, PartialEq)]
pub enum Vyhod {
    /// Напрямую, без посредника.
    Pryamo,
    /// SOCKS5: адрес и, если нужно, вход с паролем.
    Socks5 { adres: String, vhod: Option<(String, String)> },
    /// Обычный веб-прокси (HTTP CONNECT).
    Veb { adres: String, vhod: Option<(String, String)> },
}

/// Разобрать то, что человек написал в поле «выход».
///
/// ⛔ Принимаем в том виде, в каком люди это копируют: со схемой и без,
/// с именем и паролем внутри ссылки.
pub fn razobrat_vyhod(s: &str) -> anyhow::Result<Vyhod> {
    let s = s.trim();
    if s.is_empty() {
        return Ok(Vyhod::Pryamo);
    }
    let (shema, ostatok) = match s.split_once("://") {
        Some((sh, o)) => (sh.to_ascii_lowercase(), o),
        // без схемы считаем, что это SOCKS5 — так пишут чаще всего
        None => ("socks5".to_string(), s),
    };
    // вход с паролем: user:pass@host:port
    let (vhod, adres) = match ostatok.rsplit_once('@') {
        Some((uchetka, a)) => {
            let (u, p) = uchetka.split_once(':').unwrap_or((uchetka, ""));
            (Some((u.to_string(), p.to_string())), a.to_string())
        }
        None => (None, ostatok.to_string()),
    };
    let adres = adres.trim_end_matches('/').to_string();
    anyhow::ensure!(adres.contains(':'), "в адресе «{}» нет порта", adres);
    match shema.as_str() {
        "socks5" | "socks5h" | "socks" => Ok(Vyhod::Socks5 { adres, vhod }),
        "http" | "https" => Ok(Vyhod::Veb { adres, vhod }),
        inoe => anyhow::bail!("неизвестный вид выхода «{}»", inoe),
    }
}

#[derive(Clone)]
pub struct Nastrojka {
    pub vyhod: Vyhod,
    pub rezat_reklamu: bool,
}

#[derive(Clone, serde::Serialize)]
pub struct Blokirovka { pub domen: String, pub pravilo: String, pub metod: String, pub vremya_ms: u64 }
#[derive(serde::Serialize)]
pub struct ZhurnalRezki { pub vklyuchena: bool, pub zarezano: u64, pub propushcheno: u64, pub zapisi: Vec<Blokirovka> }
#[derive(Default)]
pub struct Schet {
    pub propushcheno: AtomicU64,
    pub zarezano: AtomicU64,
    pub zhurnal: Mutex<VecDeque<Blokirovka>>,
}
impl Schet {
    pub fn snimok(&self, vklyuchena: bool) -> ZhurnalRezki {
        ZhurnalRezki { vklyuchena, zarezano: self.zarezano.load(Ordering::Relaxed), propushcheno: self.propushcheno.load(Ordering::Relaxed),
            zapisi: self.zhurnal.lock().map(|v| v.iter().cloned().collect()).unwrap_or_default() }
    }
    fn blok(&self, host: &str, pravilo: &str, metod: &str) {
        self.zarezano.fetch_add(1, Ordering::Relaxed);
        if let Ok(mut v) = self.zhurnal.lock() {
            v.push_front(Blokirovka { domen: host.trim_end_matches('.').to_ascii_lowercase(), pravilo: pravilo.into(), metod: metod.into(),
                vremya_ms: std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as u64 });
            v.truncate(200);
        }
    }
}

pub struct Rabotayushchij {
    pub port: u16,
    pub schet: Arc<Schet>,
    ostanovit: tokio::sync::oneshot::Sender<()>,
}

impl Rabotayushchij {
    pub fn ostanovit(self) {
        let _ = self.ostanovit.send(());
    }
}

/// Поднять прокси на свободном порту. Возвращает порт, на который
/// натравливаем движок через `--proxy-server`.
pub async fn podnyat(n: Nastrojka) -> anyhow::Result<Rabotayushchij> {
    // Порт 0 = «дай любой свободный»: так две супер-вкладки не подерутся.
    let slushatel = TcpListener::bind("127.0.0.1:0").await?;
    let port = slushatel.local_addr()?.port();
    let schet = Arc::new(Schet::default());
    let (tx, mut rx) = tokio::sync::oneshot::channel();
    let s2 = schet.clone();

    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = &mut rx => break,
                prinyato = slushatel.accept() => {
                    let (potok, _) = match prinyato { Ok(v) => v, Err(_) => continue };
                    let n = n.clone();
                    let s = s2.clone();
                    tokio::spawn(async move {
                        let _ = obsluzhit(potok, n, s).await;
                    });
                }
            }
        }
    });

    Ok(Rabotayushchij { port, schet, ostanovit: tx })
}

fn pravilo_reklamy(host: &str) -> Option<&'static str> {
    let h = host.trim_end_matches('.').to_ascii_lowercase();
    SPISOK.iter().copied().find(|p| h == *p || h.ends_with(&format!(".{p}")))
}
#[cfg(test)]
fn reklama(host: &str) -> bool { pravilo_reklamy(host).is_some() }

async fn obsluzhit(mut klient: TcpStream, n: Nastrojka, s: Arc<Schet>) -> anyhow::Result<()> {
    let mut bufer = vec![0u8; 8192];
    let prochli = klient.read(&mut bufer).await?;
    if prochli == 0 {
        return Ok(());
    }
    let zagolovok = String::from_utf8_lossy(&bufer[..prochli]).to_string();
    let pervaya = zagolovok.lines().next().unwrap_or("").to_string();
    let mut chasti = pervaya.split_whitespace();
    let metod = chasti.next().unwrap_or("");
    let cel = chasti.next().unwrap_or("");

    // Куда человек хочет попасть
    let (host, port) = if metod.eq_ignore_ascii_case("CONNECT") {
        razobrat_adres(cel, 443)
    } else {
        // обычный http: адрес в строке запроса или в заголовке Host
        let bez_shemy = cel.strip_prefix("http://").unwrap_or(cel);
        let hvost = bez_shemy.split('/').next().unwrap_or("");
        if hvost.is_empty() {
            let h = zagolovok
                .lines()
                .find(|l| l.to_ascii_lowercase().starts_with("host:"))
                .map(|l| l[5..].trim().to_string())
                .unwrap_or_default();
            razobrat_adres(&h, 80)
        } else {
            razobrat_adres(hvost, 80)
        }
    };
    if host.is_empty() {
        return Ok(());
    }

    if let Some(pravilo) = pravilo_reklamy(&host).filter(|_| n.rezat_reklamu) {
        s.blok(&host, pravilo, metod);
        // Отвечаем коротко и вежливо: страница не ждёт впустую.
        let _ = klient
            .write_all(b"HTTP/1.1 403 Blocked\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .await;
        return Ok(());
    }
    s.propushcheno.fetch_add(1, Ordering::Relaxed);

    let mut naruzhu = match &n.vyhod {
        Vyhod::Pryamo => TcpStream::connect((host.as_str(), port)).await?,
        Vyhod::Socks5 { adres, vhod } => cherez_socks5(adres, vhod, &host, port).await?,
        Vyhod::Veb { adres, vhod } => cherez_veb(adres, vhod, &host, port).await?,
    };

    if metod.eq_ignore_ascii_case("CONNECT") {
        klient.write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n").await?;
    } else {
        // обычный http — отдаём наружу то, что уже прочитали
        naruzhu.write_all(&bufer[..prochli]).await?;
    }
    let _ = tokio::io::copy_bidirectional(&mut klient, &mut naruzhu).await;
    Ok(())
}

fn razobrat_adres(s: &str, po_umolchaniyu: u16) -> (String, u16) {
    let s = s.trim();
    if let Some(ostatok) = s.strip_prefix('[') {
        // адрес вида [::1]:443
        if let Some((a, b)) = ostatok.split_once(']') {
            let port = b.trim_start_matches(':').parse().unwrap_or(po_umolchaniyu);
            return (a.to_string(), port);
        }
    }
    match s.rsplit_once(':') {
        Some((h, p)) if p.chars().all(|c| c.is_ascii_digit()) && !p.is_empty() => {
            (h.to_string(), p.parse().unwrap_or(po_umolchaniyu))
        }
        _ => (s.to_string(), po_umolchaniyu),
    }
}

/// Выход через SOCKS5 (наш VPN). Имя узла отдаём ЕМУ, а не разрешаем сами:
/// иначе запрос имени уйдёт мимо туннеля и выдаст человека.
async fn cherez_socks5(
    adres: &str,
    vhod: &Option<(String, String)>,
    host: &str,
    port: u16,
) -> anyhow::Result<TcpStream> {
    let mut p = TcpStream::connect(adres).await?;
    // Предлагаем «без пароля» и, если он задан, «с паролем».
    if vhod.is_some() {
        p.write_all(&[0x05, 0x02, 0x00, 0x02]).await?;
    } else {
        p.write_all(&[0x05, 0x01, 0x00]).await?;
    }
    let mut otvet = [0u8; 2];
    p.read_exact(&mut otvet).await?;
    anyhow::ensure!(otvet[0] == 5, "это не SOCKS5");
    if otvet[1] == 0x02 {
        let (u, pw) = vhod.clone().ok_or_else(|| anyhow::anyhow!("выход просит имя и пароль"))?;
        let mut z = vec![0x01, u.len() as u8];
        z.extend_from_slice(u.as_bytes());
        z.push(pw.len() as u8);
        z.extend_from_slice(pw.as_bytes());
        p.write_all(&z).await?;
        let mut o = [0u8; 2];
        p.read_exact(&mut o).await?;
        anyhow::ensure!(o[1] == 0, "выход не принял имя и пароль");
    } else {
        anyhow::ensure!(otvet[1] == 0, "выход требует способ входа, которого у нас нет");
    }

    let imya = host.as_bytes();
    anyhow::ensure!(imya.len() < 256, "слишком длинное имя узла");
    let mut zapros = vec![0x05, 0x01, 0x00, 0x03, imya.len() as u8];
    zapros.extend_from_slice(imya);
    zapros.extend_from_slice(&port.to_be_bytes());
    p.write_all(&zapros).await?;

    let mut golova = [0u8; 4];
    p.read_exact(&mut golova).await?;
    anyhow::ensure!(golova[1] == 0, "SOCKS5 отказал, код {}", golova[1]);
    match golova[3] {
        0x01 => { let mut b = [0u8; 6]; p.read_exact(&mut b).await?; }
        0x04 => { let mut b = [0u8; 18]; p.read_exact(&mut b).await?; }
        0x03 => {
            let mut dlina = [0u8; 1];
            p.read_exact(&mut dlina).await?;
            let mut b = vec![0u8; dlina[0] as usize + 2];
            p.read_exact(&mut b).await?;
        }
        _ => anyhow::bail!("непонятный ответ SOCKS5"),
    }
    Ok(p)
}

/// Выход через обычный веб-прокси: просим его соединить нас с узлом.
/// ⛔ Имя узла тоже отдаём ЕМУ, а не разрешаем сами.
async fn cherez_veb(
    adres: &str,
    vhod: &Option<(String, String)>,
    host: &str,
    port: u16,
) -> anyhow::Result<TcpStream> {
    let mut p = TcpStream::connect(adres).await?;
    let mut zapros = format!(
        "CONNECT {h}:{po} HTTP/1.1\r\nHost: {h}:{po}\r\n",
        h = host,
        po = port
    );
    if let Some((u, pw)) = vhod {
        let para = obychnyj_base64(format!("{}:{}", u, pw).as_bytes());
        zapros.push_str(&format!("Proxy-Authorization: Basic {}\r\n", para));
    }
    zapros.push_str("Proxy-Connection: Keep-Alive\r\n\r\n");
    p.write_all(zapros.as_bytes()).await?;

    // Читаем ответ до пустой строки
    let mut nakopleno = Vec::new();
    let mut b = [0u8; 1024];
    loop {
        let n = p.read(&mut b).await?;
        anyhow::ensure!(n > 0, "веб-прокси закрыл соединение молча");
        nakopleno.extend_from_slice(&b[..n]);
        if nakopleno.windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
        anyhow::ensure!(nakopleno.len() < 16384, "веб-прокси отвечает ерундой");
    }
    let shapka = String::from_utf8_lossy(&nakopleno);
    let pervaya = shapka.lines().next().unwrap_or("");
    anyhow::ensure!(
        pervaya.contains(" 200"),
        "веб-прокси отказал: {}",
        pervaya
    );
    Ok(p)
}

/// Короткая своя реализация base64 — тащить ради этого целую библиотеку незачем.
fn obychnyj_base64(d: &[u8]) -> String {
    const A: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut r = String::new();
    for k in d.chunks(3) {
        let b = [k[0], *k.get(1).unwrap_or(&0), *k.get(2).unwrap_or(&0)];
        let v = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        r.push(A[(v >> 18 & 63) as usize] as char);
        r.push(A[(v >> 12 & 63) as usize] as char);
        r.push(if k.len() > 1 { A[(v >> 6 & 63) as usize] as char } else { '=' });
        r.push(if k.len() > 2 { A[(v & 63) as usize] as char } else { '=' });
    }
    r
}

#[cfg(test)]
mod proverki {
    use super::*;

    async fn sprosit(port: u16, cel: &str) -> String {
        let mut p = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        let zapros = format!("CONNECT {} HTTP/1.1\r\nHost: {}\r\n\r\n", cel, cel);
        p.write_all(zapros.as_bytes()).await.unwrap();
        let mut b = vec![0u8; 256];
        let n = tokio::time::timeout(std::time::Duration::from_secs(10), p.read(&mut b))
            .await
            .unwrap_or(Ok(0))
            .unwrap_or(0);
        String::from_utf8_lossy(&b[..n]).lines().next().unwrap_or("").to_string()
    }

    #[tokio::test]
    async fn reklamu_rezhem_obychnoe_puskaem() {
        let r = podnyat(Nastrojka { vyhod: Vyhod::Pryamo, rezat_reklamu: true })
            .await
            .unwrap();
        // рекламный узел — не пускаем
        assert!(sprosit(r.port, "doubleclick.net:443").await.contains("403"));
        // и его поддомен тоже
        assert!(sprosit(r.port, "stats.doubleclick.net:443").await.contains("403"));
        // обычный узел — пускаем
        let ok = sprosit(r.port, "example.com:443").await;
        assert!(ok.contains("200"), "обычный узел не прошёл: {}", ok);
        assert_eq!(r.schet.zarezano.load(Ordering::Relaxed), 2);
    }

    #[tokio::test]
    async fn pohozhee_imya_ne_lovim() {
        // ⛔ Главная опасность списка: поймать лишнее и сломать сайт.
        // `notdoubleclick.net` НЕ должен попасть под правило.
        let r = podnyat(Nastrojka { vyhod: Vyhod::Pryamo, rezat_reklamu: true })
            .await
            .unwrap();
        assert!(!reklama("notdoubleclick.net"));
        assert!(!reklama("mydoubleclick.net.example.org"));
        assert!(reklama("DoubleClick.NET"));
        let _ = r;
    }

    #[test]
    fn razbiraem_lyubuyu_zapis_vyhoda() {
        use Vyhod::*;
        assert_eq!(razobrat_vyhod("").unwrap(), Pryamo);
        // без схемы — считаем SOCKS5, так пишут чаще всего
        assert_eq!(
            razobrat_vyhod("127.0.0.1:1080").unwrap(),
            Socks5 { adres: "127.0.0.1:1080".into(), vhod: None }
        );
        assert_eq!(
            razobrat_vyhod("socks5://1.2.3.4:1080").unwrap(),
            Socks5 { adres: "1.2.3.4:1080".into(), vhod: None }
        );
        assert_eq!(
            razobrat_vyhod("socks5://vasya:tajna@1.2.3.4:1080").unwrap(),
            Socks5 {
                adres: "1.2.3.4:1080".into(),
                vhod: Some(("vasya".into(), "tajna".into()))
            }
        );
        assert_eq!(
            razobrat_vyhod("http://proxy.firma.ru:3128").unwrap(),
            Veb { adres: "proxy.firma.ru:3128".into(), vhod: None }
        );
        assert_eq!(
            razobrat_vyhod("http://u:p@proxy.firma.ru:3128/").unwrap(),
            Veb { adres: "proxy.firma.ru:3128".into(), vhod: Some(("u".into(), "p".into())) }
        );
        // без порта — внятная ошибка, а не падение
        assert!(razobrat_vyhod("socks5://1.2.3.4").is_err());
        assert!(razobrat_vyhod("ftp://1.2.3.4:21").is_err());
    }

    #[test]
    fn base64_kak_polozheno() {
        assert_eq!(obychnyj_base64(b"a:b"), "YTpi");
        assert_eq!(obychnyj_base64(b"vasya:tajna"), "dmFzeWE6dGFqbmE=");
        assert_eq!(obychnyj_base64(b"ab"), "YWI=");
    }

    #[tokio::test]
    async fn bez_rezki_puskaem_vsyo() {
        let r = podnyat(Nastrojka { vyhod: Vyhod::Pryamo, rezat_reklamu: false })
            .await
            .unwrap();
        let otvet = sprosit(r.port, "doubleclick.net:443").await;
        assert!(!otvet.contains("403"), "резка выключена, а узел зарезан");
    }
}

#[cfg(test)]
mod journal_tests {
    use super::*;
    #[test]
    fn zhurnal_ogranichen_i_soderzhit_pravilo() {
        let s=Schet::default();
        for _ in 0..205 { s.blok("AD.DoubleClick.NET.","doubleclick.net","CONNECT"); }
        let n=s.snimok(true); assert_eq!(n.zarezano,205); assert_eq!(n.zapisi.len(),200);
        assert_eq!(n.zapisi[0].domen,"ad.doubleclick.net"); assert_eq!(n.zapisi[0].pravilo,"doubleclick.net");
        assert!(n.zapisi[0].vremya_ms>0);
        assert_eq!(pravilo_reklamy("ad.doubleclick.net"),Some("doubleclick.net"));
        assert_eq!(pravilo_reklamy("notdoubleclick.net"),None);
    }
}
