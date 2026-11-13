//! NexusBro — оболочка с супер-вкладками поверх готового Chromium.
//!
//! Супер-вкладка = отдельный процесс движка со СВОИМ профилем и СВОИМ
//! выходом в сеть. Внутри неё человек открывает сколько угодно обычных
//! вкладок, и весь сервис со всеми его адресами идёт через нужный выход.

mod brauzer;
mod sessiya;
mod nastrojki;
mod cef_vkladki;
pub mod cef_dvizhok;
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
    /// Движок спавнится, хэндл окна ещё не получен (фоновый поток ждёт
    /// NB-OKNO). Пока true — повторный запуск не начинать.
    zapusk_idet: bool,
    zapusk_id: u64,
    kanal: Option<std::sync::mpsc::SyncSender<sessiya::Komanda>>,
    obychnye: Option<sessiya::Sessiya>,
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
    dvizhki_skryty: std::sync::atomic::AtomicBool,
    izmenenie_okon: Mutex<()>,
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
        z.push_back((std::time::Instant::now(), sobytie.clone()));
        while z.len() > 40 {
            z.pop_front();
        }
    }
    // ⛔ И на диск: при зависании in-memory журнал умирает вместе с
    // программой и бесполезен (12.11.2026, user). Пишем терпимо к
    // ошибкам: журнал не должен ломать основной путь.
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(brauzer::papka_profilej().parent().unwrap_or(std::path::Path::new(".")).join("zhurnal.log"))
    {
        use std::io::Write;
        let _ = writeln!(f, "{secs} {sobytie}");
    }
}

/// Границы команд — чтобы зависание находилось по последней строке файла.
fn v_zhurnal_granica(komanda: &str, id: &str, shag: &str) {
    zapisat_v_zhurnal(format!("{komanda}({id}): {shag}"));
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
    let okno = aktivnaya.and_then(|id| s.vkladki.lock().unwrap().get(&id).and_then(|v| v.okno));
    if let Some(o) = okno {
        let _ = sistema::peremestit(o, m);
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
            zapushchena: !x.zapusk_idet && (x.proc.is_some() || x.zapas_pid.is_some()),
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
    // ⛔ Старая проверка искала chromium/ungoogled-chromium — со схемой
    // CEF она врала «движок не найден», хотя файлы лежали рядом (12.11.2026,
    // промежуточная сборка ветки cef-migration). Ищем libcef рядом с exe
    // или в CEF_PATH.
    let ryadom = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|x| x.to_path_buf()))
        .unwrap_or_default();
    for baza in [ryadom, std::env::var("CEF_PATH").map(std::path::PathBuf::from).unwrap_or_default()] {
        if baza.join("libcef.dll").exists() || baza.join("libcef.so").exists() {
            return true;
        }
    }
    false
}

/// Спрятать/показать встроенные окна движков. Нужно лицевой части:
/// модальный диалог рисуется ВНУТРИ нашего окна, а поверх его области
/// лежит чужое окно движка — без скрытия диалог не видно (10.11.2026:
/// «добавление супервкладки не видно, пока не закроешь хром»).
/// ⛔ При скрытии прячем не только известные хэндлы, но ВСЕ окна наших
/// движков: пересозданное окно старым хэндлом не видно и лежит поверх,
/// поедая клики по диалогу.
#[tauri::command]
async fn pokazat_dvizhki(s: State<'_, Sostoyanie>, vidno: bool) -> Result<Otvet, String> {
    v_zhurnal_granica("pokazat_dvizhki", "-", &format!("vidno={vidno}"));
    let _okna = s.izmenenie_okon.lock().unwrap();
    s.dvizhki_skryty.store(!vidno, std::sync::atomic::Ordering::SeqCst);
    if !vidno {
        let pids: Vec<u32> = {
            let v = s.vkladki.lock().unwrap();
            v.values()
                .filter_map(|vk| vk.zapas_pid.or(vk.proc.as_ref().map(|p| p.id())))
                .collect()
        };
        sistema::spryatat_chuzhie(&pids);
    }
    primenit_vidimost(&s);
    v_zhurnal_granica("pokazat_dvizhki", "-", "готово");
    Ok(da())
}

