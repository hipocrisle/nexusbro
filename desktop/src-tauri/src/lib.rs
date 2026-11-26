//! NexusBro — оболочка с супер-вкладками поверх готового Chromium.
//!
//! Супер-вкладка = отдельный процесс движка со СВОИМ профилем и СВОИМ
//! выходом в сеть. Внутри неё человек открывает сколько угодно обычных
//! вкладок, и весь сервис со всеми его адресами идёт через нужный выход.

#[cfg(target_os = "windows")]
mod cef_events;
mod popup_menu;
#[cfg(target_os = "linux")]
mod gtk_tooltip_input;
mod browser_lock;
mod browser_lock_intent;
mod import_brauzera;
mod razdelitel;
mod fon_konteynera;
mod okna_app;
mod brauzer;
mod sessiya;
mod app_shutdown_protocol;
mod cef_app_shutdown;
mod app_shutdown_parent;
mod window_recovery;
mod tab_search;
mod browser_tasks;
mod browser_backup;
mod tab_routes;
mod youtube_light;
mod youtube_light_http;
mod youtube_light_sidecar;
mod youtube_light_extract;
mod youtube_light_captions;
mod youtube_light_components;
mod youtube_light_download;
mod youtube_light_mux;
mod youtube_light_jobs;
mod youtube_light_resume;
mod youtube_light_reload;
mod youtube_light_resume_service;
mod youtube_light_aux;
mod cef_youtube_resume;
mod cef_youtube_resume_fixture;
mod cef_files;
mod cef_youtube;
mod cef_youtube_captions;
mod cef_youtube_download;
mod cef_youtube_focus;
mod cef_focus_diagnostics;
mod cef_youtube_fullscreen;
mod youtube_fullscreen_parent;
mod youtube_download_parent;
mod youtube_download_diagnostics;
mod cef_youtube_probe;
mod cef_youtube_reference_probe;
mod cef_youtube_decode_probe;
mod cef_youtube_benchmark;
mod youtube_benchmark_fixture;
mod cef_youtube_stop_probe;
mod cef_youtube_privacy;
mod tab_route_parent;
mod route_network;
mod nastrojki;
mod cef_vkladki;
mod cef_route_context;
mod cdp;
mod super_lifecycle;
mod super_process_owner;
mod super_hibernation_parent;
mod super_hibernation_protocol;
mod super_cookie_escrow;
mod cef_cookie_transfer;
mod cef_hibernation_probe;
mod memory_guard;
mod memory_cef;
mod memory_bootstrap;
mod sync_receipts;
mod cef_memory_renderer;
#[cfg(target_os = "windows")]
mod cef_visibility_win;
mod cef_interfeys;
pub mod cef_dvizhok;
mod okna;
mod proxy;
mod vpn;
#[cfg(target_os = "windows")]
mod win32_oblast;
mod spisok_reklamy;
mod borba_s_reklamoy;
mod poisk;
mod devtools;
mod perevod;
mod privatnost;
mod biblioteka;
mod dannye_saytov;
mod dannye_statistika;
mod secure_vault;
mod browser_crypto;
mod browser_account;
mod browser_sync;
mod password_cef;
mod passwords;
mod zagruzki;
mod downloads_open_policy;
mod set_status;
mod supervkladki;

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
    pub sohranena: bool,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Nastrojki {
    pub id: String,
    pub nazvanie: String,
    #[serde(default)]
    pub cvet: String,
    #[serde(default)]
    pub poryadok: i64,
    /// Пусто = прямое соединение. Иначе вид `socks5://127.0.0.1:1080`.
    #[serde(default)]
    pub proxy: String,
    /// Выбранная ссылка из подписки. Пусто — первый узел (старые профили).
    #[serde(default)]
    pub proxy_uzel: String,
    #[serde(default = "domashnyaya")]
    pub adres: String,
    /// Резать рекламу в этой супер-вкладке.
    #[serde(default = "da_po_umolchaniyu")]
    pub rezat_reklamu: bool,
    /// Открывать пустой: не восстанавливать вкладки прошлой сессии.
    #[serde(default)]
    pub pustaya: bool,
    #[serde(default)]
    pub privatnaya: bool,
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
    if let Ok(s) = serde_json::to_string_pretty(&v) {
        let _ = std::fs::write(f, s);
    }
}

