//! NexusBro — оболочка с супер-вкладками поверх готового Chromium.
//!
//! Супер-вкладка = отдельный процесс движка со СВОИМ профилем и СВОИМ
//! выходом в сеть. Внутри неё человек открывает сколько угодно обычных
//! вкладок, и весь сервис со всеми его адресами идёт через нужный выход.

mod brauzer;
mod okna;
mod proxy;
mod vpn;
mod spisok_reklamy;

use okna::{sistema, Mesto};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::process::Child;
use std::sync::Mutex;
use tauri::{Emitter, Manager, State};

/// То, что уходит в полосу вкладок.
#[derive(Clone, Serialize)]
pub struct Vidno {
    #[serde(flatten)]
    pub nastrojki: Nastrojki,
    pub zapushchena: bool,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Nastrojki {
    pub id: String,
    pub nazvanie: String,
    /// Пусто = прямое соединение. Иначе вид `socks5://127.0.0.1:1080`.
    #[serde(default)]
    pub proxy: String,
    #[serde(default = "domashnyaya")]
    pub adres: String,
    /// Резать рекламу в этой супер-вкладке.
    #[serde(default = "da_po_umolchaniyu")]
    pub rezat_reklamu: bool,
    /// Открывать пустой: не восстанавливать вкладки прошлой сессии.
    #[serde(default)]
    pub pustaya: bool,
}

fn da_po_umolchaniyu() -> bool {
    true
}

fn domashnyaya() -> String {
    "about:blank".into()
}

/// Файл со списком открытых супер-вкладок. Лежит рядом с профилями.
fn fajl_spiska() -> std::path::PathBuf {
    brauzer::papka_profilej()
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."))
        .join("vkladki.json")
}

/// Файл с сохранёнными (закрытыми, но не выброшенными) супер-вкладками.
fn fajl_sohranennyh() -> std::path::PathBuf {
    brauzer::papka_profilej()
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."))
        .join("sohranennye.json")
}

fn zagruzit_spisok() -> Vec<Nastrojki> {
    std::fs::read_to_string(fajl_spiska())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn zagruzit_sohranennye() -> Vec<Nastrojki> {
    std::fs::read_to_string(fajl_sohranennyh())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn sohranit_spisok(v: &[Nastrojki]) {
    let f = fajl_spiska();
    if let Some(d) = f.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    if let Ok(s) = serde_json::to_string_pretty(v) {
        let _ = std::fs::write(f, s);
    }
}

fn sohranit_sohranennye(v: &[Nastrojki]) {
    let f = fajl_sohranennyh();
    if let Some(d) = f.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    if let Ok(s) = serde_json::to_string_pretty(v) {
        let _ = std::fs::write(f, s);
    }
}

struct Vkladka {
    nastrojki: Nastrojki,
    /// Свой прокси этой вкладки: режет рекламу и уводит в нужный выход.
    nash_proxy: Option<proxy::Rabotayushchij>,
    /// Туннель вкладки, если выход задан подпиской или ссылкой на узел.
    tunnel: Option<std::process::Child>,
    /// None = вкладка сохранена, но движок не запущен.
    /// ⛔ Запускать все сразу нельзя: каждый экземпляр ест сотни мегабайт,
    /// десять вкладок — несколько гигабайт на пустом месте.
    proc: Option<Child>,
    okno: Option<u32>,
    /// Реальный pid браузера, когда Chromium перезапустил себя из-под
    /// нашего мёртвого дочернего процесса (10.11.2026). Убиваем по нему.
    zapas_pid: Option<u32>,
}

#[derive(Default)]
struct Sostoyanie {
    vkladki: Mutex<HashMap<String, Vkladka>>,
    /// Где на экране показывать движок — это место нам сообщает лицевая часть.
    mesto: Mutex<Option<Mesto>>,
    aktivnaya: Mutex<Option<String>>,
}

/// Выставляется при завершении программы: фоновые потоки (сторож окон)
/// обязаны остановиться, чтобы не мешать гашению движков.
static ZAVERSHENIE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Журнал сторожа и запусков — последние события с метками времени.
/// Диагностика показывает его человеку, человек присылает нам факты
/// вместо гаданий (10.11.2026).
static ZHURNAL: std::sync::Mutex<std::collections::VecDeque<(std::time::Instant, String)>> =
    std::sync::Mutex::new(std::collections::VecDeque::new());

fn zapisat_v_zhurnal(sobytie: String) {
    if let Ok(mut z) = ZHURNAL.lock() {
        z.push_back((std::time::Instant::now(), sobytie));
        while z.len() > 40 {
            z.pop_front();
        }
    }
}

#[derive(Serialize)]
pub struct Otvet {
    pub ok: bool,
    pub soobshchenie: String,
}

fn da() -> Otvet {
    Otvet { ok: true, soobshchenie: String::new() }
}
fn net(s: impl std::fmt::Display) -> Otvet {
    Otvet { ok: false, soobshchenie: s.to_string() }
}

/// Собрать ошибку запуска движка с хвостом его журнала: одного
/// скриншота хватает, чтобы понять, что ему не понравилось.
fn so_oshibkoj_dvizhka(id: &str, zagolovok: String) -> Otvet {
    let log = brauzer::papka_profilej().join(id).join("engine.log");
    let hvost = std::fs::read_to_string(&log).ok()
        .map(|s| {
            s.lines().rev().take(12).collect::<Vec<_>>()
                .into_iter().rev().collect::<Vec<_>>()
                .join("\n")
        })
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| {
            "(журнал пуст — движок не проронил ни строчки: его, скорее всего, не дали запуститься)".into()
        });
    net(format!("{}. Журнал: {}\n---\n{}", zagolovok, log.display(), hvost))
}

/// Куда встраивать движок: место сообщает лицевая часть при каждом
/// изменении размера, чтобы окно браузера точно легло под полосу вкладок.
#[tauri::command]
fn zadat_mesto(s: State<Sostoyanie>, x: i32, y: i32, w: u32, h: u32) -> Otvet {
    let m = Mesto { x, y, w, h };
    *s.mesto.lock().unwrap() = Some(m);
    let aktivnaya = s.aktivnaya.lock().unwrap().clone();
    if let Some(id) = aktivnaya {
        if let Some(v) = s.vkladki.lock().unwrap().get(&id) {
            if let Some(o) = v.okno {
                let _ = sistema::peremestit(o, m);
            }
        }
    }
    da()
}

#[tauri::command]
fn spisok(s: State<Sostoyanie>) -> Vec<Vidno> {
    let v = s.vkladki.lock().unwrap();
    let mut r: Vec<Vidno> = v
        .values()
        .map(|x| Vidno {
            nastrojki: x.nastrojki.clone(),
            zapushchena: x.proc.is_some() || x.zapas_pid.is_some(),
        })
        .collect();
    r.sort_by(|a, b| a.nastrojki.id.cmp(&b.nastrojki.id));
    r
}

/// Сколько реклам зарезано в этой вкладке — чтобы человек видел пользу.
#[tauri::command]
fn schet_rezki(s: State<Sostoyanie>, id: String) -> (u64, u64) {
    use std::sync::atomic::Ordering;
    match s.vkladki.lock().unwrap().get(&id).and_then(|v| v.nash_proxy.as_ref()) {
        Some(p) => (
            p.schet.zarezano.load(Ordering::Relaxed),
            p.schet.propushcheno.load(Ordering::Relaxed),
        ),
        None => (0, 0),
    }
}

#[tauri::command]
fn aktivnaya(s: State<Sostoyanie>) -> Option<String> {
    s.aktivnaya.lock().unwrap().clone()
}

#[tauri::command]
fn dvizhok_est() -> bool {
    brauzer::najti_dvizhok().is_some()
}

/// Спрятать/показать встроенные окна движков. Нужно лицевой части:
/// модальный диалог рисуется ВНУТРИ нашего окна, а поверх его области
/// лежит чужое окно движка — без скрытия диалог не видно (10.11.2026:
/// «добавление супервкладки не видно, пока не закроешь хром»).
/// ⛔ При скрытии прячем не только известные хэндлы, но ВСЕ окна наших
/// движков: пересозданное окно старым хэндлом не видно и лежит поверх,
/// поедая клики по диалогу.
#[tauri::command]
fn pokazat_dvizhki(s: State<Sostoyanie>, vidno: bool) -> Otvet {
    if !vidno {
        let pids: Vec<u32> = {
            let v = s.vkladki.lock().unwrap();
            v.values()
                .filter_map(|vk| vk.zapas_pid.or(vk.proc.as_ref().map(|p| p.id())))
                .collect()
        };
        sistema::spryatat_chuzhie(&pids);
    }
    let aktivnaya = s.aktivnaya.lock().unwrap().clone();
    let v = s.vkladki.lock().unwrap();
    for (k, vk) in v.iter() {
        if let Some(o) = vk.okno {
            let pokazat = vidno && Some(k) == aktivnaya.as_ref();
            let _ = sistema::pokazat(o, pokazat);
        }
    }
    da()
}

#[tauri::command]
fn otkryt(
    app: tauri::AppHandle,
    s: State<Sostoyanie>,
    nastrojki: Nastrojki,
) -> Otvet {
    let id = nastrojki.id.clone();
    {
        let mut v = s.vkladki.lock().unwrap();
        if v.contains_key(&id) {
            return net("такая супер-вкладка уже есть");
        }
        // Запоминаем СРАЗУ: если движок не запустится, вкладка всё равно
        // останется в списке, и человек сможет поправить настройки.
        v.insert(
            id.clone(),
            Vkladka { nastrojki: nastrojki.clone(), proc: None, okno: None, zapas_pid: None, nash_proxy: None, tunnel: None },
        );
        let vse: Vec<Nastrojki> = v.values().map(|x| x.nastrojki.clone()).collect();
        sohranit_spisok(&vse);
    }
    zapustit_vnutri(&app, &s, &id)
}

/// Встроить ВСЕ чужие видимые окна движка вкладки, вернуть крупнейшее.
/// ⛔ Сессия движка бывает многооконной (восстановление сессии, попапы):
/// каждое не встроенное окно висят поверх нас с рамкой и кнопками —
/// жалобы «окно хрома двигается, можно закрыть, закрывает меню»
/// (12.11.2026). Встраиваем все; крупнейшее считаем главным.
fn vstroit_vse_okna(
    app: &tauri::AppHandle,
    s: &State<Sostoyanie>,
    id: &str,
    koren_pid: u32,
) -> Option<u32> {
    let nashe = app.get_webview_window("main").and_then(|w| nash_descriptor(&w))?;
    let m = s.mesto.lock().unwrap().unwrap_or(Mesto { x: 0, y: 44, w: 1200, h: 700 });
    let okna = sistema::vse_okna(&[koren_pid]);
    let mut glavnoe = None;
    for o in &okna {
        if !sistema::nash_li(*o, nashe) {
            zapisat_v_zhurnal(format!("{id}: встраиваю ещё одно окно движка {o}"));
            if let Err(e) = sistema::vstroit(*o, nashe, m) {
                zapisat_v_zhurnal(format!("{id}: встроить окно {o} не вышло: {e}"));
            }
        }
        if glavnoe.is_none() {
            glavnoe = Some(*o);
        }
    }
    glavnoe
}

/// Запустить движок для уже записанной вкладки.
fn zapustit_vnutri(app: &tauri::AppHandle, s: &State<Sostoyanie>, id: &str) -> Otvet {
    let nastrojki = match s.vkladki.lock().unwrap().get(id) {
        Some(v) => v.nastrojki.clone(),
        None => return net("такой супер-вкладки нет"),
    };
    // ⛔ Движок всегда ходит через НАШ прокси, даже при прямом выходе:
    // иначе в прямом режиме не будет резки рекламы.
    // Выход бывает трёх видов: подписка или ссылка на узел (поднимаем
    // туннель), SOCKS5 и веб-прокси (идём напрямую в них).
    let mut tunnel: Option<std::process::Child> = None;
    let vyhod = if vpn::eto_podpiska(&nastrojki.proxy) {
        let papka = brauzer::papka_profilej().join(id);
        let zapis = nastrojki.proxy.trim().to_string();
        let itog = tauri::async_runtime::block_on(async {
            // ссылка подписки — скачиваем и берём первый узел
            let ssylka = if zapis.starts_with("http") {
                let uzly = vpn::skachat_podpisku(&zapis).await?;
                uzly[0].ssylka.clone()
            } else {
                zapis.clone()
            };
            vpn::podnyat_tunnel(&papka, &ssylka).await
        });
        match itog {
            Ok((d, port)) => {
                tunnel = Some(d);
                proxy::Vyhod::Socks5 {
                    adres: format!("127.0.0.1:{}", port),
                    vhod: None,
                }
            }
            Err(e) => return net(format!("туннель не поднялся: {}", e)),
        }
    } else {
        match proxy::razobrat_vyhod(&nastrojki.proxy) {
            Ok(v) => v,
            Err(e) => return net(format!("выход записан непонятно: {}", e)),
        }
    };
    let nash = tauri::async_runtime::block_on(proxy::podnyat(proxy::Nastrojka {
        vyhod,
        rezat_reklamu: nastrojki.rezat_reklamu,
    }));
    let nash = match nash {
        Ok(p) => p,
        Err(e) => return net(format!("свой прокси не поднялся: {}", e)),
    };
    let cherez = format!("http://127.0.0.1:{}", nash.port);

    // Вкладки прошлой сессии восстанавливаются по умолчанию; адрес
    // подаём только для режима «открывать пустой» или самого первого
    // запуска — иначе Chromium открыл бы адрес вместо восстановления.
    let adres = if nastrojki.pustaya || brauzer::profil_svety(&id) {
        Some(nastrojki.adres.as_str())
    } else {
        None
    };
    // ⛔ Перед стартом подметаем сирот: живой чужой движок с этим
    // профилем = новый процесс перешлёт ему команду и выйдет с кодом 0,
    // а сирота откроет «новое окно хрома» (жалобы user'а 10.11.2026).
    // Имя бинарника уникально — чужих таких процессов не бывает.
    {
        let nashi: Vec<u32> = {
            let v = s.vkladki.lock().unwrap();
            v.values()
                .filter_map(|vk| vk.zapas_pid.or(vk.proc.as_ref().map(|p| p.id())))
                .collect()
        };
        brauzer::ubit_chuzhie_dvizhki(&nashi);
    }

    // Две попытки: мгновенная смерть движка чинится подметанием сирот
    // и повторным стартом.
    let para: Result<(std::process::Child, u32), String> = 'popitki: {
        let mut poslednyaya = String::from("движок не запустился");
        for popitka in 1..=2 {
            let mut d = match brauzer::zapustit(&id, Some(&cherez), adres, !nastrojki.pustaya) {
                Ok(c) => c,
                Err(e) => {
                    poslednyaya = e.to_string();
                    if popitka == 1 {
                        zapisat_v_zhurnal(format!("{id}: запуск не состоялся, повторяю: {e}"));
                        continue;
                    }
                    break 'popitki Err(poslednyaya);
                }
            };
            let pid = d.id();
            let nachalo = std::time::Instant::now();
            let okno = loop {
                match sistema::najti_okno(pid, 5) {
                    Ok(o) => break Some(o),
                    Err(e) => {
                        if let Ok(Some(kod)) = d.try_wait() {
                            poslednyaya = format!("движок умер сразу после запуска ({kod})");
                            zapisat_v_zhurnal(format!("{id}: {poslednyaya}"));
                            break None;
                        }
                        if nachalo.elapsed().as_secs() >= 20 {
                            let _ = d.kill();
                            let _ = d.wait();
                            poslednyaya = format!("движок запустился, но окно не нашлось: {e}");
                            zapisat_v_zhurnal(format!("{id}: {poslednyaya}"));
                            break None;
                        }
                    }
                }
            };
            if let Some(o) = okno {
                break 'popitki Ok((d, o));
            }
            if popitka == 1 {
                // вероятно, сирота перехватила: подметаем и повторяем
                let nashi: Vec<u32> = {
                    let v = s.vkladki.lock().unwrap();
                    v.values().filter_map(|vk| vk.proc.as_ref().map(|p| p.id())).collect()
                };
                brauzer::ubit_chuzhie_dvizhki(&nashi);
                zapisat_v_zhurnal(format!("{id}: повторный запуск после мгновенной смерти"));
            }
        }
        break 'popitki Err(poslednyaya);
    };
    let (mut ditya, okno) = match para {
        Ok(para) => para,
        Err(soobshchenie) => {
            nash.ostanovit();
            if let Some(mut t) = tunnel {
                let _ = t.kill();
                let _ = t.wait();
            }
            return so_oshibkoj_dvizhka(&id, soobshchenie);
        }
    };

    // Окно нашлось. Но наш дочерний процесс может быть уже мёртв:
    // Chromium иногда ПЕРЕЗАПУСКАЕТ себя с новым pid (уходит из job
    // object; смену pid поймали на машине user'а). Тогда окно живо —
    // продолжаем с ним, запомнив реальный pid. Если же окно живо без
    // всякого pid — это окно ОШИБКИ («профиль занят»): не встраиваем.
    let zapas = sistema::pid_okna(okno);
    if matches!(ditya.try_wait(), Ok(Some(_))) && zapas.is_none() {
        nash.ostanovit();
        if let Some(mut t) = tunnel {
            let _ = t.kill();
            let _ = t.wait();
        }
        zapisat_v_zhurnal(format!("{id}: окно есть, но живого движка нет — не встраиваю"));
        return so_oshibkoj_dvizhka(
            &id,
            "движок отказал сразу после старта: профиль, вероятно, занят сиротой".into(),
        );
    }
    if matches!(ditya.try_wait(), Ok(Some(_))) {
        zapisat_v_zhurnal(format!("{id}: дочерний процесс умер, окно живо — перезапуск движка, запасной pid {:?}", zapas));
    }
    // ⛔ Дочерний мог уже выйти (Chromium на Windows перезапускает себя):
    // тогда вкладка живёт по запасному pid — дальше смотрим по окну.
    let rebenok_zhiv = matches!(ditya.try_wait(), Ok(None));
    if !rebenok_zhiv {
        zapisat_v_zhurnal(format!("{id}: дочерний вышел сразу — движок живёт своим pid"));
    }

    let nashe = match app.get_webview_window("main").and_then(|w| nash_descriptor(&w)) {
        Some(h) => h,
        None => {
            let _ = ditya.kill();
            let _ = ditya.wait();
            nash.ostanovit();
            if let Some(mut t) = tunnel {
                let _ = t.kill();
                let _ = t.wait();
            }
            return net("не получил указатель на своё окно");
        }
    };
    let m = s.mesto.lock().unwrap().unwrap_or(Mesto { x: 0, y: 44, w: 1200, h: 700 });
    if let Err(e) = sistema::vstroit(okno, nashe, m) {
        let _ = ditya.kill();
        let _ = ditya.wait();
        nash.ostanovit();
        if let Some(mut t) = tunnel {
            let _ = t.kill();
            let _ = t.wait();
        }
        return net(format!("встроить не вышло: {}", e));
    }
    // ⛔ Сессия могла восстановиться в НЕСКОЛЬКИХ окнах: встраиваем все
    // чужие окна этого движка, иначе лишние висят поверх с рамкой.
    let koren = zapas.unwrap_or_else(|| ditya.id());
    let okno = vstroit_vse_okna(app, s, id, koren).unwrap_or(okno);

    if let Some(v) = s.vkladki.lock().unwrap().get_mut(id) {
        // ⛔ Дочерний мог уже выйти (Chromium перезапускает себя): трупа
        // не держим в поле, по которому решаем «работает ли вкладка», —
        // живость дальше определяем по окну и запасному pid.
        v.proc = if rebenok_zhiv { Some(ditya) } else { None };
        v.okno = Some(okno);
        v.zapas_pid = zapas;
        v.nash_proxy = Some(nash);
        v.tunnel = tunnel;
    }
    pereklyuchit_vnutri(s, id);
    // ⛔ Ввод: встроенное окно живёт в ЧУЖОЙ очереди ввода, клавиатура до
    // него не доходит, пока очереди не прикреплены (AttachThreadInput).
    // Прикрепляет поток, владеющий нашим окном, — наш UI-поток.
    let _ = app.run_on_main_thread(move || {
        sistema::prisoedinit_vvod(okno);
        sistema::fokus_dvizhku(okno);
    });
    zapisat_v_zhurnal(format!("{id}: запуск ок, окно {okno}"));
    // ⛔ Полоса вкладок должна узнавать об изменениях, сделанных ядром, —
    // иначе вкладка есть, а в полосе её нет (ловили на самопроверке).
    let _ = app.emit("vkladki-izmenilis", id.to_string());
    da()
}

#[tauri::command]
fn pereklyuchit(app: tauri::AppHandle, s: State<Sostoyanie>, id: String) -> Otvet {
    let zapushchena = match s.vkladki.lock().unwrap().get_mut(&id) {
        Some(vk) => {
            // ⛔ Дочерний процесс — «пусковая ракета»: Chromium на Windows
            // перезапускает себя и стартовый процесс выходит. Смерть
            // дочернего ≠ смерть движка — движок жив, пока живо ЕГО окно
            // и его реальный pid. Хоронили живой движок и перезапускали
            // его — а подметание при перезапуске резало соседнюю вкладку
            // (12.11.2026, диагностика user'а: «мёртвый движок при
            // переключении» через 2 с после успешного запуска).
            let rebenok_zhiv = matches!(
                vk.proc.as_mut().map(|p| p.try_wait()),
                Some(Ok(None))
            );
            if rebenok_zhiv {
                true
            } else {
                // Вкладка могла вообще не запускаться (спит с открытия
                // программы) — тогда «мёртвого движка» в журнале не пишем.
                let bilo_chto_to =
                    vk.proc.is_some() || vk.okno.is_some() || vk.zapas_pid.is_some();
                if let Some(mut p) = vk.proc.take() {
                    let _ = p.try_wait(); // прибрать мёртвого ребёнка
                }
                let okno_zhivo = vk.okno.map(sistema::okno_zhivo).unwrap_or(false);
                let dvizhok_zhiv = vk.zapas_pid.map(sistema::pid_zhiv).unwrap_or(false);
                if okno_zhivo && dvizhok_zhiv {
                    true
                } else {
                    if bilo_chto_to {
                        zapisat_v_zhurnal(format!("{id}: мёртвый движок при переключении, поднимаю заново"));
                    }
                    if let Some(zp) = vk.zapas_pid.take() {
                        sistema::ubit_po_pid(zp);
                    }
                    vk.okno = None;
                    if let Some(p) = vk.nash_proxy.take() {
                        p.ostanovit();
                    }
                    if let Some(mut t) = vk.tunnel.take() {
                        let _ = t.kill();
                        let _ = t.wait();
                    }
                    false
                }
            }
        }
        None => return net("такой супер-вкладки нет"),
    };
    // Вкладку поднимаем в тот момент, когда на неё нажали, а не при запуске
    // программы — иначе десять вкладок сразу съедят всю память.
    if !zapushchena {
        return zapustit_vnutri(&app, &s, &id);
    }
    pereklyuchit_vnutri(&s, &id);
    // Клавиатуру — показанному движку (очереди уже прикреплены).
    if let Some(o) = s.vkladki.lock().unwrap().get(&id).and_then(|v| v.okno) {
        let _ = app.run_on_main_thread(move || sistema::fokus_dvizhku(o));
    }
    da()
}

fn sohranit_aktivnuyu(id: &str) {
    let f = brauzer::papka_profilej()
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."))
        .join("aktivnaya.json");
    let _ = std::fs::write(f, serde_json::to_string(id).unwrap_or_default());
}

fn zagruzit_aktivnuyu() -> Option<String> {
    let f = brauzer::papka_profilej()
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."))
        .join("aktivnaya.json");
    std::fs::read_to_string(f)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
}

fn pereklyuchit_vnutri(s: &State<Sostoyanie>, id: &str) {
    let m = s.mesto.lock().unwrap().clone();
    let v = s.vkladki.lock().unwrap();
    for (k, vk) in v.iter() {
        if let Some(o) = vk.okno {
            let nuzhna = k == id;
            let _ = sistema::pokazat(o, nuzhna);
            if nuzhna {
                if let Some(mm) = m {
                    let _ = sistema::peremestit(o, mm);
                }
            }
        }
    }
    *s.aktivnaya.lock().unwrap() = Some(id.to_string());
    // Помним, на какой вкладке закрыли программу, — при следующем
    // запуске откроется именно она (10.11.2026, задал user).
    sohranit_aktivnuyu(id);
}

/// Глушить экземпляр вкладки: открепляем ввод и убиваем процесс НА
/// НАШЕМ UI-ПОТОКЕ. ⛔ Убить чужой процесс, к потоку которого мы
/// прикреплены AttachThreadInput, НЕЛЬЗЯ: Windows вешает нашу очередь
/// ввода — после закрытия вкладки программа перестаёт реагировать,
/// а окно движка остаётся висеть (10.11.2026, поймал user).
fn ostanovit_vnutri(app: &tauri::AppHandle, s: &State<Sostoyanie>, id: &str) {
    let (mut proc, okno, zapas) = {
        let mut v = s.vkladki.lock().unwrap();
        match v.get_mut(id) {
            Some(vk) => (vk.proc.take(), vk.okno.take(), vk.zapas_pid.take()),
            None => return,
        }
    };
    let pid_rebenka = proc.as_ref().map(|p| p.id());
    let rebenok_zhiv = proc
        .as_mut()
        .map(|p| matches!(p.try_wait(), Ok(None)))
        .unwrap_or(false);
    if rebenok_zhiv {
        // Обычный путь: открепление и чистая остановка — на нашем
        // UI-потоке (убийство прикреплённого AttachThreadInput процесса
        // с чужого потока вешает нашу очередь ввода).
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        let otpravleno = app.run_on_main_thread(move || {
            if let Some(o) = okno {
                sistema::otsoedinit_vvod(o);
            }
            if let Some(mut p) = proc {
                brauzer::ostanovit_dvizhka(&mut p, okno);
            }
            let _ = tx.send(());
        });
        if otpravleno.is_ok() {
            // Не ждём дольше 10 с: задание всё равно добьёт дерево.
            let _ = rx.recv_timeout(std::time::Duration::from_secs(10));
        }
        // Движок мог перезапуститься с другим pid — дочерний его не добьёт.
        if let Some(zp) = zapas {
            if pid_rebenka != Some(zp) {
                sistema::ubit_po_pid(zp);
            }
        }
    } else {
        // ⛔ Дочернего нет или мёртв: движок живёт по запасному pid
        // (Chromium перезапустил себя). Закрываем ВЕЖЛИВО через окно —
        // убийством сессия не сбросится, а она должна.
        if let Some(zp) = zapas {
            if let Some(o) = okno {
                let _ = app.run_on_main_thread(move || {
                    sistema::otsoedinit_vvod(o);
                    sistema::zakryt_vezhlivo(o);
                });
                let srok = std::time::Instant::now();
                while sistema::pid_zhiv(zp) && srok.elapsed().as_secs() < 5 {
                    std::thread::sleep(std::time::Duration::from_millis(150));
                }
            }
            sistema::ubit_po_pid(zp);
        }
        if let Some(mut p) = proc {
            let _ = p.try_wait(); // прибрать мёртвого ребёнка
        }
    }
    let (proxy, tunnel) = {
        let mut v = s.vkladki.lock().unwrap();
        match v.get_mut(id) {
            Some(vk) => (vk.nash_proxy.take(), vk.tunnel.take()),
            None => (None, None),
        }
    };
    if let Some(p) = proxy {
        p.ostanovit();
    }
    if let Some(mut t) = tunnel {
        let _ = t.kill();
        let _ = t.wait();
    }
}

/// Правка настроек супер-вкладки после создания. Работающую перезапускаем
/// с новыми настройками (прокси и резка рекламы задаются на запуск),
/// остановленную просто перезаписываем.
#[tauri::command]
fn izmenit(app: tauri::AppHandle, s: State<Sostoyanie>, nastrojki: Nastrojki) -> Otvet {
    let rabotaet = {
        let mut v = s.vkladki.lock().unwrap();
        match v.get_mut(&nastrojki.id) {
            Some(vk) => {
                vk.nastrojki = nastrojki.clone();
                vk.proc.is_some()
            }
            None => return net("такой супер-вкладки нет"),
        }
    };
    {
        let v = s.vkladki.lock().unwrap();
        let vse: Vec<Nastrojki> = v.values().map(|x| x.nastrojki.clone()).collect();
        sohranit_spisok(&vse);
    }
    if rabotaet {
        ostanovit_vnutri(&app, &s, &nastrojki.id);
        return zapustit_vnutri(&app, &s, &nastrojki.id);
    }
    let _ = app.emit("vkladki-izmenilis", nastrojki.id);
    da()
}

/// Есть ли в профиле что-то стоящее сохранять: куки, история. Свежая
/// вкладка закрывается без вопросов; жилая спрашивает «сохранить?».
#[tauri::command]
fn nuzhno_sohranenie(id: String) -> bool {
    !brauzer::profil_svety(&id)
}

/// Стереть профиль вкладки. Файлы могут быть ещё подхвачены только что
/// убитым движком (Windows держит блокировки) — повторяем несколько раз,
/// иначе «не сохраняет/не удаляет».
fn steret_profil(id: &str) {
    let put = brauzer::papka_profilej().join(id);
    for _ in 0..5 {
        if std::fs::remove_dir_all(&put).is_ok() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(400));
    }
    let _ = std::fs::remove_dir_all(&put);
}