#[tauri::command]
async fn otkryt(
    app: tauri::AppHandle,
    s: State<'_, Sostoyanie>,
    nastrojki: Nastrojki,
) -> Result<Otvet, String> {
    let id = nastrojki.id.clone();
    v_zhurnal_granica("otkryt", &id, "вход");
    {
        let mut v = s.vkladki.lock().unwrap();
        if v.contains_key(&id) {
            return Ok(net("такая супер-вкладка уже есть"));
        }
        // Запоминаем СРАЗУ: если движок не запустится, вкладка всё равно
        // останется в списке, и человек сможет поправить настройки.
        v.insert(
            id.clone(),
            Vkladka { nastrojki: nastrojki.clone(), proc: None, okno: None, zapas_pid: None, nash_proxy: None, tunnel: None, zapusk_idet: false, zapusk_id: 0, kanal: None, obychnye: None },
        );
        let vse: Vec<Nastrojki> = v.values().map(|x| x.nastrojki.clone()).collect();
        sohranit_spisok(&vse);
    }
    Ok(zapustit_vnutri(&app, &s, &id).await)
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
async fn zapustit_vnutri(app: &tauri::AppHandle, s: &State<'_, Sostoyanie>, id: &str) -> Otvet {
    static SLEDUYUSHCHIJ_ZAPUSK: std::sync::atomic::AtomicU64 =
        std::sync::atomic::AtomicU64::new(1);
    let (nastrojki, zapusk_id) = {
        let mut v = s.vkladki.lock().unwrap();
        let Some(vk) = v.get_mut(id) else { return net("такой супер-вкладки нет") };
        if vk.zapusk_idet || vk.proc.is_some() {
            drop(v);
            pereklyuchit_vnutri(s, id);
            return da();
        }
        vk.zapusk_idet = true;
        vk.zapusk_id = SLEDUYUSHCHIJ_ZAPUSK.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        (vk.nastrojki.clone(), vk.zapusk_id)
    };
    pereklyuchit_vnutri(s, id);
    let otvet = podgotovit_dvizhok(app, s, id, nastrojki, zapusk_id).await;
    if !otvet.ok {
        let mut v = s.vkladki.lock().unwrap();
        if let Some(vk) = v.get_mut(id).filter(|vk| vk.zapusk_id == zapusk_id) {
            vk.zapusk_idet = false;
        }
    }
    otvet
}

async fn podgotovit_dvizhok(
    app: &tauri::AppHandle, s: &State<'_, Sostoyanie>, id: &str,
    nastrojki: Nastrojki, zapusk_id: u64,
) -> Otvet {
    // ⛔ Движок всегда ходит через НАШ прокси, даже при прямом выходе:
    // иначе в прямом режиме не будет резки рекламы.
    // Выход бывает трёх видов: подписка или ссылка на узел (поднимаем
    // туннель), SOCKS5 и веб-прокси (идём напрямую в них).
    let mut tunnel: Option<std::process::Child> = None;
    let vyhod = if vpn::eto_podpiska(&nastrojki.proxy) {
        let papka = brauzer::papka_profilej().join(id);
        let zapis = nastrojki.proxy.trim().to_string();
        let itog = async {
            // ссылка подписки — скачиваем и берём первый узел
            let ssylka = if zapis.starts_with("http") {
                let uzly = vpn::skachat_podpisku(&zapis).await?;
                uzly[0].ssylka.clone()
            } else {
                zapis.clone()
            };
            vpn::podnyat_tunnel(&papka, &ssylka).await
        }
        .await;
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
    let nash = proxy::podnyat(proxy::Nastrojka {
        vyhod,
        rezat_reklamu: nastrojki.rezat_reklamu,
    })
    .await;
    let nash = match nash {
        Ok(p) => p,
        Err(e) => return net(format!("свой прокси не поднялся: {}", e)),
    };
    let cherez = format!("http://127.0.0.1:{}", nash.port);

    // Стартовый адрес подаём только для режима «открывать пустой»:
    // восстановление обычных вкладок переезжает в наш JSON (этап 3).
    let adres = if nastrojki.pustaya {
        Some(nastrojki.adres.as_str())
    } else {
        None
    };

    // CEF-движок: процесс супер-вкладки рождает окно ДОЧЕРНИМ к нашему.
    // Сирот по определению нет: процесс привязан к нам заданием (Windows)
    // или PDEATHSIG (Linux) и умирает вместе с программой. Подметание
    // сирот и повторные запуски уходят в прошлое вместе со старой схемой.
    let nashe = match app.get_webview_window("main").and_then(|w| nash_descriptor(&w)) {
        Some(h) => h,
        None => {
            nash.ostanovit();
            if let Some(mut t) = tunnel {
                let _ = t.kill();
                let _ = t.wait();
            }
            return net("не получил указатель на своё окно");
        }
    };
    let m = s.mesto.lock().unwrap().unwrap_or(Mesto { x: 0, y: 44, w: 1200, h: 700 });
    let mut ditya = match cef_dvizhok::zapustit_cep(id, &cherez, adres, nashe, m) {
        Ok(d) => d,
        Err(e) => {
            nash.ostanovit();
            if let Some(mut t) = tunnel {
                let _ = t.kill();
                let _ = t.wait();
            }
            return net(format!("движок не запустился: {e}"));
        }
    };

    let stdout = ditya.stdout.take().unwrap();
    let kanal = cef_dvizhok::kanal_komand(ditya.stdin.take().unwrap());
    {
        let mut v = s.vkladki.lock().unwrap();
        match v.get_mut(id).filter(|vk| vk.zapusk_id == zapusk_id && vk.zapusk_idet) {
            Some(vk) => {
                vk.proc = Some(ditya);
                vk.kanal = Some(kanal);
                vk.nash_proxy = Some(nash);
                vk.tunnel = tunnel;
            }
            None => {
                drop(v);
                ostanovit_resursy(Some(ditya), Some(nash), tunnel);
                return da();
            }
        }
    }
    let h = app.clone();
    let id_fon = id.to_string();
    std::thread::spawn(move || {
        let h_sessii = h.clone();
        let id_sessii = id_fon.clone();
        let okno = cef_dvizhok::dozhdatsya_okna(stdout, 25, move |sessiya| {
            let _zapis = sessiya::ZAPIS.lock().unwrap();
            let s: State<Sostoyanie> = h_sessii.state();
            {
                let mut v = s.vkladki.lock().unwrap();
                let Some(vk) = v.get_mut(&id_sessii).filter(|vk| vk.zapusk_id == zapusk_id && vk.proc.is_some()) else { return };
                vk.obychnye = Some(sessiya.clone());
            }
            if let Err(e) = sessiya.sohranit(&brauzer::papka_profilej().join(&id_sessii)) {
                zapisat_v_zhurnal(format!("{id_sessii}: сессия не записана: {e}"));
            }
            let _ = h_sessii.emit("obychnye-izmenilis", serde_json::json!({"id":id_sessii,"sessiya":sessiya}));
        });
        let s: State<Sostoyanie> = h.state();
        let mut v = s.vkladki.lock().unwrap();
        let Some(vk) = v.get_mut(&id_fon)
            .filter(|vk| vk.zapusk_id == zapusk_id && vk.zapusk_idet)
        else { return };
        vk.zapusk_idet = false;
        if let Some(okno) = okno {
            vk.okno = Some(okno);
            drop(v);
            {
                let _okna = s.izmenenie_okon.lock().unwrap();
                primenit_vidimost(&s);
            }
            zapisat_v_zhurnal(format!("{id_fon}: запуск ок, окно {okno}"));
        } else {
            let resursy = (vk.proc.take(), vk.nash_proxy.take(), vk.tunnel.take());
            drop(v);
            ostanovit_resursy(resursy.0, resursy.1, resursy.2);
            zapisat_v_zhurnal(format!("{id_fon}: движок не ответил за 25 с (см. cef.log)"));
            let _ = h.emit("oshibka-dvizhka", format!("{id_fon}: движок не ответил за 25 с (см. cef.log)"));
        }
        let _ = h.emit("vkladki-izmenilis", "");
    });
    da()
}

#[tauri::command]
async fn pereklyuchit(app: tauri::AppHandle, s: State<'_, Sostoyanie>, id: String) -> Result<Otvet, String> {
    v_zhurnal_granica("pereklyuchit", &id, "вход");
    let zapushchena = match s.vkladki.lock().unwrap().get_mut(&id) {
        Some(vk) => {
            // ⛔ Дочерний процесс — «пусковая ракета»: Chromium на Windows
            // перезапускает себя и стартовый процесс выходит. Смерть
            // дочернего ≠ смерть движка — движок жив, пока живо ЕГО окно
            // и его реальный pid. Хоронили живой движок и перезапускали
            // его — а подметание при перезапуске резало соседнюю вкладку
            // (12.11.2026, диагностика user'а: «мёртвый движок при
            // переключении» через 2 с после успешного запуска).
            if vk.zapusk_idet {
                // Движок спавнится прямо сейчас — считаем запущенной,
                // повторный спавн не начинаем.
                true
            } else {
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
        }
        None => return Ok(net("такой супер-вкладки нет")),
    };
    // Вкладку поднимаем в тот момент, когда на неё нажали, а не при запуске
    // программы — иначе десять вкладок сразу съедят всю память.
    if !zapushchena {
        return Ok(zapustit_vnutri(&app, &s, &id).await);
    }
    pereklyuchit_vnutri(&s, &id);
    // ⛔ SetFocus в чужое окно с главного потока на Windows может повиснуть
    // вместе с потоком движка (12.11.2026, «виснет в рандомные моменты»).
    // Фокус движку отдаст клик мышью — штатно.
    Ok(da())
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

fn primenit_vidimost(s: &State<Sostoyanie>) {
    let m = *s.mesto.lock().unwrap();
    let aktivnaya = s.aktivnaya.lock().unwrap().clone();
    let skryty = s.dvizhki_skryty.load(std::sync::atomic::Ordering::SeqCst);
    let okna: Vec<_> = s.vkladki.lock().unwrap().iter()
        .filter_map(|(id, vk)| vk.okno.map(|o| (id.clone(), o))).collect();
    for (id, o) in okna {
        let vidno = !skryty && Some(&id) == aktivnaya.as_ref();
        let _ = sistema::pokazat(o, vidno);
        if vidno {
            if let Some(m) = m { let _ = sistema::peremestit(o, m); }
        }
    }
}

fn pereklyuchit_vnutri(s: &State<Sostoyanie>, id: &str) {
    let _okna = s.izmenenie_okon.lock().unwrap();
    if !s.vkladki.lock().unwrap().contains_key(id) { return }
    *s.aktivnaya.lock().unwrap() = Some(id.to_string());
    primenit_vidimost(s);
    sohranit_aktivnuyu(id);
}

fn ostanovit_resursy(
    proc: Option<Child>, proxy: Option<proxy::Rabotayushchij>, tunnel: Option<Child>,
) {
    if let Some(p) = proc {
        cef_dvizhok::ostanovit_process(p);
    }
    if let Some(p) = proxy {
        p.ostanovit();
    }
    if let Some(mut t) = tunnel {
        let _ = t.kill();
        let _ = t.wait();
    }
}

async fn ostanovit_vnutri(s: &State<'_, Sostoyanie>, id: &str) {
    let resursy = {
        let mut v = s.vkladki.lock().unwrap();
        let Some(vk) = v.get_mut(id) else { return };
        vk.zapusk_idet = false;
        vk.kanal = None;
        vk.okno = None;
        vk.zapas_pid = None;
        (vk.proc.take(), vk.nash_proxy.take(), vk.tunnel.take())
    };
    let _ = tauri::async_runtime::spawn_blocking(move || {
        ostanovit_resursy(resursy.0, resursy.1, resursy.2);
    }).await;
}

/// Правка настроек супер-вкладки после создания. Работающую перезапускаем
/// с новыми настройками (прокси и резка рекламы задаются на запуск),
/// остановленную просто перезаписываем.
#[tauri::command]
async fn izmenit(app: tauri::AppHandle, s: State<'_, Sostoyanie>, nastrojki: Nastrojki) -> Result<Otvet, String> {
    v_zhurnal_granica("izmenit", &nastrojki.id, "вход");
    let rabotaet = {
        let mut v = s.vkladki.lock().unwrap();
        match v.get_mut(&nastrojki.id) {
            Some(vk) => {
                vk.nastrojki = nastrojki.clone();
                vk.proc.is_some() || vk.zapusk_idet
            }
            None => return Ok(net("такой супер-вкладки нет")),
        }
    };
    {
        let v = s.vkladki.lock().unwrap();
        let vse: Vec<Nastrojki> = v.values().map(|x| x.nastrojki.clone()).collect();
        sohranit_spisok(&vse);
    }
    if rabotaet {
        ostanovit_vnutri(&s, &nastrojki.id).await;
        return Ok(zapustit_vnutri(&app, &s, &nastrojki.id).await);
    }
    let _ = app.emit("vkladki-izmenilis", nastrojki.id);
    Ok(da())
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
    let _zapis = sessiya::ZAPIS.lock().unwrap();
    let put = brauzer::papka_profilej().join(id);
    for _ in 0..5 {
        match std::fs::remove_dir_all(&put) {
            Ok(()) => return,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
            Err(_) => {}
        }
        std::thread::sleep(std::time::Duration::from_millis(400));
    }
    let _ = std::fs::remove_dir_all(&put);
}

/// Закрыть супер-вкладку. `sohranit` — true: профиль и настройки кладём
/// в список сохранённых (открыть заново через меню у «+»); false: профиль
/// выбрасывается насовсем. Подтверждение спрашивает лицевая часть.
#[tauri::command]
async fn zakryt(app: tauri::AppHandle, s: State<'_, Sostoyanie>, id: String, sohranit: bool) -> Result<Otvet, String> {
    let udalena = {
        let mut v = s.vkladki.lock().unwrap();
        v.remove(&id)
    };
    let Some(vk) = udalena else {
        return Ok(net("такой супер-вкладки нет"));
    };
    v_zhurnal_granica("zakryt", &id, "останавливаю движок");
    let nastrojki = vk.nastrojki;
    let _ = tauri::async_runtime::spawn_blocking(move || {
        ostanovit_resursy(vk.proc, vk.nash_proxy, vk.tunnel);
    }).await;
    v_zhurnal_granica("zakryt", &id, "движок остановлен");
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
        let id_profilya = id.clone();
        let _ = tauri::async_runtime::spawn_blocking(move || steret_profil(&id_profilya)).await;
    }
    let nado_pereklyuchit = {
        let mut aktivnaya = s.aktivnaya.lock().unwrap();
        if aktivnaya.as_deref() == Some(&id) {
            *aktivnaya = None;
            true
        } else { false }
    };
    let sleduyushchaya = if nado_pereklyuchit {
        s.vkladki.lock().unwrap().keys().min().cloned()
    } else { None };
    if let Some(id) = sleduyushchaya {
        let _ = zapustit_vnutri(&app, &s, &id).await;
    }
    let _ = app.emit("vkladki-izmenilis", "");
    Ok(da())
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
async fn vosstanovit(app: tauri::AppHandle, s: State<'_, Sostoyanie>, id: String) -> Result<Otvet, String> {
    let nastrojki = {
        let mut sohr = zagruzit_sohranennye();
        let n = match sohr.iter().position(|n| n.id == id) {
            Some(i) => sohr.remove(i),
            None => return Ok(net("такой сохранённой вкладки нет")),
        };
        sohranit_sohranennye(&sohr);
        n
    };
    {
        let mut v = s.vkladki.lock().unwrap();
        if v.contains_key(&id) {
            return Ok(net("такая супер-вкладка уже открыта"));
        }
        v.insert(id.clone(), Vkladka { nastrojki: nastrojki.clone(), proc: None, okno: None, zapas_pid: None, nash_proxy: None, tunnel: None, zapusk_idet: false, zapusk_id: 0, kanal: None, obychnye: None });
        let vse: Vec<Nastrojki> = v.values().map(|x| x.nastrojki.clone()).collect();
        sohranit_spisok(&vse);
    }
    Ok(zapustit_vnutri(&app, &s, &id).await)
}

/// Выбросить сохранённую супер-вкладку насовсем: настройки и профиль
/// со всеми входами. Подтверждение спрашивает лицевая часть.
#[tauri::command]
async fn udalit_sohranennuyu(id: String) -> Result<Otvet, String> {
    tauri::async_runtime::spawn_blocking(move || {
    let mut sohr = zagruzit_sohranennye();
    if sohr.iter().any(|n| n.id == id) {
        sohr.retain(|n| n.id != id);
        sohranit_sohranennye(&sohr);
        steret_profil(&id);
        da()
    } else {
        net("такой сохранённой вкладки нет")
    }
    }).await.map_err(|e| e.to_string())
}

/// Перезагрузить движок вкладки (контекстное меню вкладки).
#[tauri::command]
async fn perezagruzit(app: tauri::AppHandle, s: State<'_, Sostoyanie>, id: String) -> Result<Otvet, String> {
    if !s.vkladki.lock().unwrap().contains_key(&id) {
        return Ok(net("такой супер-вкладки нет"));
    }
    ostanovit_vnutri(&s, &id).await;
    Ok(zapustit_vnutri(&app, &s, &id).await)
}

#[tauri::command]
async fn fokus_obolochke(window: tauri::WebviewWindow) -> Result<(),String> {
    #[cfg(target_os = "linux")]
    {
        let nashe = nash_descriptor(&window).ok_or("не найдено окно оболочки")?;
        sistema::fokus_obolochke(nashe).map_err(|e|e.to_string())
    }
    #[cfg(not(target_os = "linux"))]
    {
        window.set_focus().map_err(|e|e.to_string())?;
        let webview: &tauri::Webview = window.as_ref();
        webview.set_focus().map_err(|e|e.to_string())
    }
}

#[tauri::command]
async fn obychnye_spisok(s: State<'_, Sostoyanie>, id: String) -> Result<sessiya::Sessiya,String> {
    {
        let v = s.vkladki.lock().unwrap();
        let vk = v.get(&id).ok_or("такой супер-вкладки нет")?;
        if let Some(sessiya) = &vk.obychnye { return Ok(sessiya.clone()) }
    }
    tauri::async_runtime::spawn_blocking(move || sessiya::Sessiya::zagruzit(&brauzer::papka_profilej().join(id)))
        .await.map_err(|e| e.to_string())
}

#[tauri::command]
async fn obychnaya_komanda(s: State<'_, Sostoyanie>, id: String, komanda: sessiya::Komanda) -> Result<(),String> {
    let kanal = s.vkladki.lock().unwrap().get(&id).and_then(|v|v.kanal.clone())
        .ok_or("движок ещё не запущен")?;
    kanal.try_send(komanda).map_err(|e| match e {
        std::sync::mpsc::TrySendError::Full(_) => "Движок занят. Повторите действие.".into(),
        std::sync::mpsc::TrySendError::Disconnected(_) => "Движок остановлен. Откройте супер-вкладку снова.".into(),
    })
}

/// Диагностика по живому: состояние вкладок, окна, журнал сторожа,
/// хвосты engine.log. Человек копирует и присылает — разбираем факты,
/// а не гадания (10.11.2026).
#[tauri::command]
async fn diagnostika(app: tauri::AppHandle, s: State<'_, Sostoyanie>) -> Result<String, String> {
    v_zhurnal_granica("diagnostika", "-", "вход");
    let mut out = format!("=== NexusBro диагностика {} ({}) ===\n", env!("CARGO_PKG_VERSION"), std::env::consts::OS);
    let nashe = app
        .get_webview_window("main")
        .and_then(|w| nash_descriptor(&w))
        .map(|h| h.to_string())
        .unwrap_or_else(|| "нет".into());
    out.push_str(&format!("наше окно: {nashe}\n"));
    out.push_str(&format!("активная: {:?}, место: {:?}, скрыты для диалога: {}\n",
        s.aktivnaya.lock().unwrap().clone(), *s.mesto.lock().unwrap(),
        s.dvizhki_skryty.load(std::sync::atomic::Ordering::Acquire)));
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
                    format!("{o} (наше: {nash})\n    {}", okna::opisanie_okna(o))
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
    out.push_str("\n--- zhurnal.log (живой журнал с диска, хвост) ---\n");
    {
        let f = brauzer::papka_profilej().parent().unwrap_or(std::path::Path::new(".")).join("zhurnal.log");
        if let Ok(soderzhimoe) = std::fs::read_to_string(&f) {
            let hvost: Vec<&str> = soderzhimoe.lines().rev().take(15).collect();
            for stroka in hvost.iter().rev() {
                out.push_str(&format!("  {stroka}\n"));
            }
        }
    }
    out.push_str("\n--- cef.log (хвосты, процессы движка) ---\n");
    {
        let v = s.vkladki.lock().unwrap();
        for (id, _) in v.iter() {
            let log = brauzer::papka_profilej().join(id).join("cef.log");
            if let Ok(soderzhimoe) = std::fs::read_to_string(&log) {
                let hvost: Vec<&str> = soderzhimoe.lines().rev().take(8).collect();
                out.push_str(&format!("[{id}] {}\n", log.display()));
                for stroka in hvost.iter().rev() {
                    out.push_str(&format!("  {stroka}\n"));
                }
            }
        }
    }
    out.push_str("\n--- cef-engine.log (актуальный журнал CEF) ---\n");
    let ids: Vec<_> = s.vkladki.lock().unwrap().keys().cloned().collect();
    for id in ids {
        let log = brauzer::papka_profilej().join(&id).join("cef-engine.log");
        if let Ok(text) = std::fs::read_to_string(log) {
            out.push_str(&format!("[{id}]\n"));
            for line in text.lines().rev().take(15).collect::<Vec<_>>().into_iter().rev() {
                out.push_str(&format!("  {line}\n"));
            }
        }
    }
    out.push_str("\n--- engine.log (старый Chromium, может быть от прошлой версии) ---\n");
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
    Ok(out)
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
    if let Ok(_okna) = s.izmenenie_okon.try_lock() {
        primenit_vidimost(&s);
    };

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
                    v.insert(n.id.clone(), Vkladka { nastrojki: n, proc: None, okno: None, zapas_pid: None, nash_proxy: None, tunnel: None, zapusk_idet: false, zapusk_id: 0, kanal: None, obychnye: None });
                }
            }
            // Самопроверка: NEXUSBRO_PROBA=1 сразу создаёт супер-вкладку,
            // чтобы весь путь (запуск → поиск окна → встраивание) можно было
            // проверить без рук.
            let proba = std::env::var("NEXUSBRO_PROBA").unwrap_or_default();
            if proba == "1" || proba == "2" {
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
                    let o = tauri::async_runtime::block_on(otkryt(h.clone(), s, n))
                        .unwrap_or_else(|e| net(e));
                    println!("САМОПРОВЕРКА: ok={} {}", o.ok, o.soobshchenie);
                    // СЦЕНАРИЙ 2: воспроизводим жалобы user'а — вторая
                    // вкладка и удаление (висло на машине, 12.11.2026).
                    if proba == "2" {
                        let shag = |t: &str| println!("СЦЕНАРИЙ: {t}");
                        std::thread::sleep(std::time::Duration::from_secs(4));
                        shag("создаю вторую…");
                        let n2 = Nastrojki {
                            id: "proba2".into(),
                            nazvanie: "Проба2".into(),
                            proxy: String::new(),
                            adres: "about:blank".into(),
                            rezat_reklamu: true,
                            pustaya: true,
                        };
                        let s2: tauri::State<Sostoyanie> = h.state();
                        let o2 = tauri::async_runtime::block_on(otkryt(h.clone(), s2, n2))
                            .unwrap_or_else(|e| net(e));
                        println!("СЦЕНАРИЙ: вторая ok={} {}", o2.ok, o2.soobshchenie);
                        std::thread::sleep(std::time::Duration::from_secs(4));
                        shag("удаляю вторую…");
                        let s3: tauri::State<Sostoyanie> = h.state();
                        let o3 = tauri::async_runtime::block_on(zakryt(h.clone(), s3, "proba2".into(), false))
                            .unwrap_or_else(|e| net(e));
                        println!("СЦЕНАРИЙ: удалена ok={} {}", o3.ok, o3.soobshchenie);
                        std::thread::sleep(std::time::Duration::from_secs(3));
                        shag("удаляю первую…");
                        let s4: tauri::State<Sostoyanie> = h.state();
                        let o4 = tauri::async_runtime::block_on(zakryt(h.clone(), s4, "proba".into(), true))
                            .unwrap_or_else(|e| net(e));
                        println!("СЦЕНАРИЙ: удалена ok={} {}", o4.ok, o4.soobshchenie);
                        shag("ГОТОВ");
                    }
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
                    let s: State<Sostoyanie> = h.state();
                    let mertvye: Vec<_> = {
                        let Ok(mut v) = s.vkladki.try_lock() else { continue };
                        v.iter_mut().filter_map(|(id, vk)| {
                            if vk.zapusk_idet { return None }
                            let umer = matches!(vk.proc.as_mut().map(|p| p.try_wait()), Some(Ok(Some(_))));
                            if !umer { return None }
                            vk.okno = None;
                            vk.kanal = None;
                            vk.zapas_pid = None;
                            Some((id.clone(), vk.proc.take(), vk.nash_proxy.take(), vk.tunnel.take()))
                        }).collect()
                    };
                    for (id, proc, proxy, tunnel) in mertvye {
                        ostanovit_resursy(proc, proxy, tunnel);
                        zapisat_v_zhurnal(format!("{id}: движок умер, вкладка спит"));
                        let _ = h.emit("vkladki-izmenilis", "");
                    }
                });
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            nastrojki::nastrojki_poluchit, nastrojki::nastrojki_sohranit,
            fokus_obolochke, obychnye_spisok, obychnaya_komanda, spisok, aktivnaya, schet_rezki, otkryt, pereklyuchit, zakryt, izmenit,
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
                    let _ = tauri::async_runtime::block_on(zapustit_vnutri(&h, &s, &id));
                    let _ = h.emit("vkladki-izmenilis", id);
                });
            }
            // При закрытии программы гасим процессы супер-вкладок.
            // CEF-процессы — наши дети: задание (Windows) и PDEATHSIG
            // (Linux) добьют остатки дерева сразу после нашего выхода.
            if let tauri::RunEvent::Exit = sobytie {
                ZAVERSHENIE.store(true, std::sync::atomic::Ordering::SeqCst);
                let deti: Vec<std::process::Child> = {
                    let s: tauri::State<Sostoyanie> = app.state();
                    let mut v = s.vkladki.lock().unwrap();
                    v.values_mut()
                        .filter_map(|vk| vk.proc.take())
                        .collect()
                };
                for mut d in deti {
                    let _ = d.kill();
                    let _ = d.wait();
                }
            }
        });
}
