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
}

fn da_po_umolchaniyu() -> bool {
    true
}

fn domashnyaya() -> String {
    "about:blank".into()
}

/// Файл со списком супер-вкладок. Лежит рядом с профилями.
fn fajl_spiska() -> std::path::PathBuf {
    brauzer::papka_profilej()
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."))
        .join("vkladki.json")
}

fn zagruzit_spisok() -> Vec<Nastrojki> {
    std::fs::read_to_string(fajl_spiska())
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
}

#[derive(Default)]
struct Sostoyanie {
    vkladki: Mutex<HashMap<String, Vkladka>>,
    /// Где на экране показывать движок — это место нам сообщает лицевая часть.
    mesto: Mutex<Option<Mesto>>,
    aktivnaya: Mutex<Option<String>>,
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
            zapushchena: x.proc.is_some(),
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
/// Показываем обратно только АКТИВНУЮ вкладку: неактивные не должны
/// выползти поверх.
#[tauri::command]
fn pokazat_dvizhki(s: State<Sostoyanie>, vidno: bool) -> Otvet {
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
            Vkladka { nastrojki: nastrojki.clone(), proc: None, okno: None, nash_proxy: None, tunnel: None },
        );
        let vse: Vec<Nastrojki> = v.values().map(|x| x.nastrojki.clone()).collect();
        sohranit_spisok(&vse);
    }
    zapustit_vnutri(&app, &s, &id)
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

    let mut ditya = match brauzer::zapustit(&id, Some(&cherez), &nastrojki.adres) {
        Ok(c) => c,
        Err(e) => {
            nash.ostanovit();
            return net(e);
        }
    };
    let pid = ditya.id();

    // Окно ищем кусками: если движок умер сразу (или его команда
    // перехватилась сиротой от прошлого раза), узнаем об этом за секунды,
    // а не через 20 с тупого ожидания. ⛔ Уходя с ошибкой, движка НЕ
    // оставляем жить: живой процесс с тем же профилем ломает следующие
    // запуски — Chromium пересылает команду ему, а он открывает ОБЫЧНОЕ
    // окно поверх всего.
    let okno = {
        let nachalo = std::time::Instant::now();
        loop {
            match sistema::najti_okno(pid, 5) {
                Ok(o) => break o,
                Err(e) => {
                    if let Ok(Some(kod)) = ditya.try_wait() {
                        nash.ostanovit();
                        if let Some(mut t) = tunnel {
                            let _ = t.kill();
                            let _ = t.wait();
                        }
                        return so_oshibkoj_dvizhka(
                            &id,
                            format!("движок умер сразу после запуска ({})", kod),
                        );
                    }
                    if nachalo.elapsed().as_secs() >= 20 {
                        let _ = ditya.kill();
                        let _ = ditya.wait();
                        nash.ostanovit();
                        if let Some(mut t) = tunnel {
                            let _ = t.kill();
                            let _ = t.wait();
                        }
                        return so_oshibkoj_dvizhka(
                            &id,
                            format!("движок запустился, но окно не нашлось: {}", e),
                        );
                    }
                }
            }
        }
    };

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

    if let Some(v) = s.vkladki.lock().unwrap().get_mut(id) {
        v.proc = Some(ditya);
        v.okno = Some(okno);
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
    // ⛔ Полоса вкладок должна узнавать об изменениях, сделанных ядром, —
    // иначе вкладка есть, а в полосе её нет (ловили на самопроверке).
    let _ = app.emit("vkladki-izmenilis", id.to_string());
    da()
}

#[tauri::command]
fn pereklyuchit(app: tauri::AppHandle, s: State<Sostoyanie>, id: String) -> Otvet {
    let zapushchena = match s.vkladki.lock().unwrap().get(&id) {
        Some(v) => v.proc.is_some(),
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
}

/// Остановить движок, но вкладку и её профиль сохранить.
fn ostanovit_vnutri(app: &tauri::AppHandle, s: &State<Sostoyanie>, id: &str) {
    let mut v = s.vkladki.lock().unwrap();
    if let Some(vk) = v.get_mut(id) {
        // Открепляем очередь ввода — иначе после остановки наш ввод
        // останется привязан к потоку чужого окна.
        if let Some(o) = vk.okno.take() {
            let _ = app.run_on_main_thread(move || sistema::otsoedinit_vvod(o));
        }
        if let Some(p) = vk.proc.as_mut() {
            let _ = p.kill();
            let _ = p.wait();
        }
        vk.proc = None;
        vk.okno = None;
        if let Some(p) = vk.nash_proxy.take() {
            p.ostanovit();
        }
        if let Some(mut t) = vk.tunnel.take() {
            let _ = t.kill();
            let _ = t.wait();
        }
    }
}

#[tauri::command]
fn ostanovit(app: tauri::AppHandle, s: State<Sostoyanie>, id: String) -> Otvet {
    if s.vkladki.lock().unwrap().contains_key(&id) {
        ostanovit_vnutri(&app, &s, &id);
        da()
    } else {
        net("такой супер-вкладки нет")
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

/// Удалить вкладку насовсем вместе с её профилем.
/// ⛔ В профиле лежат входы в почту и прочее — спрашивать подтверждение
/// обязан тот, кто вызывает.
#[tauri::command]
fn udalit(app: tauri::AppHandle, s: State<Sostoyanie>, id: String) -> Otvet {
    let mut v = s.vkladki.lock().unwrap();
    match v.remove(&id) {
        Some(mut vk) => {
            if let Some(o) = vk.okno.take() {
                let _ = app.run_on_main_thread(move || sistema::otsoedinit_vvod(o));
            }
            for p in [vk.proc.as_mut(), vk.tunnel.as_mut()].into_iter().flatten() {
                let _ = p.kill();
                let _ = p.wait();
            }
            if let Some(p) = vk.nash_proxy.take() {
                p.ostanovit();
            }
            let _ = std::fs::remove_dir_all(brauzer::papka_profilej().join(&id));
            let vse: Vec<Nastrojki> = v.values().map(|x| x.nastrojki.clone()).collect();
            sohranit_spisok(&vse);
            da()
        }
        None => net("такой супер-вкладки нет"),
    }
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
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(Sostoyanie::default())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_process::init())
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
                    v.insert(n.id.clone(), Vkladka { nastrojki: n, proc: None, okno: None, nash_proxy: None, tunnel: None });
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
                    };
                    let o = otkryt(h.clone(), s, n);
                    println!("САМОПРОВЕРКА: ok={} {}", o.ok, o.soobshchenie);
                });
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            spisok, aktivnaya, schet_rezki, otkryt, pereklyuchit, ostanovit, udalit,
            zadat_mesto, dvizhok_est, pokazat_dvizhki, izmenit
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
            // ⛔ При закрытии программы движки ОСТАЮТСЯ жить: поймали
            // 06.11.2026 — закрыли окно, в памяти висело 13 процессов
            // браузера. Гасим все при выходе.
            if let tauri::RunEvent::Exit = sobytie {
                let s: tauri::State<Sostoyanie> = app.state();
                let mut v = s.vkladki.lock().unwrap();
                for vk in v.values_mut() {
                    for pr in [vk.proc.as_mut(), vk.tunnel.as_mut()].into_iter().flatten() {
                        let _ = pr.kill();
                        let _ = pr.wait();
                    }
                }
            }
        });
}