fn sohranit_sohranennye(v: &[Nastrojki]) -> Result<(), String> {
    let f = fajl_sohranennyh();
    let d = f.parent().ok_or_else(|| "Нет каталога профилей".to_string())?;
    std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
    let tmp = f.with_extension("tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(&v).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    std::fs::rename(tmp, f).map_err(|e| e.to_string())
}

struct Vkladka {
    nastrojki: Nastrojki,
    /// Свой прокси этой вкладки: режет рекламу и уводит в нужный выход.
    nash_proxy: Option<proxy::Rabotayushchij>,
    inherit_route: Option<tab_routes::NativeRoute>,
    route_settings: Option<Nastrojki>,
    /// Туннель вкладки, если выход задан подпиской или ссылкой на узел.
    tunnel: Option<std::process::Child>,
    /// None = вкладка сохранена, но движок не запущен.
    /// ⛔ Запускать все сразу нельзя: каждый экземпляр ест сотни мегабайт,
    /// десять вкладок — несколько гигабайт на пустом месте.
    proc: Option<super_process_owner::ProcessOwner>,
    hibernate:super_hibernation_parent::Node,
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
    routes: route_network::Registry,
    okna:okna_app::Okna,
    #[cfg(target_os = "windows")]
    obolochka: std::sync::OnceLock<tauri::WebviewWindow>,
    #[cfg(target_os = "windows")]
    oblast_pokolenie: std::sync::Arc<std::sync::atomic::AtomicU64>,
    vkladki: Mutex<HashMap<String, Vkladka>>,
    /// Где на экране показывать движок — это место нам сообщает лицевая часть.
    mesto: Mutex<Option<Mesto>>,
    aktivnaya: Mutex<Option<String>>,
    dvizhki_skryty: std::sync::atomic::AtomicBool,
    izmenenie_okon: Mutex<()>,
}

/// Выставляется при завершении программы: фоновые потоки (сторож окон)
/// обязаны остановиться, чтобы не мешать гашению движков.
// Rows removed by a user deletion still own live native writers in a worker.
// Cancellation/panic without an actual joined completion must retain this gate.
static OWNERSHIP_REMOVALS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
struct DeletionGuard { stopped: bool }
impl DeletionGuard {
    fn new() -> Self { OWNERSHIP_REMOVALS.fetch_add(1,std::sync::atomic::Ordering::SeqCst); Self { stopped:false } }
}
impl Drop for DeletionGuard {
    fn drop(&mut self) { if self.stopped { OWNERSHIP_REMOVALS.fetch_sub(1,std::sync::atomic::Ordering::SeqCst); } }
}
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
async fn zadat_mesto(app:tauri::AppHandle,window:tauri::WebviewWindow,s: State<'_, Sostoyanie>, x: i32, y: i32, w: u32, h: u32) -> Result<Otvet, String> {
    if window.label()!="main"{
        if let Some(extra)=s.okna.0.lock().unwrap().get_mut(window.label()){extra.mesto=Some(Mesto{x,y,w,h});}
        okna_app::obolochka(&app,&s,window.label());return Ok(da());
    }
    let _okna = s.izmenenie_okon.lock().unwrap();
    *s.mesto.lock().unwrap() = Some(Mesto { x, y, w, h });
    // Область WebView2 должна следовать и за CSS-resize панели, не только HWND resize.
    primenit_vidimost(&s);
    Ok(da())
}

#[tauri::command]
async fn spisok(s: State<'_, Sostoyanie>) -> Result<Vec<Vidno>, String> {
    let mut r: Vec<Vidno> = s.vkladki.lock().map_err(|e| e.to_string())?.values().map(|v| Vidno {
        nastrojki: v.nastrojki.clone(), zapushchena: !v.zapusk_idet && (v.proc.is_some() || v.zapas_pid.is_some()), sohranena: false,
    }).collect();
    let saved = tauri::async_runtime::spawn_blocking(zagruzit_sohranennye).await.map_err(|e| e.to_string())?;
    for v in &mut r { v.sohranena = saved.iter().any(|n| n.id == v.nastrojki.id); }
    r.sort_by(|a, b| (a.nastrojki.poryadok, &a.nastrojki.id).cmp(&(b.nastrojki.poryadok, &b.nastrojki.id)));
    Ok(r)
}

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
fn zhurnal_rezki(s: State<Sostoyanie>, id: String) -> proxy::ZhurnalRezki {
    let data = s.vkladki.lock().unwrap().get(&id).map(|v| (v.nastrojki.rezat_reklamu, v.nash_proxy.as_ref().map(|p| p.schet.clone())));
    match data {
        Some((on, Some(s))) => s.snimok(on),
        Some((on, None)) => proxy::ZhurnalRezki { pravila_vsego: crate::spisok_reklamy::SPISOK.len(), summy: Vec::new(), vklyuchena: on, zarezano: 0, udaleny_elementy:0, propushcheno: 0, zapisi: Vec::new() },
        None => proxy::ZhurnalRezki { pravila_vsego: crate::spisok_reklamy::SPISOK.len(), summy: Vec::new(), vklyuchena: false, zarezano: 0, udaleny_elementy:0, propushcheno: 0, zapisi: Vec::new() },
    }
}

#[tauri::command]
fn aktivnaya(window:tauri::WebviewWindow,s: State<Sostoyanie>) -> Option<String> {
    if window.label()!="main"{return s.okna.0.lock().unwrap().get(window.label()).filter(|w|!w.super_id.is_empty()).map(|w|w.super_id.clone());}
    s.aktivnaya.lock().unwrap().clone()
}

#[tauri::command]
fn dvizhok_est() -> bool {
    crate::cef_files::directory().is_some()
}

/// Спрятать/показать встроенные окна движков. Нужно лицевой части:
/// модальный диалог рисуется ВНУТРИ нашего окна, а поверх его области
/// лежит чужое окно движка — без скрытия диалог не видно (10.11.2026:
/// «добавление супервкладки не видно, пока не закроешь хром»).
/// ⛔ При скрытии прячем не только известные хэндлы, но ВСЕ окна наших
/// движков: пересозданное окно старым хэндлом не видно и лежит поверх,
/// поедая клики по диалогу.
#[tauri::command]
async fn pokazat_dvizhki(app:tauri::AppHandle,window:tauri::WebviewWindow,s: State<'_, Sostoyanie>, vidno: bool) -> Result<Otvet, String> {
    if window.label()!="main"{
        if let Some(extra)=s.okna.0.lock().unwrap().get_mut(window.label()){extra.hidden=!vidno;}
        okna_app::obolochka(&app,&s,window.label());return Ok(da());
    }
    v_zhurnal_granica("pokazat_dvizhki", "-", &format!("vidno={vidno}"));
    let _okna = s.izmenenie_okon.lock().unwrap();
    s.dvizhki_skryty.store(!vidno, std::sync::atomic::Ordering::SeqCst);
    primenit_vidimost(&s);
    v_zhurnal_granica("pokazat_dvizhki", "-", "готово");
    Ok(da())
}

#[tauri::command]
async fn otkryt(
    app: tauri::AppHandle,
    window:tauri::WebviewWindow,
    s: State<'_, Sostoyanie>,
    mut nastrojki: Nastrojki,
) -> Result<Otvet, String> {
    if !supervkladki::cvet_dopustim(&nastrojki.cvet){return Ok(net("Неизвестный цвет супер-вкладки"));}
    nastrojki.privatnaya|=privatnost::eto_id(&nastrojki.id);
    if nastrojki.privatnaya&&!privatnost::eto_id(&nastrojki.id){nastrojki.id=format!("pvt-{}",nastrojki.id);}
    let id = nastrojki.id.clone();
    v_zhurnal_granica("otkryt", &id, "вход");
    {
        let mut v = s.vkladki.lock().unwrap();
        if app_shutdown_parent::is_draining(){return Err("Браузер завершает работу".into());}
        if v.contains_key(&id) {
            return Ok(net("такая супер-вкладка уже есть"));
        }
        nastrojki.poryadok = v.values().map(|v| v.nastrojki.poryadok).max().unwrap_or(-1).saturating_add(1);
        // Запоминаем СРАЗУ: если движок не запустится, вкладка всё равно
        // останется в списке, и человек сможет поправить настройки.
        v.insert(
            id.clone(),
            Vkladka { nastrojki: nastrojki.clone(), proc: None, okno: None, zapas_pid: None, nash_proxy: None, inherit_route: None, route_settings: None, tunnel: None, zapusk_idet: false, zapusk_id: 0, kanal: None, obychnye: None, hibernate:Default::default() },
        );
        let vse: Vec<Nastrojki> = v.values().map(|x| x.nastrojki.clone()).collect();
        sohranit_spisok(&vse);
    }
    if window.label()!="main"{return okna_app::vybrat(app,s,window,id).await;}
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
    if browser_lock::locked_global() { return net("Браузер заблокирован. Введите мастер-пароль."); }
    let (nastrojki, zapusk_id) = {
        let mut v = s.vkladki.lock().unwrap();
        if app_shutdown_parent::is_draining(){return net("Браузер завершает работу");}
        let Some(vk) = v.get_mut(id) else { return net("такой супер-вкладки нет") };
        if vk.hibernate.wake(vk.kanal.as_ref()) { drop(v); pereklyuchit_vnutri(app,s,id); return da(); }
        if vk.zapusk_idet || vk.proc.is_some() {
            drop(v);
            pereklyuchit_vnutri(app,s, id);
            return da();
        }
        vk.zapusk_id=match vk.hibernate.begin_start(){Ok(generation)=>generation,Err(_)=>return net("Предыдущий движок ещё завершает работу")};
        vk.zapusk_idet = true;
        (vk.nastrojki.clone(), vk.zapusk_id)
    };
    pereklyuchit_vnutri(app,s, id);
    let otvet = podgotovit_dvizhok(app, s, id, nastrojki, zapusk_id).await;
    if !otvet.ok {
        let mut v = s.vkladki.lock().unwrap();
        if let Some(vk) = v.get_mut(id).filter(|vk| vk.zapusk_id == zapusk_id) {
            vk.zapusk_idet = false;
            if vk.proc.is_none(){if vk.hibernate.machine.lease_confirmed(){vk.hibernate.no_child_start_failed(zapusk_id);}else{vk.hibernate.lease_failed(zapusk_id);}}
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
            // Проверяем выбор по свежему списку; исчезнувший узел не заменяем молча.
            let ssylka = if zapis.starts_with("http") {
                let uzly = vpn::skachat_podpisku(&zapis).await?;
                vpn::vybrat_uzel(&uzly, &nastrojki.proxy_uzel)?.ssylka.clone()
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

    // Новая и приватная Super начинают со стартового адреса. Старые
    // обычные профили восстанавливают вкладки из JSON без фоновой загрузки.
    let adres = if nastrojki.pustaya || nastrojki.privatnaya
        || (!brauzer::papka_profilya(id).join("obychnye.json").is_file()
            && !brauzer::papka_profilya(id).join("obychnye.backup.json").is_file()) {
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
    let route_bootstrap = tab_routes::RouteBootstrap {
        inherit: tab_routes::NativeRoute {
            key: route_network::effective_key(&nastrojki.proxy, &nastrojki.proxy_uzel),
            proxy: cherez.clone(),
            allow_non_proxied_udp: matches!(proxy::razobrat_vyhod(&nastrojki.proxy),Ok(proxy::Vyhod::Pryamo)),
        },
        bindings: Vec::new(),
    };
    let profile=brauzer::papka_profilya(id);
    if std::fs::create_dir_all(&profile).is_err(){return net("Не удалось открыть профиль");}
    let lease=match super_process_owner::ProfileLease::acquire(&profile){
        Ok(lease)=>lease,
        Err(_)=>{if let Some(v)=s.vkladki.lock().unwrap().get_mut(id).filter(|v|v.zapusk_id==zapusk_id){v.hibernate.lease_failed(zapusk_id);} nash.ostanovit(); if let Some(mut t)=tunnel{let _=t.kill();let _=t.wait();}return net("Предыдущий движок ещё использует профиль");}
    };
    let restore={let mut tabs=s.vkladki.lock().unwrap();
        let Some(v)=tabs.get_mut(id).filter(|v|v.zapusk_id==zapusk_id&&v.zapusk_idet)else{nash.ostanovit();if let Some(mut t)=tunnel{let _=t.kill();let _=t.wait();}return da();};
        if !v.hibernate.lease_acquired(zapusk_id){nash.ostanovit();if let Some(mut t)=tunnel{let _=t.kill();let _=t.wait();}return net("Запуск отменён");}
        match v.hibernate.restore_wire(){Ok(wire)=>wire,Err(_)=>{nash.ostanovit();if let Some(mut t)=tunnel{let _=t.kill();let _=t.wait();}return net("Не удалось подготовить сохранённый вход");}}
    };
    if app_shutdown_parent::is_draining(){
        drop(lease);nash.ostanovit();if let Some(mut t)=tunnel{let _=t.kill();let _=t.wait();}
        return net("Браузер завершает работу");
    }
    let mut ditya = match cef_dvizhok::zapustit_cep(id, &cherez, adres, nashe, nastrojki.rezat_reklamu, m, &route_bootstrap, zapusk_id, lease, restore.is_some()) {
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

    let stdout = ditya.take_stdout().unwrap();
    let kanal = cef_dvizhok::kanal_komand_bootstrap(ditya.take_stdin().unwrap(),restore);
    {
        let mut v = s.vkladki.lock().unwrap();
        match v.get_mut(id).filter(|vk| vk.zapusk_id == zapusk_id && vk.zapusk_idet) {
            Some(vk) => {
                vk.proc = Some(ditya);
                vk.inherit_route = Some(route_bootstrap.inherit.clone());
                vk.route_settings = Some(nastrojki.clone());
                browser_lock::synchronize_engine(&kanal);
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
        let h_adresa = h.clone(); let id_adresa = id_fon.clone();
        let h_ui = h.clone(); let id_ui = id_fon.clone();
        let h_hibernate=h.clone();let id_hibernate=id_fon.clone();
        let okno = cef_dvizhok::dozhdatsya_okna(stdout, 25, move |frame|{super_hibernation_parent::receive(&h_hibernate,&id_hibernate,zapusk_id,frame);}, move |sessiya| {
            let _zapis = sessiya::ZAPIS.lock().unwrap();
            let s: State<Sostoyanie> = h_sessii.state();
            let smena_vidimosti = {
                let mut v = s.vkladki.lock().unwrap();
                let Some(vk) = v.get_mut(&id_sessii).filter(|vk| vk.zapusk_id == zapusk_id && vk.proc.is_some()) else { return };
                let bylo = vk.obychnye.as_ref().is_some_and(stranica_gotova);
                let teper = stranica_gotova(&sessiya);
                vk.obychnye = Some(sessiya.clone());
                bylo != teper
            };
            for tab in &sessiya.vkladki{biblioteka::poseshchenie(&id_sessii,tab);}
            if smena_vidimosti { vosstanovit_dvizhok(&h_sessii); }
            if let Err(e) = sessiya.sohranit(&brauzer::papka_profilya(&id_sessii)) {
                zapisat_v_zhurnal(format!("{id_sessii}: сессия не записана: {e}"));
            }
            youtube_fullscreen_parent::reconcile(&h_sessii);
            let _ = h_sessii.emit_to("main","obychnye-izmenilis", serde_json::json!({"id":id_sessii,"sessiya":sessiya}));
        }, move || { let _ = h_adresa.emit_to("main","adres-vvesti", &id_adresa); }, move |data| {
            // Поздние сообщения завершённого движка не должны менять новый сеанс.
            { let s:State<Sostoyanie>=h_ui.state();
              if !s.vkladki.lock().unwrap().get(&id_ui).is_some_and(|v|v.zapusk_id==zapusk_id){return;} }
            if data["tip"]=="browser-lock"{browser_lock::request_lock(h_ui.clone());return;}
            if app_shutdown_parent::receive(&h_ui,&id_ui,zapusk_id,&data){return;}
            if tab_route_parent::receive(&h_ui,&id_ui,zapusk_id,&data){return;}
            if passwords::receive(&h_ui,&id_ui,&data){return;}
            if youtube_download_parent::receive(&h_ui,&id_ui,zapusk_id,&data){return;}
            if youtube_fullscreen_parent::receive(&h_ui,&id_ui,zapusk_id,&data){return;}
            if dannye_saytov::otvet(&id_ui,&data)||dannye_statistika::otvet(&id_ui,&data){return;}
            {let s:State<Sostoyanie>=h_ui.state();if okna_app::sobytie(&h_ui,&s,&id_ui,&data){youtube_fullscreen_parent::reconcile(&h_ui);return;}}
            let target=data["label"].as_str().filter(|s|!s.is_empty()).unwrap_or("main");
            if data["tip"]=="novoe-okno"{let app=h_ui.clone();let id=id_ui.clone();let private=data["privatnaya"]==true;okna_app::zapros_okna(app,Some(id),private);return;}
            if data["tip"]=="adres-vvesti"{let _=h_ui.emit_to(target,"adres-vvesti",&id_ui);return;}
            if data["tip"]=="blocked"||data["tip"]=="blocked_dom"{
                let s:State<Sostoyanie>=h_ui.state();let counter=s.vkladki.lock().unwrap().get(&id_ui).filter(|v|v.zapusk_id==zapusk_id).and_then(|v|v.nash_proxy.as_ref().map(|p|p.schet.clone()));
                if let Some(counter)=counter{let host=data["host"].as_str().unwrap_or("");if data["tip"]=="blocked"{counter.blok(host,data["rule"].as_str().unwrap_or("Правило ресурса"),"CEF");}else if let Some(rules)=data["rules"].as_object(){for (rule,count) in rules{if let Some(n)=count.as_u64(){counter.kosmetika(host,rule,n.min(10000));}}}}
                return;
            }
            if matches!(data["tip"].as_str(),Some("okno-session"|"okno-closed"|"okno-ready")){youtube_fullscreen_parent::reconcile(&h_ui);}
            if data["tip"]=="download"{zagruzki::obnovit(&id_ui,&data);return;}
            if data["tip"]=="private-ended"{zagruzki::ochistit_privatnye(&id_ui);let s:State<Sostoyanie>=h_ui.state();if let Some(p)=s.vkladki.lock().unwrap().get(&id_ui).and_then(|v|v.nash_proxy.as_ref().map(|p|p.schet.clone())){p.ochistit();}return;}
            if data["tip"]=="perevod-otmena" {
                if let Some(id)=data["id"].as_u64(){perevod::otmenit(&id_ui,id);}
            }
            if data["tip"]=="perevod-zapros" {
                let s:State<Sostoyanie>=h_ui.state();
                let route=tab_route_parent::translation_port(&s,&id_ui,zapusk_id,&data)
                    .and_then(|port| s.vkladki.lock().ok()?.get(&id_ui)?.kanal.clone().map(|channel|(port,channel)));
                if let Some((port,kanal))=route{perevod::zapros(id_ui.clone(),&data,port,kanal);}
                return;
            }
            let _ = h_ui.emit_to(target,"stranica-ui", serde_json::json!({"super_id":id_ui,"data":data}));
        });
        let s: State<Sostoyanie> = h.state();
        let mut v = s.vkladki.lock().unwrap();
        let Some(vk) = v.get_mut(&id_fon)
            .filter(|vk| vk.zapusk_id == zapusk_id && vk.zapusk_idet && vk.proc.is_some())
        else { return };
        vk.zapusk_idet = false;
        if app_shutdown_parent::is_draining(){return;}
        if let Some(okno) = okno {
            vk.okno = Some(okno);
            vk.hibernate.ready(zapusk_id);
            drop(v);
            {
                let _okna = s.izmenenie_okon.lock().unwrap();
                primenit_vidimost(&s);
            }
            okna_app::vosstanovit(&h,&s,&id_fon,zapusk_id);
            zapisat_v_zhurnal(format!("{id_fon}: запуск ок, окно {okno}"));
        } else {
            let resursy = (vk.proc.take(), vk.nash_proxy.take(), vk.tunnel.take());
            drop(v);
            ostanovit_resursy(resursy.0, resursy.1, resursy.2);
            release_route_resources(&h,&s, &id_fon, zapusk_id);
            if let Some(v)=s.vkladki.lock().unwrap().get_mut(&id_fon).filter(|v|v.zapusk_id==zapusk_id&&v.proc.is_none()){v.hibernate.no_child_start_failed(zapusk_id);}
            zapisat_v_zhurnal(format!("{id_fon}: движок не ответил за 25 с (см. cef.log)"));
            let _ = h.emit("oshibka-dvizhka", format!("{id_fon}: движок не ответил за 25 с (см. cef.log)"));
        }
        let _ = h.emit("vkladki-izmenilis", "");
    });
    da()
}

#[tauri::command]
async fn pereklyuchit(app: tauri::AppHandle,window:tauri::WebviewWindow, s: State<'_, Sostoyanie>, id: String) -> Result<Otvet, String> {
    if window.label()!="main"{return okna_app::vybrat(app,s,window,id).await;}
    v_zhurnal_granica("pereklyuchit", &id, "вход");
    let zapushchena = match s.vkladki.lock().unwrap().get_mut(&id) {
        Some(vk) => {
            let waiting=vk.hibernate.wake(vk.kanal.as_ref());
            // Launcher exit cannot drop a still-live descendant/profile lease.
            waiting || vk.zapusk_idet || vk.proc.is_some()
        }
        None => return Ok(net("такой супер-вкладки нет")),
    };
    // Вкладку поднимаем в тот момент, когда на неё нажали, а не при запуске
    // программы — иначе десять вкладок сразу съедят всю память.
    if !zapushchena {
        return Ok(zapustit_vnutri(&app, &s, &id).await);
    }
    pereklyuchit_vnutri(&app,&s, &id);
    // ⛔ SetFocus в чужое окно с главного потока на Windows может повиснуть
    // вместе с потоком движка (12.11.2026, «виснет в рандомные моменты»).
    // Фокус движку отдаст клик мышью — штатно.
    Ok(da())
}

fn sohranit_aktivnuyu(id: &str) {
    if privatnost::eto_id(id){return;}
    let f = brauzer::papka_profilej()
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."))
        .join("aktivnaya.json");
    let _ = sessiya::durable_replace(&f, serde_json::to_string(id).unwrap_or_default().as_bytes());
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

#[cfg(target_os = "windows")]
fn obnovit_oblast_obolochki(s: &State<Sostoyanie>, oblast: Option<Mesto>) {
    let Some(w) = s.obolochka.get() else { return };
    let pokolenie = s.oblast_pokolenie.clone();
    let nomer = pokolenie.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
    let ui = w.clone();
    let _ = w.run_on_main_thread(move || {
        // UI не ждёт mutex работника. Старые show/resize не могут затереть hide.
        if pokolenie.load(std::sync::atomic::Ordering::SeqCst) != nomer { return }
        if let Ok(hwnd) = ui.hwnd() {
            if let Err(e) = win32_oblast::primenit(windows::Win32::Foundation::HWND(hwnd.0), oblast) {
                zapisat_v_zhurnal(format!("область WebView2: {e}"));
            }
        }
    });
}

fn stranica_gotova(s: &sessiya::Sessiya) -> bool {
    s.vkladki.iter().find(|v| v.id == s.aktivnaya).is_some_and(|v| !v.otlozhena && v.browser_id > 0)
}
fn primenit_vidimost(s: &State<Sostoyanie>) {
    let m = *s.mesto.lock().unwrap();
    let aktivnaya = s.aktivnaya.lock().unwrap().clone();
    let skryty = s.dvizhki_skryty.load(std::sync::atomic::Ordering::SeqCst) || browser_lock::locked_global();
    let okna: Vec<_> = s.vkladki.lock().unwrap().iter()
        .filter_map(|(id, vk)| vk.okno.map(|o| (id.clone(), o, vk.obychnye.as_ref().is_some_and(stranica_gotova)))).collect();
    #[cfg(target_os = "windows")]
    let vyrez = !skryty && okna.iter().any(|(id, _, gotova)| *gotova && Some(id) == aktivnaya.as_ref());
    for (id, o, gotova) in okna {
        let vidno = !skryty && gotova && Some(&id) == aktivnaya.as_ref();
        let _ = sistema::pokazat(o, vidno);
        if vidno {
            if let Some(m) = m { let _ = sistema::peremestit(o, m); }
        }
    }
    #[cfg(target_os = "windows")]
    obnovit_oblast_obolochki(s, if vyrez { m } else { None });
}

fn pereklyuchit_vnutri(app:&tauri::AppHandle,s: &State<Sostoyanie>, id: &str) {
    let previous=s.aktivnaya.lock().unwrap().clone();
    let _okna = s.izmenenie_okon.lock().unwrap();
    if !s.vkladki.lock().unwrap().contains_key(id) { return }
    *s.aktivnaya.lock().unwrap() = Some(id.to_string());
    primenit_vidimost(s);
    sohranit_aktivnuyu(id);
    drop(_okna);
    if let Some(old)=previous.filter(|old|old!=id){let channel=s.vkladki.lock().unwrap().get(&old).and_then(|v|v.kanal.clone());if let Some(channel)=channel{let _=channel.try_send(sessiya::Komanda::YoutubeExitFullscreen);}}
    youtube_fullscreen_parent::reconcile(app);
}

fn release_route_resources(app:&tauri::AppHandle,s: &Sostoyanie, id: &str, engine: u64) {
    youtube_fullscreen_parent::release_super(app,id,engine);
    perevod::otmenit_super(id);
    tab_route_parent::release_engine(id, engine);
    s.routes.release_engine(id, engine);
}

fn ostanovit_resursy(
    proc: Option<super_process_owner::ProcessOwner>, proxy: Option<proxy::Rabotayushchij>, tunnel: Option<Child>,
) {
    if let Some(p) = proc {
        cef_dvizhok::ostanovit_owned_process(p);
    }
    if let Some(p) = proxy {
        p.ostanovit();
    }
    if let Some(mut t) = tunnel {
        let _ = t.kill();
        let _ = t.wait();
    }
}

async fn ostanovit_vnutri(app:&tauri::AppHandle,s: &State<'_, Sostoyanie>, id: &str) {
    perevod::otmenit_super(id);
    let resursy = {
        let mut v = s.vkladki.lock().unwrap();
        if app_shutdown_parent::is_draining(){return;}
        let Some(vk) = v.get_mut(id) else { return };
        vk.hibernate.explicit_stop();
        vk.zapusk_idet = true;
        vk.kanal = None;
        vk.okno = None;
        vk.zapas_pid = None;
        (vk.proc.take(), vk.nash_proxy.take(), vk.tunnel.take(), vk.zapusk_id)
    };
    if tauri::async_runtime::spawn_blocking(move || {
        ostanovit_resursy(resursy.0, resursy.1, resursy.2);
    }).await.is_err(){return;} // Keep pending ownership on an unproven worker failure.
    release_route_resources(app,s, id, resursy.3);
    if let Some(v)=s.vkladki.lock().unwrap().get_mut(id).filter(|v|v.zapusk_id==resursy.3){v.hibernate=Default::default();v.zapusk_idet=false;}
}

/// Правка настроек супер-вкладки после создания. Работающую перезапускаем
/// с новыми настройками (прокси и резка рекламы задаются на запуск),
/// остановленную просто перезаписываем.
#[tauri::command]
async fn izmenit(app: tauri::AppHandle, s: State<'_, Sostoyanie>, nastrojki: Nastrojki) -> Result<Otvet, String> {
    if !supervkladki::cvet_dopustim(&nastrojki.cvet){return Ok(net("Неизвестный цвет супер-вкладки"));}
    v_zhurnal_granica("izmenit", &nastrojki.id, "вход");
    let (staraya, aktivnaya_stranica) = {
        let v = s.vkladki.lock().unwrap();
        let vk = v.get(&nastrojki.id).ok_or("такой супер-вкладки нет")?;
        (vk.nastrojki.clone(), vk.obychnye.as_ref().map(|s| s.aktivnaya))
    };
    if staraya.privatnaya!=nastrojki.privatnaya{return Ok(net("Приватный режим задаётся при создании супер-вкладки"));}
    let nuzhen_restart=staraya.proxy!=nastrojki.proxy||staraya.proxy_uzel!=nastrojki.proxy_uzel||staraya.adres!=nastrojki.adres||staraya.rezat_reklamu!=nastrojki.rezat_reklamu||staraya.pustaya!=nastrojki.pustaya;
    let smena_seti = staraya.proxy != nastrojki.proxy || staraya.proxy_uzel != nastrojki.proxy_uzel;
    if smena_seti && s.vkladki.lock().unwrap().get(&nastrojki.id).is_some_and(|v|v.proc.is_none()&&v.hibernate.snapshot.is_some()){return Ok(net("Сначала откройте эту супервкладку, затем смените её путь."));}
    if smena_seti && vpn::eto_podpiska(&nastrojki.proxy) {
        let proverka = vpn::proverit_podklyuchenie(&nastrojki.proxy, &nastrojki.proxy_uzel).await;
        if proverka.is_err() {
            return Ok(net("Выбранный узел не прошёл проверку HTTP(S). Прежние настройки и подключение сохранены. Выберите другой узел или повторите проверку."));
        }
    }
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
    let archive = nastrojki.clone();
    tauri::async_runtime::spawn_blocking(move || supervkladki::obnovit_sohranennye(|list| {
        if let Some(n) = list.iter_mut().find(|n| n.id == archive.id) { *n = archive; }
    })).await.map_err(|e| e.to_string())??;
    if rabotaet && nuzhen_restart {
        ostanovit_vnutri(&app,&s, &nastrojki.id).await;
        let result = zapustit_vnutri(&app, &s, &nastrojki.id).await;
        if result.ok {
            let id = if nastrojki.pustaya { Some(1) } else { aktivnaya_stranica };
            if let Some(id) = id {
                if let Some(k) = s.vkladki.lock().unwrap().get(&nastrojki.id).and_then(|v| v.kanal.clone()) {
                    let _ = k.try_send(sessiya::Komanda::Vybrat { id });
                }
            }
        }
        if !result.ok && smena_seti {
            // Между проверкой и запуском сеть может измениться: возвращаем прежний выход.
            {
                let mut v = s.vkladki.lock().unwrap();
                if let Some(vk) = v.get_mut(&nastrojki.id) { vk.nastrojki = staraya.clone(); }
                sohranit_spisok(&v.values().map(|vk| vk.nastrojki.clone()).collect::<Vec<_>>());
            }
            let archive = staraya.clone();
            let _ = tauri::async_runtime::spawn_blocking(move || supervkladki::obnovit_sohranennye(|list| {
                if let Some(n) = list.iter_mut().find(|n| n.id == archive.id) { *n = archive; }
            })).await;
            let old = zapustit_vnutri(&app, &s, &nastrojki.id).await;
            if old.ok {
                let id = if staraya.pustaya { Some(1) } else { aktivnaya_stranica };
                if let Some(k) = s.vkladki.lock().unwrap().get(&nastrojki.id).and_then(|v| v.kanal.clone()) {
                    if let Some(id) = id { let _ = k.try_send(sessiya::Komanda::Vybrat { id }); }
                }
                return Ok(net("Новый выход не запустился. Прежние настройки и подключение восстановлены."));
            }
            return Ok(net("Новый выход не запустился. Прежние настройки сохранены, но восстановить подключение не удалось. Повторите подключение супер-вкладки."));
        }
        return Ok(result);
    }
    let _ = app.emit("vkladki-izmenilis", nastrojki.id);
    Ok(da())
}

/// Есть ли в профиле что-то стоящее сохранять: куки, история. Свежая
/// вкладка закрывается без вопросов; жилая спрашивает «сохранить?».
#[tauri::command]
fn nuzhno_sohranenie(id: String) -> bool {
    !privatnost::eto_id(&id) && !brauzer::profil_svety(&id)
}

/// Стереть профиль вкладки. Файлы могут быть ещё подхвачены только что
/// убитым движком (Windows держит блокировки) — повторяем несколько раз,
/// иначе «не сохраняет/не удаляет».
fn steret_profil(id: &str) {
    let _zapis = sessiya::ZAPIS.lock().unwrap();
    let put = brauzer::papka_profilya(id);
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
    let generation = browser_lock::generation();
    let sync = app.state::<browser_sync::SyncState>();
    let _vault_operation = sync.serial.lock().await;
    if generation != browser_lock::generation() || browser_lock::locked_global() {
        return Err("Состояние блокировки изменилось; повторите действие".into());
    }
    let _sync_mutation=browser_sync::mutation("super",&id);
    if !sohranit {
        let settings=s.vkladki.lock().map_err(|_|"Список занят")?.get(&id).map(|v|v.nastrojki.clone());
        if let Some(settings)=settings{browser_sync::archive_super(&app.state::<std::sync::Arc<secure_vault::VaultState>>(),&settings,"local_delete")?;}
    }
    okna_app::zakryt_super(&app,&s,&id);
    {let engine=s.vkladki.lock().unwrap().get(&id).map(|v|v.zapusk_id);if let Some(engine)=engine{youtube_fullscreen_parent::release_super(&app,&id,engine);}}
    perevod::otmenit_super(&id);
    let (udalena, mut deletion) = {
        let mut v = s.vkladki.lock().unwrap();
        if app_shutdown_parent::is_draining(){return Err("Браузер завершает работу".into());}
        let guard=v.contains_key(&id).then(DeletionGuard::new);
        (v.remove(&id),guard)
    };
    let Some(vk) = udalena else {
        return Ok(net("такой супер-вкладки нет"));
    };
    v_zhurnal_granica("zakryt", &id, "останавливаю движок");
    let nastrojki = vk.nastrojki;
    tauri::async_runtime::spawn_blocking(move || {
        ostanovit_resursy(vk.proc, vk.nash_proxy, vk.tunnel);
    }).await.map_err(|_|"Завершение удаляемой супервкладки не подтверждено")?;
    if let Some(guard)=deletion.as_mut(){guard.stopped=true;}
    v_zhurnal_granica("zakryt", &id, "движок остановлен");
    {
        let v = s.vkladki.lock().unwrap();
        let vse: Vec<Nastrojki> = v.values().map(|x| x.nastrojki.clone()).collect();
        sohranit_spisok(&vse);
    }
    let id_biblioteki = id.clone();
    tauri::async_runtime::spawn_blocking(move || supervkladki::obnovit_sohranennye(|list| {
        list.retain(|n| n.id != id_biblioteki);
        if sohranit { list.push(nastrojki); }
    })).await.map_err(|e| e.to_string())??;
    if !sohranit {
        if !privatnost::eto_id(&id){browser_sync::record_delete("super",&id)?;}
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
        s.vkladki.lock().unwrap().values().min_by_key(|v| (v.nastrojki.poryadok, &v.nastrojki.id)).map(|v| v.nastrojki.id.clone())
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
async fn vosstanovit(app: tauri::AppHandle,window:tauri::WebviewWindow, s: State<'_, Sostoyanie>, id: String) -> Result<Otvet, String> {
    let _sync_mutation=browser_sync::mutation("super",&id);
    if s.vkladki.lock().unwrap().contains_key(&id) {
        if window.label()!="main"{return okna_app::vybrat(app,s,window,id).await;}
        return Ok(zapustit_vnutri(&app, &s, &id).await);
    }
    let saved_id = id.clone();
    let nastrojki = tauri::async_runtime::spawn_blocking(move || zagruzit_sohranennye().into_iter().find(|n| n.id == saved_id))
        .await.map_err(|e| e.to_string())?.ok_or_else(|| "Такой сохранённой вкладки нет".to_string())?;
    {
        let mut v = s.vkladki.lock().unwrap();
        if v.contains_key(&id) {
            return Ok(net("такая супер-вкладка уже открыта"));
        }
        v.insert(id.clone(), Vkladka { nastrojki: nastrojki.clone(), proc: None, okno: None, zapas_pid: None, nash_proxy: None, inherit_route: None, route_settings: None, tunnel: None, zapusk_idet: false, zapusk_id: 0, kanal: None, obychnye: None, hibernate:Default::default() });
        let vse: Vec<Nastrojki> = v.values().map(|x| x.nastrojki.clone()).collect();
        sohranit_spisok(&vse);
    }
    if window.label()!="main"{return okna_app::vybrat(app,s,window,id).await;}
    Ok(zapustit_vnutri(&app, &s, &id).await)
}

/// Выбросить сохранённую супер-вкладку насовсем: настройки и профиль
/// со всеми входами. Подтверждение спрашивает лицевая часть.
#[tauri::command]
async fn udalit_sohranennuyu(app: tauri::AppHandle, s: State<'_, Sostoyanie>, id: String) -> Result<Otvet, String> {
    let generation = browser_lock::generation();
    let sync = app.state::<browser_sync::SyncState>();
    let _vault_operation = sync.serial.lock().await;
    if generation != browser_lock::generation() || browser_lock::locked_global() {
        return Err("Состояние блокировки изменилось; повторите действие".into());
    }
    let _sync_mutation=browser_sync::mutation("super",&id);
    let otkryta = s.vkladki.lock().map_err(|e| e.to_string())?.contains_key(&id);
    if !otkryta&&!privatnost::eto_id(&id){
        if let Some(settings)=zagruzit_sohranennye().into_iter().find(|n|n.id==id){browser_sync::archive_super(&app.state::<std::sync::Arc<secure_vault::VaultState>>(),&settings,"local_delete")?;}
        browser_sync::record_delete("super",&id)?;
    }
    tauri::async_runtime::spawn_blocking(move || {
        supervkladki::obnovit_sohranennye(|list| list.retain(|n| n.id != id))?;
        // Открытую вкладку удаляем из библиотеки, её живой профиль не трогаем.
        if !otkryta { steret_profil(&id); }
        Ok::<_,String>(())
    }).await.map_err(|e| e.to_string())??;
    let _ = app.emit("vkladki-izmenilis", ""); Ok(da())
}

/// Перезагрузить движок вкладки (контекстное меню вкладки).
#[tauri::command]
async fn perezagruzit(app: tauri::AppHandle, s: State<'_, Sostoyanie>, id: String) -> Result<Otvet, String> {
    if !s.vkladki.lock().unwrap().contains_key(&id) {
        return Ok(net("такой супер-вкладки нет"));
    }
    ostanovit_vnutri(&app,&s, &id).await;
    Ok(zapustit_vnutri(&app, &s, &id).await)
}

#[tauri::command]
async fn fokus_obolochke(window: tauri::WebviewWindow) -> Result<(),String> {
    #[cfg(target_os = "linux")]
    {
        let (sender, receiver) = tokio::sync::oneshot::channel();
        let shell = window.clone();
        window.run_on_main_thread(move || {
            let result = (|| -> Result<(), String> {
                use gtk::prelude::*;
                shell.gtk_window().map_err(|e|e.to_string())?.present();
                let nashe = nash_descriptor(&shell).ok_or("не найдено окно оболочки")?;
                sistema::fokus_obolochke(nashe).map_err(|e|e.to_string())?;
                shell.with_webview(|platform| { platform.inner().grab_focus(); })
                    .map_err(|e|e.to_string())?;
                let webview: &tauri::Webview = shell.as_ref();
                webview.set_focus().map_err(|e|e.to_string())
            })();
            let _ = sender.send(result);
        }).map_err(|e|e.to_string())?;
        tokio::time::timeout(std::time::Duration::from_secs(5), receiver).await
            .map_err(|_|"Оболочка не приняла фокус".to_string())?
            .map_err(|_|"Окно оболочки закрыто".to_string())?
    }
    #[cfg(not(target_os = "linux"))]
    {
        window.set_focus().map_err(|e|e.to_string())?;
        let webview: &tauri::Webview = window.as_ref();
        webview.set_focus().map_err(|e|e.to_string())
    }
}

#[tauri::command]
async fn obychnye_spisok(window:tauri::WebviewWindow,s: State<'_, Sostoyanie>, id: String) -> Result<sessiya::Sessiya,String> {
    if window.label()!="main"{return s.okna.0.lock().unwrap().get(window.label()).filter(|w|w.super_id==id).and_then(|w|w.sessiya.clone()).ok_or("Окно ещё запускается".into());}
    {
        let v = s.vkladki.lock().unwrap();
        let vk = v.get(&id).ok_or("такой супер-вкладки нет")?;
        if let Some(sessiya) = &vk.obychnye { return Ok(sessiya.clone()) }
        // Не подставлять фиктивную about:blank во время запуска: она могла
        // перехватить фокус и затереть ввод до первой настоящей сессии CEF.
        if vk.zapusk_idet||vk.proc.is_some(){return Err("Окно ещё запускается".into());}
    }
    tauri::async_runtime::spawn_blocking(move || sessiya::Sessiya::zagruzit(&brauzer::papka_profilya(&id)))
        .await.map_err(|e| e.to_string())
}

#[tauri::command]
async fn obychnaya_komanda(window:tauri::WebviewWindow,s: State<'_, Sostoyanie>, id: String, komanda: sessiya::Komanda) -> Result<(),String> {
    let komanda=if window.label()!="main"{okna_app::ready_command(window.app_handle(),&s,window.label(),komanda).await?}else{komanda};
    let kanal=okna_app::kanal(&s,window.label(),&id)?;
    let komanda=if window.label()=="main"{komanda}else{sessiya::Komanda::OknoKomanda{label:window.label().into(),komanda:Box::new(komanda)}};
    kanal.try_send(komanda).map_err(|e| match e {
        std::sync::mpsc::TrySendError::Full(_) => "Движок занят. Повторите действие.".into(),
        std::sync::mpsc::TrySendError::Disconnected(_) => "Движок остановлен. Откройте супер-вкладку снова.".into(),
    })
}

// Доступ имеет только доверенная оболочка Tauri, не страницы CEF.
#[tauri::command]
async fn bufer_chitat(app:tauri::AppHandle)->Result<String,String>{
 use tauri_plugin_clipboard_manager::ClipboardExt;
 tauri::async_runtime::spawn_blocking(move||app.clipboard().read_text().map_err(|_|"Не удалось прочитать текст буфера обмена".to_owned())).await.map_err(|e|e.to_string())?
}
#[tauri::command]
async fn bufer_zapisat(app:tauri::AppHandle,text:String)->Result<(),String>{
 use tauri_plugin_clipboard_manager::ClipboardExt;
 if text.len()>2*1024*1024{return Err("Слишком большой текст для буфера обмена".into());}
 tauri::async_runtime::spawn_blocking(move||app.clipboard().write_text(text).map_err(|_|"Не удалось скопировать текст".to_owned())).await.map_err(|e|e.to_string())?
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
    out.push_str(&format!("наше окно: {nashe}\n    {}\n", okna::opisanie_okna(nashe.parse().unwrap_or(0))));
    out.push_str(&format!("активная: {:?}, место: {:?}, скрыты для диалога: {}\n",
        s.aktivnaya.lock().unwrap().clone(), *s.mesto.lock().unwrap(),
        s.dvizhki_skryty.load(std::sync::atomic::Ordering::Acquire)));
    out.push_str(&format!("впереди сейчас: {}\n", sistema::kto_v_peredne()));
    {
        let v = s.vkladki.lock().unwrap();
        for (id, vk) in v.iter() {
            if let Some(t) = &vk.tunnel {
                out.push_str(&format!("VPN {id}: pid={}, жив={}\n", t.id(), sistema::pid_zhiv(t.id())));
            }
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
    youtube_fullscreen_parent::reconcile(app);
    let s: tauri::State<Sostoyanie> = app.state();
    if let Ok(_okna) = s.izmenenie_okon.try_lock() {
        primenit_vidimost(&s);
    };

}

fn trusted_app_control(app: &tauri::AppHandle, window: &tauri::WebviewWindow) -> Result<(), String> {
    let s: State<Sostoyanie> = app.state();
    if window.label() == "main" || s.okna.0.lock().unwrap().contains_key(window.label()) { Ok(()) }
    else { Err("Окно не имеет доступа к завершению приложения".into()) }
}
#[tauri::command]
fn app_restart(app: tauri::AppHandle, window: tauri::WebviewWindow) -> Result<(), String> {
    trusted_app_control(&app, &window)?;
    app_shutdown_parent::begin(&app, i32::MAX);
    Ok(())
}
#[tauri::command]
async fn app_prepare_update(app: tauri::AppHandle, window: tauri::WebviewWindow) -> Result<(), String> {
    trusted_app_control(&app, &window)?;
    let handle = app.clone();
    app.run_on_main_thread(move || app_shutdown_parent::prepare_update(&handle)).map_err(|_| "Не удалось начать завершение движков")?;
    let started = std::time::Instant::now();
    loop {
        match app_shutdown_parent::preparation_result() {
            Some(true) => return Ok(()),
            Some(false) => return Err("Движки не подтвердили безопасное завершение. Обновление отменено.".into()),
            None => {}
        }
        if started.elapsed() > std::time::Duration::from_secs(15) {
            return Err("Завершение движков ещё не подтверждено. Обновление не установлено.".into());
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let recovery_root=brauzer::papka_profilej();
    if let Err(error)=browser_backup::recover_pending(recovery_root.parent().expect("Не найдена папка данных")){
        eprintln!("Не удалось безопасно завершить восстановление резервной копии: {error}");
        return; // Never load a partially committed settings/profile configuration.
    }

    let command_handler: Box<dyn Fn(tauri::ipc::Invoke<tauri::Wry>) -> bool + Send + Sync> =
        Box::new(tauri::generate_handler![

                app_restart, app_prepare_update,
                popup_menu::popup_menu_open,popup_menu::popup_menu_data,popup_menu::popup_menu_ready,
                popup_menu::popup_menu_choose,popup_menu::popup_menu_dismiss,popup_menu::popup_menu_lock,
                browser_lock::browser_lock_status,browser_lock::browser_lock,browser_lock::browser_unlock,
                browser_lock::browser_vault_backups,browser_lock::browser_vault_reset,browser_lock::browser_vault_restore,

            okna_app::novoe_okno,okna_app::okno_gotovo,okna_app::pokazat_glavnoe,
            set_status::set_status,set_status::set_proverit_ip,
            zagruzki::zagruzki_spisok,zagruzki::zagruzki_ochistit,zagruzki::zagruzka_deystvie,
            import_brauzera::import_istochniki,import_brauzera::import_nachat,import_brauzera::import_status,import_brauzera::import_otmenit,
            passwords::security_vault,passwords::security_passwords,browser_sync::security_account,browser_sync::security_sync,
            dannye_statistika::dannye_saytov_statistika,biblioteka::dannye_obem,dannye_saytov::ochistit_dannye_saytov,biblioteka::biblioteka_podskazki,biblioteka::dannye_ochistit,
            biblioteka::biblioteka_spisok,biblioteka::biblioteka_udalit,biblioteka::zakladki_sohranit,biblioteka::zakladki_pereklyuchit,biblioteka::zakladki_import,
            nastrojki::nastrojki_poluchit, nastrojki::nastrojki_sohranit, vpn::uzly_podpiski, vpn::proverit_vyhod, vpn::proverit_uzel,
            supervkladki::sohranit_vkladku, supervkladki::peremestit_super,
            browser_backup::security_backup, browser_tasks::browser_tasks_snapshot,
            super_hibernation_parent::super_lifecycle_snapshot, tab_search::tab_search_snapshot, tab_search::tab_search_activate, fokus_obolochke, obychnye_spisok, obychnaya_komanda, spisok, aktivnaya, schet_rezki, zhurnal_rezki, otkryt, pereklyuchit, zakryt, izmenit,
            tab_route_parent::tab_route_choices, tab_route_parent::tab_route_set,
            zadat_mesto, dvizhok_est, pokazat_dvizhki, nuzhno_sohranenie,
            bufer_chitat,bufer_zapisat,sohranennye, vosstanovit, udalit_sohranennuyu, diagnostika, perezagruzit

                    ]);
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app,_args,_cwd|{okna_app::zapros_okna(app.clone(),None,false);}))
        .manage(Sostoyanie::default())
        .manage(std::sync::Arc::new(secure_vault::VaultState::for_browser_session()))
        .manage(browser_account::AccountState::default())
        .manage(browser_sync::SyncState::default())
        .manage(popup_menu::PopupMenuState::default())
        .manage(browser_lock::BrowserLockState::default())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        // Размер, позиция и монитор окна запоминаются сами.
        .plugin(tauri_plugin_window_state::Builder::default().with_filter(|label|label=="main").build())
        .setup(|app| {
            #[cfg(target_os = "linux")]
            gtk_tooltip_input::install().map_err(std::io::Error::other)?;
            app_shutdown_parent::init(app.handle());
            passwords::setup(app.handle()).map_err(std::io::Error::other)?;
            browser_lock::setup(app.handle());
            #[cfg(target_os = "windows")]
            if let Some(w) = app.get_webview_window("main") {
                let s: State<Sostoyanie> = app.state();
                let _ = s.obolochka.set(w);
            }
            // Движки, оставшиеся от прошлого запуска, ломают новые:
            // Chromium пересылает команду старому процессу, а тот открывает
            // обычное окно поверх всего. Режем по уникальному имени exe.
            privatnost::ochistit_posle_sboya();
            // Жизнью CEF управляют JobObject/PDEATHSIG. Не убиваем процессы
            // по имени exe: это закрывало уже работающий NexusBro при повторном запуске.
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
                    v.insert(n.id.clone(), Vkladka { nastrojki: n, proc: None, okno: None, zapas_pid: None, nash_proxy: None, inherit_route: None, route_settings: None, tunnel: None, zapusk_idet: false, zapusk_id: 0, kanal: None, obychnye: None, hibernate:Default::default() });
                }
            }
            if !browser_lock::locked_global(){okna_app::restore_windows(app.handle());}
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
                        nazvanie: "Проба".into(), cvet:String::new(), poryadok: 0,
                        proxy: String::new(),
                        proxy_uzel: String::new(),
                        adres: "about:blank".into(),
                        rezat_reklamu: true,
                        pustaya: true,
                        privatnaya: false,
                    };
                    let o = tauri::async_runtime::block_on(otkryt(h.clone(), h.get_webview_window("main").expect("main"), s, n))
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
                            nazvanie: "Проба2".into(), cvet:String::new(), poryadok: 1,
                            proxy: String::new(),
                            proxy_uzel: String::new(),
                            adres: "about:blank".into(),
                            rezat_reklamu: true,
                            pustaya: true,
                            privatnaya: false,
                        };
                        let s2: tauri::State<Sostoyanie> = h.state();
                        let o2 = tauri::async_runtime::block_on(otkryt(h.clone(), h.get_webview_window("main").expect("main"), s2, n2))
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
                std::thread::spawn(move || {
                let mut vosstanovleniya = std::collections::HashMap::<String, std::time::Instant>::new();
                loop {
                    std::thread::sleep(std::time::Duration::from_millis(1000));
                    if ZAVERSHENIE.load(std::sync::atomic::Ordering::SeqCst) {
                        return;
                    }
                    super_hibernation_parent::tick(&h);
                    youtube_fullscreen_parent::reconcile(&h);
                    let s: State<Sostoyanie> = h.state();
                    let mertvye: Vec<_> = {
                        let Ok(mut v) = s.vkladki.try_lock() else { continue };
                        if app_shutdown_parent::is_draining(){return;}
                        v.iter_mut().filter_map(|(id, vk)| {
                            if vk.zapusk_idet || matches!(vk.hibernate.machine.phase(),super_lifecycle::Phase::Stopping) { return None }
                            // A dead launcher permits crash cleanup, never lease release.
                            // The retained reaper still waits for the whole Job/group.
                            let umer = vk.proc.as_mut().is_some_and(|p|matches!(p.try_wait(),Ok(Some(_))));
                            let vpn_umer = matches!(vk.tunnel.as_mut().map(|p| p.try_wait()), Some(Ok(Some(_))));
                            if !umer && !vpn_umer { return None }
                            let active = vk.obychnye.as_ref().map(|s| s.aktivnaya);
                            vk.hibernate.explicit_stop();
                            vk.okno = None;
                            vk.kanal = None;
                            vk.zapas_pid = None;
                            Some((id.clone(), vk.proc.take(), vk.nash_proxy.take(), vk.tunnel.take(), vpn_umer, active, vk.zapusk_id))
                        }).collect()
                    };
                    for (id, proc, proxy, tunnel, vpn_umer, active, engine) in mertvye {
                        youtube_fullscreen_parent::release_super(&h,&id,engine);
                        ostanovit_resursy(proc, proxy, tunnel);
                        release_route_resources(&h,&s, &id, engine);
                        if let Some(v)=s.vkladki.lock().unwrap().get_mut(&id).filter(|v|v.zapusk_id==engine){v.hibernate=Default::default();}
                        {
                            let _okna = s.izmenenie_okon.lock().unwrap();
                            primenit_vidimost(&s);
                        }
                        if vpn_umer {
                            let mozhno = vosstanovleniya.get(&id).map(|t| t.elapsed().as_secs() >= 60).unwrap_or(true);
                            if mozhno {
                                vosstanovleniya.insert(id.clone(), std::time::Instant::now());
                                zapisat_v_zhurnal(format!("{id}: VPN завершился, восстанавливаю"));
                                let result = tauri::async_runtime::block_on(zapustit_vnutri(&h, &s, &id));
                                if result.ok {
                                    let restored = s.vkladki.lock().unwrap().get(&id).map(|v| (v.kanal.clone(), if v.nastrojki.pustaya { Some(1) } else { active }));
                                    if let Some((Some(k), active)) = restored {
                                        if let Some(id) = active { let _ = k.try_send(sessiya::Komanda::Vybrat { id }); }
                                    }
                                } else {
                                    let _ = h.emit("oshibka-dvizhka", "VPN завершился, восстановить подключение не удалось. Нажмите супер-вкладку для повторного подключения или выберите другой узел.");
                                }
                            } else {
                                let _ = h.emit("oshibka-dvizhka", "VPN повторно завершился. Нажмите супер-вкладку для повторного подключения или выберите другой узел.");
                            }
                        }
                        if !vpn_umer { zapisat_v_zhurnal(format!("{id}: движок умер, вкладка спит")); }
                        let _ = h.emit("vkladki-izmenilis", "");
                    }
                }
                });
            }
            Ok(())
        })
        .invoke_handler(move |invoke| {
            let label = invoke.message.webview_ref().label();
            let command = invoke.message.command();
            if label.starts_with("menu-") && !matches!(command,
                "popup_menu_data" | "popup_menu_ready" | "popup_menu_choose" | "popup_menu_dismiss" | "popup_menu_lock")
            {
                invoke.resolver.reject("Меню не имеет доступа к этой операции");
                return true;
            }
            if browser_lock::locked_global() && !matches!(command,
                "app_restart" | "browser_lock_status" | "browser_lock" | "browser_unlock" |
                "browser_vault_backups" | "browser_vault_reset" | "browser_vault_restore")
            {
                invoke.resolver.reject("Браузер заблокирован. Введите мастер-пароль.");
                return true;
            }
            if app_shutdown_parent::is_draining() && !matches!(command, "app_restart" | "app_prepare_update") {
                invoke.resolver.reject("Браузер завершает работу");
                return true;
            }
            command_handler(invoke)
        })
        .build(tauri::generate_context!())
        .expect("NexusBro не запустился")
        .run(|app, sobytie| {
            // Восстановление встроенного окна после сворачивания/фокуса:
            // чужое окно само не возвращается — показываем и ставим на место.
            match &sobytie {
                tauri::RunEvent::ExitRequested{api,code,..}=>{
                    if !app_shutdown_parent::may_exit(){api.prevent_exit();app_shutdown_parent::begin(app,code.unwrap_or(0));}
                }
                tauri::RunEvent::WindowEvent{label,event:tauri::WindowEvent::CloseRequested{api,..},..} if label=="main"=>{
                    let s:State<Sostoyanie>=app.state();
                    if !s.okna.0.lock().unwrap().is_empty(){
                        api.prevent_close();
                        let channels=s.vkladki.lock().unwrap().values().filter_map(|v|v.kanal.clone()).collect::<Vec<_>>();
                        for channel in channels{let _=channel.try_send(sessiya::Komanda::GlavnoeSkryto);}
                        if let Some(w)=app.get_webview_window("main"){let _=w.hide();}
                    } else if !app_shutdown_parent::may_exit() {
                        api.prevent_close();
                        app_shutdown_parent::begin(app,0);
                    }
                }
                tauri::RunEvent::WindowEvent{label,event:tauri::WindowEvent::CloseRequested{api,..},..} if label.starts_with("browser-")=>{
                    let s:State<Sostoyanie>=app.state();
                    let last_secondary={let windows=s.okna.0.lock().unwrap();windows.len()==1&&windows.contains_key(label)};
                    let hidden_main=app.get_webview_window("main").is_some_and(|w|!w.is_visible().unwrap_or(true));
                    if !app_shutdown_parent::may_exit()&&(app_shutdown_parent::is_draining()||(last_secondary&&hidden_main)){
                        api.prevent_close();
                        if !app_shutdown_parent::is_draining(){app_shutdown_parent::begin(app,0);}
                    }else if okna_app::nachat_zakrytie(app,label){api.prevent_close();}
                }
                tauri::RunEvent::WindowEvent{label,event:tauri::WindowEvent::Destroyed,..} if label.starts_with("browser-")=>{okna_app::zakryt(app,label);}
                tauri::RunEvent::WindowEvent{label,event:tauri::WindowEvent::Focused(true)|tauri::WindowEvent::Resized(_),..} if label.starts_with("browser-")=>{
                    let s:State<Sostoyanie>=app.state();okna_app::obolochka(app,&s,label);
                }
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
            if okno_zhivo && !app_shutdown_parent::is_draining() && !browser_lock::locked_global() && !AVTOSTART_SDELAN.swap(true, std::sync::atomic::Ordering::SeqCst)
            {
                // При запуске поднимается ТОЛЬКО вкладка, которая была
                // активна при закрытии (10.11.2026, задал user). Остальные
                // спят до клика — вкладки не жрут память понапрасну.
                let h = app.clone();
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_millis(600));
                    if browser_lock::locked_global() { return; }
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
                let deti: Vec<_> = {
                    let s: tauri::State<Sostoyanie> = app.state();
                    let mut v = s.vkladki.lock().unwrap();
                    v.values_mut()
                        .map(|vk| (vk.proc.take(),vk.nash_proxy.take(),vk.tunnel.take()))
                        .collect()
                };
                for (d,p,t) in deti { ostanovit_resursy(d,p,t); }
                perevod::otmenit_vse();
                { let s: tauri::State<Sostoyanie> = app.state(); s.routes.stop_all(); }
                privatnost::ochistit();
            }
        });
}
