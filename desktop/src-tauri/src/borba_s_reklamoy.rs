//! Вторая половина защиты от рекламы: то, что прокси НЕ видит.
//!
//! Прокси (`proxy.rs`) режет по имени узла — это основной и самый честный
//! механизм. Но у него два слепых пятна, из-за которых человек и продолжал
//! видеть рекламу (17.11.2026):
//!
//! 1. **Путь запроса.** HTTPS прокси не расшифровывает: `yabs.yandex.ru/promo`
//!    и `site.ru/ads/...` для него — безобидный адрес. Здесь мы смотрим
//!    полный адрес ДО загрузки (`RequestHandler` → `resource_request_handler`)
//!    и отменяем вызов.
//! 2. **Разметка на «чистом» домене.** Плашка «Скачайте Firefox», «Загрузите
//!    Chrome», «Сделать Яндекс поиском по умолчанию» — это обычный HTML/CSS
//!    yandex.ru и google.com. Никакой блокировщик по имени узла её не уберёт:
//!    её убирает скрипт в самой странице.
//!
//! ⛔ Главная опасность — сломать сайт. Поэтому:
//! * режем по пути/параметрам только явно рекламные образцы и никогда —
//!   документ верхней навигации (иначе поиск Яндекса не откроется);
//! * скрипт удаляет элементы ТОЛЬКО вместе с рекламным текстом и только
//!   небольшие (не страницу и не ленту), плюс запасное правило по классу.

use crate::spisok_reklamy::{reklamnyj_host, PUTI, ZAPRESHCHENNYE_PARAMETRY};
use cef::*;

/// Нужно ли отменить этот вызов. `None` = ресурс не наш, пусть идёт.
pub fn zarezannyj_resurs(adres: &str, navigaciya: bool) -> Option<&'static str> {
    let nizhnij = adres.to_ascii_lowercase();
    if navigaciya {
        return None;
    }
    let (host, hvost) = razdelit(&nizhnij)?;
    if let Some(domen) = reklamnyj_host(&host) {
        if domen == host {
            return Some("домен");
        }
        // Домен-адware-сеть (например `yabs.yandex.ru`) режем, только если
        // адрес похож на рекламный. ⛔ Сам `yandex.ru` от этого не страдает:
        // его рекламные пути ниже отсекаются правилами пути, а запросы на
        // `yandex.ru/clck/...` в списке нет — открытие поиска работает.
        if pokhozh_na_reklamu(hvost) {
            return Some("домен");
        }
    }
    // Путь и параметры проверяем для ЛЮБОГО узла, включая узлы-сети: иначе
    // `yandex.ru/ads/...` (домен, равный правилу) выходил бы раньше.
    let (put, parametry) = match hvost.split_once('?') {
        Some((p, q)) => (p, q),
        None => (hvost, ""),
    };
    // ⛔ Параметр `?ad=` не режем, а ВЫРЕЗАЕМ из пути перед сверкой: иначе
    // `?ad=`/`?advertiser` совпал бы с «/ad» и зарезал обычную страницу.
    let put_bez_parametrov = put.split('?').next().unwrap_or(put);
    if let Some(pravilo) = PUTI.iter().find(|p| put_bez_parametrov.contains(**p)) {
        return Some(pravilo);
    }
    for (imya, znachenie) in parametry.split('&').filter_map(|p| p.split_once('=')) {
        if let Some((k, _)) = ZAPRESHCHENNYE_PARAMETRY.iter().find(|(k, _)| *k == imya) {
            if znachenie.is_empty() {
                continue;
            }
            return Some(k);
        }
    }
    None
}

