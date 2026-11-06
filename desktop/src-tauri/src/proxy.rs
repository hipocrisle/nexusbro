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
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use crate::spisok_reklamy::SPISOK;

#[derive(Clone)]
pub struct Nastrojka {
    /// Куда уводить: `None` — напрямую, иначе адрес SOCKS5.
    pub vyhod: Option<String>,
    pub rezat_reklamu: bool,
}

#[derive(Default)]
pub struct Schet {
    pub propushcheno: AtomicU64,
    pub zarezano: AtomicU64,
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

fn reklama(host: &str) -> bool {
    let h = host.trim_end_matches('.').to_ascii_lowercase();
    SPISOK.iter().any(|p| {
        let p = *p;
        // ⛔ Совпадение по ХВОСТУ, а не по вхождению: иначе `notads.ru`
        // поймается правилом `ads.ru` и сайт сломается на ровном месте.
        h == p || h.ends_with(&format!(".{}", p))
    })
}

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

    if n.rezat_reklamu && reklama(&host) {
        s.zarezano.fetch_add(1, Ordering::Relaxed);
        // Отвечаем коротко и вежливо: страница не ждёт впустую.
        let _ = klient
            .write_all(b"HTTP/1.1 403 Blocked\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .await;
        return Ok(());
    }
    s.propushcheno.fetch_add(1, Ordering::Relaxed);

    let mut naruzhu = match &n.vyhod {
        None => TcpStream::connect((host.as_str(), port)).await?,
        Some(adres) => cherez_socks5(adres, &host, port).await?,
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
async fn cherez_socks5(adres: &str, host: &str, port: u16) -> anyhow::Result<TcpStream> {
    let chistyj = adres
        .trim_start_matches("socks5h://")
        .trim_start_matches("socks5://");
    let addr: SocketAddr = chistyj.parse()?;
    let mut p = TcpStream::connect(addr).await?;
    p.write_all(&[0x05, 0x01, 0x00]).await?;
    let mut otvet = [0u8; 2];
    p.read_exact(&mut otvet).await?;
    anyhow::ensure!(otvet[0] == 5 && otvet[1] == 0, "SOCKS5 не принял соединение");

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
        let r = podnyat(Nastrojka { vyhod: None, rezat_reklamu: true })
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
        let r = podnyat(Nastrojka { vyhod: None, rezat_reklamu: true })
            .await
            .unwrap();
        assert!(!reklama("notdoubleclick.net"));
        assert!(!reklama("mydoubleclick.net.example.org"));
        assert!(reklama("DoubleClick.NET"));
        let _ = r;
    }

    #[tokio::test]
    async fn bez_rezki_puskaem_vsyo() {
        let r = podnyat(Nastrojka { vyhod: None, rezat_reklamu: false })
            .await
            .unwrap();
        let otvet = sprosit(r.port, "doubleclick.net:443").await;
        assert!(!otvet.contains("403"), "резка выключена, а узел зарезан");
    }
}
