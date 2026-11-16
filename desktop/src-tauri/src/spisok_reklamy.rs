//! Начальный список рекламных и следящих узлов.
//!
//! ⛔ Это ЗАГОТОВКА, а не полноценный блокировщик. Настоящие списки — это
//! десятки тысяч правил, их надо качать и обновлять. Здесь то, что даёт
//! основную пользу сразу и ничего не ломает: рекламные сети и счётчики,
//! без «серых» случаев вроде сетей доставки содержимого.
//!
//! Совпадение по ХВОСТУ имени: `ads.example.com` поймает и
//! `a.ads.example.com`, но не поймает `notads.example.com`.

pub const SPISOK: &[&str] = &[
    // ── реклама и промо Яндекса (17.11.2026: на главной лезло «сделать
    //    Яндекс поиском», «скачай Яндекс Браузер», блоки рекламы) ──
    "yabs.yandex.ru",
    "an.yandex.ru",
    "ads.yandex.ru",
    "advertising.yandex.ru",
    "an.yandex.com",
    "yandexwebcache.net",
    "strm.yandex.ru",
    // ── подборки «читайте ещё» и медиареклама ──
    "ad.adriver.ru",
    "adv.mail.ru",
    "target.my.com",
    "x01.aidata.io",
    "onetag-sys.com",
    "mediametrics.ru",
    // рекламные сети Google
    "doubleclick.net",
    "googlesyndication.com",
    "googleadservices.com",
    "adservice.google.com",
    "pagead2.googlesyndication.com",
    // счётчики Google
    "google-analytics.com",
    "analytics.google.com",
    "googletagmanager.com",
    "googletagservices.com",
    // российские счётчики и реклама
    "mc.yandex.ru",
    "an.yandex.ru",
    "adfox.ru",
    "ads.adfox.ru",
    "top-fwz1.mail.ru",
    "ad.mail.ru",
    "rb.mail.ru",
    "counter.rambler.ru",
    "top100.rambler.ru",
    "smi2.ru",
    "luckyads.pro",
    "relap.io",
    // крупные зарубежные сети
    "adnxs.com",
    "adsrvr.org",
    "criteo.com",
    "criteo.net",
    "taboola.com",
    "outbrain.com",
    "pubmatic.com",
    "rubiconproject.com",
    "openx.net",
    "casalemedia.com",
    "sharethrough.com",
    "smartadserver.com",
    "teads.tv",
    "33across.com",
    "bidswitch.net",
    // слежка соцсетей
    "connect.facebook.net",
    "ads.twitter.com",
    "analytics.tiktok.com",
    "ads.tiktok.com",
    // прочие счётчики
    "scorecardresearch.com",
    "quantserve.com",
    "hotjar.com",
    "mixpanel.com",
    "segment.io",
    "amplitude.com",
    "branch.io",
    "appsflyer.com",
    "adjust.com",
    "sentry.io",
    "bugsnag.com",
    "newrelic.com",
    "nr-data.net",
];

/// Рекламные ПУТИ внутри сайта. ⛔ Смотрим их только у вызовов страницы
/// (`borba_s_reklamoy.rs`), и НИКОГДА — у документа верхней навигации:
/// `yandex.ru/ads/...` — реклама, а сам yandex.ru обязан открываться.
pub const PUTI: &[&str] = &[
    "/ads/", "/advert", "/adv/", "/banners/", "/promo/", "/pagead/",
    "/adserver", "/adsense", "/gampad", "/doubleclick", "/yabs/",
];

/// Параметры адреса, которые есть ТОЛЬКО у рекламных и следящих вызовов.
/// (utm_* не берём: их ставят и обычные ссылки — сломали бы переходы.)
pub const ZAPRESHCHENNYE_PARAMETRY: &[(&str, &str)] = &[
    ("ad_type", ""), ("adtype", ""), ("ad_unit", ""), ("adunit", ""),
    ("gclid", ""), ("yclid", ""), ("fbclid", ""),
    ("adv_id", ""), ("banner_id", ""), ("clickid", ""),
];

/// Поймать рекламный узел по хвосту имени (с поддоменами).
pub fn reklamnyj_host(host: &str) -> Option<&'static str> {
    let h = host.trim_end_matches('.').to_ascii_lowercase();
    SPISOK.iter().copied().find(|p| h == *p || h.ends_with(&format!(".{p}")))
}