/// Закрыть супер-вкладку. `sohranit` — true: профиль и настройки кладём
/// в список сохранённых (открыть заново через меню у «+»); false: профиль
/// выбрасывается насовсем. Подтверждение спрашивает лицевая часть.
#[tauri::command]
fn zakryt(app: tauri::AppHandle, s: State<Sostoyanie>, id: String, sohranit: bool) -> Otvet {
    // Убираем из списка ПЕРВЫМ и отпускаем мьютекс — остановка движка
    // сама берёт этот мьютекс, держать его тут = тупик.
    let udalena = {
        let mut v = s.vkladki.lock().unwrap();
        v.remove(&id).map(|vk| vk.nastrojki)
    };
    let Some(nastrojki) = udalena else {
        return net("такой супер-вкладки нет");
    };
    ostanovit_vnutri(&app, &s, &id);
    {
        let v = s.vkladki.lock().unwrap();
        let vse: Vec<Nastrojki> = v.values().map(|x| x.nastrojki.clone()).collect();
        sohranit_spisok(&vse);
    }
    if sohranit {
        let mut sohr = zagruzit_sohranennye();
        sohr.retain(|n| n.id != id);
        sohr.push(nastrojki);
        sohranit_sohranennye(&sohr);
    } else {
        steret_profil(&id);
    }
    let _ = app.emit("vkladki-izmenilis", id);
    da()
}