/// Похож ли адрес на рекламный вызов у узла-сети. «Похож» — явные рекламные
/// признаки в пути/параметрах. ⛔ Так мы НЕ режем обычный ресурс субдомена
/// большой сети, но режем трекеры-картинки с `yabs.yandex.ru` у ЛЮБОГО
/// сайта — именно они и вешали полосу «загружаем».
fn pokhozh_na_reklamu(hvost: &str) -> bool {
    const ZNAKI: &[&str] = &[
        "/promo",
        "/ads",
        "/ad?",
        "/ad/",
        "/advert",
        "/banner",
        "/bann/",
        "/click",
        "/track",
        "/pixel",
        "/watch",
        "/count",
        "/stat",
        "/metrika",
        "/clck",
        "utm_source",
        "utm_medium",
        "utm_campaign",
        "ad_id",
        "adunit",
        "ad_unit",
        "yclid",
        "gclid",
        "fbclid",
    ];
    ZNAKI.iter().any(|z| hvost.contains(z))
}

/// Хост и путь из адреса. `None` — не http(s)/ws(s) (data:, blob:, about:).
fn razdelit(adres: &str) -> Option<(String, &str)> {
    let ostatok = adres
        .strip_prefix("https://")
        .or_else(|| adres.strip_prefix("http://"))
        .or_else(|| adres.strip_prefix("wss://"))
        .or_else(|| adres.strip_prefix("ws://"))
        .or_else(|| adres.strip_prefix("//"))?;
    let (host_port, hvost) = match ostatok.find(['/', '?', '#']) {
        Some(i) => (&ostatok[..i], &ostatok[i..]),
        None => (ostatok, "/"),
    };
    let host = host_port.rsplit('@').next().unwrap_or(host_port);
    let host = host
        .split(':')
        .next()
        .unwrap_or(host)
        .trim_end_matches('.')
        .to_string();
    if host.is_empty() {
        return None;
    }
    Some((host, hvost))
}

/// Куда перевести страницу: «перевести» в правом меню страницы.
pub fn perevod_adres(adres: &str, yazyk: &str) -> Result<String, String> {
    let a = adres.trim();
    if !(a.starts_with("http://") || a.starts_with("https://")) {
        return Err("Первую страницу не перевести: она не открыта из сети".into());
    }
    let yazyk = if yazyk.trim().is_empty() {
        "ru"
    } else {
        yazyk.trim()
    };
    if !yazyk
        .chars()
        .all(|c| c.is_ascii_alphabetic() || c == '-' || c == '_')
    {
        return Err("Неизвестный язык перевода".into());
    }
    let u = urlencode(a);
    Ok(format!(
        "https://translate.google.com/translate?sl=auto&tl={yazyk}&u={u}"
    ))
}

fn urlencode(s: &str) -> String {
    let mut r = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' | b':' => {
                r.push(b as char)
            }
            _ => r.push_str(&format!("%{b:02X}")),
        }
    }
    r
}

// ─────────────────────────── скрипт для страницы ───────────────────────────

/// Подписи-признаки плашек. Только рекламный текст: обычную кнопку
/// «Установить приложение» на полезном сайте так не задеть.
const ZNAKI: &[&str] = &[
    "скачать firefox",
    "загрузить firefox",
    "скачать chrome",
    "скачай chrome",
    "скачайте chrome",
    "скачать google chrome",
    "загрузить chrome",
    "установить chrome",
    "установить google chrome",
    "скачать яндекс браузер",
    "установить яндекс браузер",
    "яндекс браузер для windows",
    "скачать браузер",
    "сделать яндекс поиском",
    "яндекс — поиск по умолчанию",
    "сделать google поиском",
    "сделать google поисковой системой",
    "быстрее без тормозов",
    "быстрее и безопаснее",
    "установите другой браузер",
    "этот браузер устарел",
];

/// Имена классов-приманок: их почти никто, кроме рекламы, не использует.
const KLASSY: &[&str] = &[
    "advert",
    "promo-banner",
    "promo_banner",
    "browser-promo",
    "browser_promo",
    "browser-offer",
    "browser_offer",
    "smart-banner",
    "smart_banner",
    "app-banner",
    "app_banner",
    "install-banner",
    "install_banner",
    "get-browser",
    "browser-ad",
    "ad-banner",
    "ads-banner",
];

