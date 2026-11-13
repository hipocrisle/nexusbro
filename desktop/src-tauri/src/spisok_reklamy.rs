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