/// Сохранённые супер-вкладки — для меню у кнопки «+».
#[tauri::command]
fn sohranennye() -> Vec<Nastrojki> {
    let mut v = zagruzit_sohranennye();
    v.sort_by(|a, b| a.nazvanie.cmp(&b.nazvanie));
    v
}

/// Открыть сохранённую супер-вкладку заново: из списка сохранённых —
/// в полосу, движок поднимается.
#[tauri::command]
fn vosstanovit(app: tauri::AppHandle, s: State<Sostoyanie>, id: String) -> Otvet {
    let nastrojki = {
        let mut sohr = zagruzit_sohranennye();
        let n = match sohr.iter().position(|n| n.id == id) {
            Some(i) => sohr.remove(i),
            None => return net("такой сохранённой вкладки нет"),
        };
        sohranit_sohranennye(&sohr);
        n
    };
    {
        let mut v = s.vkladki.lock().unwrap();
        if v.contains_key(&id) {
            return net("такая супер-вкладка уже открыта");
        }
        v.insert(id.clone(), Vkladka { nastrojki: nastrojki.clone(), proc: None, okno: None, zapas_pid: None, nash_proxy: None, tunnel: None });
        let vse: Vec<Nastrojki> = v.values().map(|x| x.nastrojki.clone()).collect();
        sohranit_spisok(&vse);
    }
    zapustit_vnutri(&app, &s, &id)
}