/// Страницы, где такие плашки живут. У остальных скрипт не запускаем вовсе —
/// на чужом сайте мы ничего не трогаем.
pub fn trebuet_skript(adres: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(adres) else { return false };
    if !matches!(url.scheme(), "http" | "https") { return false; }
    let host = url.host_str().unwrap_or("").trim_end_matches('.');
    ["ya.ru", "yandex.ru", "yandex.com", "yandex.by", "yandex.kz", "yandex.uz", "yandex.com.tr", "google.com", "google.ru", "mail.ru", "vk.com", "rambler.ru", "dzen.ru"]
        .iter().any(|d| host == *d || host.ends_with(&format!(".{d}")))
}

static SCRIPT_KOD: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// Скрипт для страницы. Собирается один раз за запуск процесса.
pub fn skript_dlya(adres: &str) -> Option<&'static str> {
    trebuet_skript(adres)
        .then(|| {
            SCRIPT_KOD.get_or_init(|| {
                let nacalo = format!(
                    "(function(){{if(window.__nbReklama)return;window.__nbReklama=1;\
                 var ZN={};var KL={};",
                    serde_json::to_string(ZNAKI).unwrap_or_else(|_| "[]".into()),
                    serde_json::to_string(KLASSY).unwrap_or_else(|_| "[]".into()),
                );
                format!(
                    "{nacalo}\
function skryt(el){{try{{if(!el||!el.isConnected)return;\
el.style.setProperty('display','none','important');el.remove();}}catch(e){{}}}}\
function otsenit(el){{if(!el||el.nodeType!==1)return;\
if(el===document.body||el===document.documentElement)return;\
var r=el.getBoundingClientRect();\
if(r.width*r.height>window.innerWidth*window.innerHeight*0.5)return;\
var t=(el.innerText||'').toLowerCase();if(t.length>2000)return;\
for(var i=0;i<ZN.length;i++)if(t.indexOf(ZN[i])>=0){{skryt(el);return;}}\
var k=((el.className&&el.className.baseVal)||el.className||'')+'';k=k.toLowerCase();\
for(var j=0;j<KL.length;j++)if(k.indexOf(KL[j])>=0){{skryt(el);return;}}}}\
function projti(k){{if(!k||!k.querySelectorAll)return;\
var v=k.querySelectorAll('div,section,aside,ins,iframe,span');\
for(var i=v.length-1;i>=0;i--)otsenit(v[i]);}}\
new MutationObserver(function(ms){{for(var i=0;i<ms.length;i++){{var d=ms[i].addedNodes;\
for(var j=0;j<d.length;j++){{var n=d[j];if(n.nodeType===1){{otsenit(n);projti(n);}}}}}}}}).observe(\
document.documentElement,{{childList:true,subtree:true}});projti(document);}})();"
                )
            })
        })
        .map(|s| s.as_str())
}

/// Настройка передаётся отдельно каждому процессу супер-вкладки.
pub fn vklyuchena() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::args().any(|a| a == "--nb-rezat-reklamu"))
}

// ───────────────────────── обработчик ресурсов CEF ─────────────────────────

wrap_resource_request_handler! {
    struct ResursStranicy { id: u64, navigaciya: bool }
    impl ResourceRequestHandler {
        fn on_before_resource_load(&self, _browser: Option<&mut Browser>, _frame: Option<&mut Frame>,
            request: Option<&mut Request>, callback: Option<&mut Callback>) -> ReturnValue {
            let adres = request
                .map(|r| CefString::from(&r.url()).to_string())
                .unwrap_or_default();
            if let Some(pravilo) = zarezannyj_resurs(&adres, self.navigaciya) {
                eprintln!("cef: вкладка {} зарезан ресурс ({pravilo})", self.id);
                if let Some(c) = callback { c.cancel(); }
                return ReturnValue::CANCEL;
            }
            ReturnValue::CONTINUE
        }
    }
}