/// Выбросить сохранённую супер-вкладку насовсем: настройки и профиль
/// со всеми входами. Подтверждение спрашивает лицевая часть.
#[tauri::command]
fn udalit_sohranennuyu(id: String) -> Otvet {
    let mut sohr = zagruzit_sohranennye();
    if sohr.iter().any(|n| n.id == id) {
        sohr.retain(|n| n.id != id);
        sohranit_sohranennye(&sohr);
        steret_profil(&id);
        da()
    } else {
        net("такой сохранённой вкладки нет")
    }
}

/// Перезагрузить движок вкладки (контекстное меню вкладки).
#[tauri::command]
fn perezagruzit(app: tauri::AppHandle, s: State<Sostoyanie>, id: String) -> Otvet {
    if !s.vkladki.lock().unwrap().contains_key(&id) {
        return net("такой супер-вкладки нет");
    }
    ostanovit_vnutri(&app, &s, &id);
    zapustit_vnutri(&app, &s, &id)
}

/// Диагностика по живому: состояние вкладок, окна, журнал сторожа,
/// хвосты engine.log. Человек копирует и присылает — разбираем факты,
/// а не гадания (10.11.2026).
#[tauri::command]
fn diagnostika(app: tauri::AppHandle, s: State<Sostoyanie>) -> String {
    let mut out = String::from("=== NexusBro диагностика ===\n");
    let nashe = app
        .get_webview_window("main")
        .and_then(|w| nash_descriptor(&w))
        .map(|h| h.to_string())
        .unwrap_or_else(|| "нет".into());
    out.push_str(&format!("наше окно: {nashe}\n"));
    out.push_str(&format!("впереди сейчас: {}\n", sistema::kto_v_peredne()));
    {
        let v = s.vkladki.lock().unwrap();
        for (id, vk) in v.iter() {
            let proc_s = match (&vk.proc, vk.zapas_pid) {
                (Some(p), Some(z)) if p.id() != z => format!("pid {} (реальный {})", p.id(), z),
                (Some(p), _) => format!("pid {}", p.id()),
                (None, Some(z)) => format!("pid {} (движок перезапущен)", z),
                (None, None) => "нет".into(),
            };
            let okno_s = match vk.okno {
                Some(o) => {
                    let nash = sistema::nash_li(o, nashe.parse().unwrap_or(0));
                    format!("{o} (наше: {nash})")
                }
                None => "нет".into(),
            };
            out.push_str(&format!("вкладка {id}: движок {proc_s}, окно {okno_s}\n"));
        }
    }
    out.push_str("\n--- журнал ---\n");
    if let Ok(z) = ZHURNAL.lock() {
        let start = z.front().map(|(t, _)| *t);
        for (t, m) in z.iter() {
            let secs = start.map(|st| t.duration_since(st).as_secs()).unwrap_or(0);
            out.push_str(&format!("+{secs}s {m}\n"));
        }
    }
    out.push_str("\n--- engine.log (хвосты) ---\n");
    {
        let v = s.vkladki.lock().unwrap();
        for (id, _) in v.iter() {
            let log = brauzer::papka_profilej().join(id).join("engine.log");
            if let Ok(soderzhimoe) = std::fs::read_to_string(&log) {
                let hvost: Vec<&str> = soderzhimoe.lines().rev().take(5).collect();
                out.push_str(&format!("[{id}]\n"));
                for stroka in hvost.iter().rev() {
                    out.push_str(&format!("  {stroka}\n"));
                }
            }
        }
    }
    out
}