/// Обработчик вызова для одной страницы. `initiator` — сайт, с которого идёт
/// вызов (для журнала; решение принимает сам адрес). `navigaciya` — это
/// документ верхней навигации: его не режем никогда.
pub fn handler_resursov(id: u64, _initiator: &str, navigaciya: bool) -> ResourceRequestHandler {
    ResursStranicy::new(id, navigaciya)
}

#[cfg(test)]
mod proverki {
    use super::*;

    #[test]
    fn reklamnye_puti_rezhem_dokumenty_ne_trogaem() {
        // Трекер-картинка у узла-сети Яндекса: домен-сеть + рекламный путь.
        assert_eq!(
            zarezannyj_resurs("https://yabs.yandex.ru/promo/p.gif", false),
            Some("домен-сеть")
        );
        // ⛔ Главная опасность — зарезать сам поиск. `yandex.ru` не должен
        // попасть ни под «домен» (он есть в SPISOK), ни под «домен-сеть»
        // (у него нет правил): иначе поиск Яндекса не откроется никогда.
        assert_eq!(
            zarezannyj_resurs("https://yandex.ru/search/?text=погода", false),
            None
        );
        assert_eq!(zarezannyj_resurs("https://yandex.ru/", false), None);
        // у узла-сети обычный ресурс не трогаем, рекламный — режем
        assert_eq!(zarezannyj_resurs("https://an.yandex.ru/favicon.ico", false), Some("домен"));
        assert!(zarezannyj_resurs("https://an.yandex.ru/ad/banner.png", false).is_some());
        // поддомен-сеть: маркер «.» перед правилом — не тот же домен
        assert!(zarezannyj_resurs("https://ads.example.com/x", false).is_none());
        assert!(zarezannyj_resurs("https://site.ru/ads/banner.png", false).is_some());
        assert!(zarezannyj_resurs("https://site.ru/page?utm_source=x", false).is_none());
        // документ верхней навигации — никогда: иначе поиск Яндекса не откроется
        assert_eq!(zarezannyj_resurs("https://yandex.ru/ads/help", true), None);
        // обычное — не трогаем
        assert_eq!(
            zarezannyj_resurs("https://site.ru/news/2026/11/16", false),
            None
        );
        assert_eq!(zarezannyj_resurs("data:image/png;base64,AAAA", false), None);
        assert_eq!(zarezannyj_resurs("about:blank", false), None);
    }

    #[test]
    fn skript_tolko_dlya_svoih_stranic() {
        assert!(trebuet_skript("https://yandex.ru/"));
        assert!(trebuet_skript("https://www.google.com/"));
        assert!(!trebuet_skript("https://example.com/"));
        assert!(!trebuet_skript("view-source:https://yandex.ru/"));
        let s = skript_dlya("https://yandex.ru/").unwrap();
        assert!(s.starts_with("(function()") && s.ends_with("})();"));
        assert!(s.contains("MutationObserver"));
        assert!(!trebuet_skript("https://example.com/?url=https://yandex.ru/"));
        assert!(!trebuet_skript("https://evilgoogle.com/"));
        assert!(trebuet_skript("https://ya.ru/"));
    }

    #[test]
    fn perevod_stroitsya_iz_bezopasnogo_adresa() {
        let u = perevod_adres("https://example.com/a?b=c&d=е", "ru").unwrap();
        let parsed = reqwest::Url::parse(&u).unwrap();
        assert_eq!(parsed.query_pairs().find(|(key, _)| key == "u").unwrap().1, "https://example.com/a?b=c&d=е");
        assert!(perevod_adres("about:blank", "ru").is_err());
        assert!(perevod_adres("https://a.b/", "ru; rm").is_err());
    }
}