/// Указатель на наше окно в том виде, какого ждёт система.
fn nash_descriptor(w: &tauri::WebviewWindow) -> Option<u64> {
    #[cfg(target_os = "windows")]
    {
        w.hwnd().ok().map(|h| h.0 as usize as u64)
    }
    #[cfg(target_os = "linux")]
    {
        use gtk::prelude::*;
        let g = w.gtk_window().ok()?;
        let okno = g.window()?;
        // ⛔ Указатель X11 лежит в gdkx11, а не в gtk::gdk.
        let x11 = okno.downcast::<gdkx11::X11Window>().ok()?;
        Some(x11.xid() as u64)
    }
    #[cfg(not(any(target_os = "windows", target_os = "linux")))]
    {
        let _ = w;
        None
    }
}

/// Показать и поставить на место окно активной вкладки — после
/// сворачивания/возврата нашего окна: встроенное чужое окно из другой
/// очереди само за нами не следует (ловили 10.11.2026: «свернул и
/// больше не развернёшь»).
fn vosstanovit_dvizhok(app: &tauri::AppHandle) {
    let s: tauri::State<Sostoyanie> = app.state();
    let naideno = {
        let aktivnaya = s.aktivnaya.lock().unwrap().clone();
        let vkladki = s.vkladki.lock().unwrap();
        aktivnaya.and_then(|id| vkladki.get(&id).and_then(|vk| vk.okno.map(|o| (id, o))))
    };
    let Some((_id, okno)) = naideno else { return };
    let m = s.mesto.lock().unwrap().clone();
    let _ = sistema::pokazat(okno, true);
    if let Some(mm) = m {
        let _ = sistema::peremestit(okno, mm);
    }
    // ⛔ Фокус при возврате из другого приложения оседает на нашем
    // окне — вернём клавиатуру движку, иначе текст не набирается
    // (10.11.2026, поймал user).
    let _ = app.run_on_main_thread(move || sistema::fokus_dvizhku(okno));
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(Sostoyanie::default())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_process::init())
        // Размер, позиция и монитор окна запоминаются сами.
        .plugin(tauri_plugin_window_state::Builder::default().build())
        .setup(|app| {
            // Движки, оставшиеся от прошлого запуска, ломают новые:
            // Chromium пересылает команду старому процессу, а тот открывает
            // обычное окно поверх всего. Режем по уникальному имени exe.
            brauzer::pribit_proshlyh_dvizhkov();
            // Где лежит движок, положенный сборкой
            if let Ok(res) = app.path().resource_dir() {
                brauzer::zadat_papku_resursov(res);
            }
            // Сохранённые вкладки показываем сразу, но НЕ запускаем:
            // движок поднимется, когда на вкладку нажмут.
            {
                let s: tauri::State<Sostoyanie> = app.state();
                let mut v = s.vkladki.lock().unwrap();
                for n in zagruzit_spisok() {
                    v.insert(n.id.clone(), Vkladka { nastrojki: n, proc: None, okno: None, zapas_pid: None, nash_proxy: None, tunnel: None });
                }
            }
            // Самопроверка: NEXUSBRO_PROBA=1 сразу создаёт супер-вкладку,
            // чтобы весь путь (запуск → поиск окна → встраивание) можно было
            // проверить без рук.
            if std::env::var("NEXUSBRO_PROBA").as_deref() == Ok("1") {
                let h = app.handle().clone();
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_millis(1500));
                    let s: tauri::State<Sostoyanie> = h.state();
                    let n = Nastrojki {
                        id: "proba".into(),
                        nazvanie: "Проба".into(),
                        proxy: String::new(),
                        adres: "about:blank".into(),
                        rezat_reklamu: true,
                        pustaya: true,
                    };
                    let o = otkryt(h.clone(), s, n);
                    println!("САМОПРОВЕРКА: ok={} {}", o.ok, o.soobshchenie);
                });
            }
            // Сторож встроенных окон: Chromium иногда ПЕРЕСОЗДАЁТ своё
            // окно (новое — уже не наш потомок, с рамкой и минимизацией,
            // живёт своей жизнью поверх). Раз в пару секунд проверяем,
            // что окно каждой работающей вкладки всё ещё наше, и если
            // нет — ищем новое и встраиваем заново (10.11.2026, поймал
            // user: «окно внутри супервкладки живёт своей жизнью»).
            {
                let h = app.handle().clone();
                std::thread::spawn(move || loop {
                    std::thread::sleep(std::time::Duration::from_millis(1000));
                    if ZAVERSHENIE.load(std::sync::atomic::Ordering::SeqCst) {
                        return;
                    }
                    let Some(nashe) =
                        h.get_webview_window("main").and_then(|w| nash_descriptor(&w))
                    else {
                        continue;
                    };
                    let nado: Vec<(String, u32, u32)> = {
                        let s: tauri::State<Sostoyanie> = h.state();
                        let Ok(v) = s.vkladki.try_lock() else { continue };
                        v.iter()
                            .filter_map(|(id, vk)| {
                                // Движок живёт и по дочернему pid, и по
                                // запасному (перезапустивший себя Chromium):
                                // сторож смотрит оба варианта.
                                let real = vk
                                    .zapas_pid
                                    .or(vk.proc.as_ref().map(|p| p.id()))?;
                                let o = vk.okno?;
                                Some((id.clone(), real, o))
                            })
                            .collect()
                    };
                    for (id, pid, staroe) in nado {
                        // ⛔ Отслеживаем ДРУЖЕСКОЕ окно, а не «всё ли у
                        // старого хорошо»: Chromium пересоздаёт окно,
                        // зомби остаётся нашим потомком (nash_li=true),
                        // а живёт уже в новом — с рамкой и кнопками
                        // (10.11.2026, поймал user по кнопкам управления
                        // во встроенном хроме).
                        match sistema::najti_okno(pid, 1) {
                            Ok(novoe) if novoe != staroe => {
                                zapisat_v_zhurnal(format!("{id}: окно пересоздано ({staroe} → {novoe}), встраиваю"));
                                let s: tauri::State<Sostoyanie> = h.state();
                                let m = s
                                    .mesto
                                    .lock()
                                    .unwrap()
                                    .unwrap_or(Mesto { x: 0, y: 44, w: 1200, h: 700 });
                                let _ = sistema::vstroit(novoe, nashe, m);
                                if let Ok(mut v) = s.vkladki.try_lock() {
                                    if let Some(vk) = v.get_mut(&id) {
                                        vk.okno = Some(novoe);
                                    }
                                }
                                let _ = h.run_on_main_thread(move || {
                                    sistema::prisoedinit_vvod(novoe);
                                    sistema::fokus_dvizhku(novoe);
                                });
                                let _ = h.emit("vkladki-izmenilis", id.clone());
                            }
                            Ok(_) => {}
                            Err(_) => {
                                // ⛔ Спим ТОЛЬКО при мёртвом процессе: окно
                                // могло пересоздаться или пропасть на
                                // секунду (свип ниже его найдёт и встроит).
                                // Иначе хоронили живой движок — а его окно
                                // дальше жило своей жизнью поверх (12.11.2026:
                                // «движок перезапущен, окно нет», движок
                                // при этом писал в engine.log).
                                if !sistema::nash_li(staroe, nashe)
                                    && !sistema::pid_zhiv(pid)
                                {
                                    zapisat_v_zhurnal(format!("{id}: движок умер, вкладка спит"));
                                    let s: tauri::State<Sostoyanie> = h.state();
                                    if let Ok(mut v) = s.vkladki.try_lock() {
                                        if let Some(vk) = v.get_mut(&id) {
                                            vk.okno = None;
                                            vk.proc = None;
                                            if let Some(p) = vk.nash_proxy.take() {
                                                p.ostanovit();
                                            }
                                            if let Some(mut t) = vk.tunnel.take() {
                                                let _ = t.kill();
                                                let _ = t.wait();
                                            }
                                        }
                                    }
                                    let _ = h.emit("vkladki-izmenilis", id.clone());
                                }
                            }
                        }
                        // Плюс подметаем чужие окна этого движка: сессия
                        // с несколькими окнами, попапы, пересозданное окно.
                        // Если известное окно погибло — крупнейшее чужое
                        // становится главным.
                        let s2: tauri::State<Sostoyanie> = h.state();
                        if let Some(glavnoe) = vstroit_vse_okna(&h, &s2, &id, pid) {
                            if !sistema::nash_li(staroe, nashe) {
                                if let Ok(mut v) = s2.vkladki.try_lock() {
                                    if let Some(vk) = v.get_mut(&id) {
                                        vk.okno = Some(glavnoe);
                                    }
                                }
                            }
                        }
                    }
                });
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            spisok, aktivnaya, schet_rezki, otkryt, pereklyuchit, zakryt, izmenit,
            zadat_mesto, dvizhok_est, pokazat_dvizhki, nuzhno_sohranenie,
            sohranennye, vosstanovit, udalit_sohranennuyu, diagnostika, perezagruzit
        ])
        .build(tauri::generate_context!())
        .expect("NexusBro не запустился")
        .run(|app, sobytie| {
            // Восстановление встроенного окна после сворачивания/фокуса:
            // чужое окно само не возвращается — показываем и ставим на место.
            match &sobytie {
                tauri::RunEvent::WindowEvent { label, event: tauri::WindowEvent::Resized(r), .. }
                    if label == "main" && (r.width > 0 || r.height > 0) =>
                {
                    vosstanovit_dvizhok(app);
                }
                tauri::RunEvent::WindowEvent {
                    label,
                    event: tauri::WindowEvent::Focused(true),
                    ..
                } if label == "main" => {
                    vosstanovit_dvizhok(app);
                }
                _ => {}
            }
            static AVTOSTART_SDELAN: std::sync::atomic::AtomicBool =
                std::sync::atomic::AtomicBool::new(false);
            // Триггер — ПЕРВЫЙ фокус или первый непустой размер окна: под
            // Xvfb при старте приходит только Focused (10.11.2026).
            let okno_zhivo = matches!(
                &sobytie,
                tauri::RunEvent::WindowEvent {
                    label,
                    event: tauri::WindowEvent::Resized(r),
                    ..
                } if label == "main" && r.width > 0 && r.height > 0
            ) || matches!(
                &sobytie,
                tauri::RunEvent::WindowEvent {
                    label,
                    event: tauri::WindowEvent::Focused(true),
                    ..
                } if label == "main"
            );
            if okno_zhivo && !AVTOSTART_SDELAN.swap(true, std::sync::atomic::Ordering::SeqCst)
            {
                // При запуске поднимается ТОЛЬКО вкладка, которая была
                // активна при закрытии (10.11.2026, задал user). Остальные
                // спят до клика — вкладки не жрут память понапрасну.
                let h = app.clone();
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_millis(600));
                    // ⛔ Окно могло сохраниться свёрнутым (видели
                    // WS_MINIMIZE на машине user'а) — разворачиваем,
                    // иначе движок стартует в минимизированное окно.
                    if let Some(w) = h.get_webview_window("main") {
                        let _ = w.unminimize();
                    }
                    let Some(id) = zagruzit_aktivnuyu() else { return };
                    let est = {
                        let s: tauri::State<Sostoyanie> = h.state();
                        let v = s.vkladki.lock().unwrap();
                        v.contains_key(&id)
                    };
                    if !est {
                        return;
                    }
                    let s: tauri::State<Sostoyanie> = h.state();
                    let _ = zapustit_vnutri(&h, &s, &id);
                    let _ = h.emit("vkladki-izmenilis", id);
                });
            }
            // ⛔ При закрытии программы просим движки выйти ЧИСТО —
            // иначе сессии вкладок не восстановятся (проверено: kill →
            // exit_type=Crashed → вкладки пропадают). Обработчик Exit
            // и так крутится на нашем UI-потоке — открепляем ввод и
            // шлём WM_CLOSE/SIGTERM прямо здесь, без ожидания; кто не
            // успел — того добьёт задание/PDEATHSIG при выходе процесса.
            if let tauri::RunEvent::Exit = sobytie {
                ZAVERSHENIE.store(true, std::sync::atomic::Ordering::SeqCst);
                let spisok: Vec<(u32, Option<u32>)> = {
                    let s: tauri::State<Sostoyanie> = app.state();
                    let v = s.vkladki.lock().unwrap();
                    v.values()
                        .filter_map(|vk| {
                            let real = vk
                                .zapas_pid
                                .or(vk.proc.as_ref().map(|p| p.id()))?;
                            Some((real, vk.okno))
                        })
                        .collect()
                };
                for (pid, okno) in &spisok {
                    brauzer::poprosit_o_chem_vyhode(*pid, *okno);
                }
                // ⛔ Дать движкам время сбросить сессию: как только наш
                // процесс выйдет, задание мгновенно добьёт дерево — не
                // успевший записать сессию Chromium = крах = «восстановить?»
                // вместо вкладок (12.11.2026, жалоба «перезапуск ничего
                // не возвращает»).
                let srok = std::time::Instant::now();
                while spisok.iter().any(|(pid, _)| sistema::pid_zhiv(*pid))
                    && srok.elapsed().as_secs() < 4
                {
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
            }
        });
}
