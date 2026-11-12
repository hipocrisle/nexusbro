//! NexusBro CEF-спайк.
//!
//! Окно — наше (winit). Движок — CEF, созданный СРАЗУ дочерним для нашего
//! окна (`WindowInfo::set_as_child` — штатный режим CEF, в отличие от
//! встраивания чужого окна chromium). Трафик движка — через мини-прокси
//! на 127.0.0.1:8899: страница-маркер доказывает, что прокси действительно
//! посередине. Если это работает — архитектура «супер-вкладка = дочерний
//! CEF с --user-data-dir и --proxy-server» жизнеспособна.

use cef::*;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use winit::event::{Event, WindowEvent};
use winit::event_loop::{ControlFlow, EventLoop};

/// Хэндл нашего окна: X11 xid (Linux) или HWND (Windows).
static RODITEL: OnceLock<u64> = OnceLock::new();
static RAZMER: OnceLock<(u32, u32)> = OnceLock::new();
/// Созданный движок — чтобы реагировать на ресайз и закрытие.
static DVIZHOK: Mutex<Option<cef::Browser>> = Mutex::new(None);
static PROSIT_VYHOD: AtomicBool = AtomicBool::new(false);

// ─────────────────────────── мини-прокси ───────────────────────────
// Любой запрос → тёмная страница-маркер. Соединение закрываем сразу:
// это спайк, не продакшен.

fn pustit_proksi() {
    std::thread::spawn(|| {
        let slushatel =
            std::net::TcpListener::bind("127.0.0.1:8899").expect("прокси: порт 8899 занят");
        for potok in slushatel.incoming() {
            std::thread::spawn(move || {
                use std::io::{Read, Write};
                let Ok(mut potok) = potok else { return };
                let mut buf = [0u8; 8192];
                let n = potok.read(&mut buf).unwrap_or(0);
                let zapros = String::from_utf8_lossy(&buf[..n]);
                let stroka = zapros.lines().next().unwrap_or("");
                println!("ПРОКСИ: {stroka}");
                let telo = concat!(
                    "<!doctype html><html><head><meta charset=utf-8>",
                    "<style>body{background:#14171c;color:#e6e6e6;font-family:system-ui;",
                    "display:flex;height:100vh;margin:0;align-items:center;justify-content:center}",
                    "h1{font-size:22px}</style></head><body><div>",
                    "<h1>NexusBro: страница отдана через наш прокси</h1>",
                    "<p>Движок — CEF, рождённый дочерним для нашего окна. ",
                    "Рамок, кнопок и чужих окон быть не должно.</p>",
                    "</div></body></html>"
                );
                let otvet = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    telo.len(),
                    telo
                );
                let _ = potok.write_all(otvet.as_bytes());
            });
        }
    });
}

// ─────────────────────────── клиент CEF ───────────────────────────

wrap_client! {
    pub struct SpikeClient;

    impl Client {}
}

// ───────────────────── создание движка после init ─────────────────────

wrap_browser_process_handler! {
    struct SpikeBrowserProcessHandler;

    impl BrowserProcessHandler {
        fn on_context_initialized(&self) {
            let (Some(&roditel), Some(razmer)) = (
                RODITEL.get(),
                RAZMER.get(),
            ) else {
                eprintln!("спайк: окно ещё не создано");
                return;
            };
            #[cfg(target_os = "linux")]
            let roditel_handle: cef::sys::cef_window_handle_t = roditel;
            #[cfg(target_os = "windows")]
            let roditel_handle: cef::sys::cef_window_handle_t =
                cef::sys::HWND(roditel as usize as *mut cef::sys::HWND__);
            #[cfg(not(any(target_os = "linux", target_os = "windows")))]
            let roditel_handle: cef::sys::cef_window_handle_t = roditel as usize as cef::sys::cef_window_handle_t;
            let info = WindowInfo::default().set_as_child(
                roditel_handle,
                &Rect {
                    x: 0,
                    y: 0,
                    width: razmer.0 as i32,
                    height: razmer.1 as i32,
                },
            );
            let mut klient = SpikeClient::new();
            let url = CefString::from("http://spike.nexusbro/");
            let nastrojki = BrowserSettings::default();
            if let Some(dvizhok) = browser_host_create_browser_sync(
                Some(&info),
                Some(&mut klient),
                Some(&url),
                Some(&nastrojki),
                None,
                None,
            ) {
                *DVIZHOK.lock().unwrap() = Some(dvizhok);
                println!("СПАЙК: движок создан и встроен в наше окно");
            } else {
                eprintln!("СПАЙК: движок НЕ создался");
            }
        }
    }
}

wrap_app! {
    pub struct SpikeApp;

    impl App {
        fn browser_process_handler(&self) -> Option<cef::BrowserProcessHandler> {
            Some(SpikeBrowserProcessHandler::new())
        }

        fn on_before_command_line_processing(
            &self,
            tip_processa: Option<&cef::CefString>,
            komandnaya_stroka: Option<&mut cef::CommandLine>,
        ) {
            // Все процессы движка: прокси и тёмная тема движка.
            if let Some(ks) = komandnaya_stroka {
                ks.append_switch_with_value(
                    Some(&cef::CefString::from("proxy-server")),
                    Some(&cef::CefString::from("http://127.0.0.1:8899")),
                );
                ks.append_switch(Some(&cef::CefString::from("force-dark-mode")));
                // Спайк без песочницы (crate собран без sandbox-фичи).
                ks.append_switch(Some(&cef::CefString::from("no-sandbox")));
            }
            let _ = tip_processa;
        }
    }
}

fn main() {
    // Инициализация версии CEF API — без этого libcef падает с trap.
    let _ = api_hash(sys::CEF_API_VERSION_LAST, 0);

    // Подпроцессы движка — перезапуск этого же exe с --type=...
    // ⛔ Только БРАУЗЕРНЫЙ процесс несёт прокси и окно: рендереры
    // перезапускают этот же exe и тоже попадут в main().
    let args = cef::args::Args::new();
    let Some(komandnaya_stroka) = args.as_cmd_line() else {
        eprintln!("спайк: не разобрал аргументы");
        std::process::exit(1);
    };
    let rebyonok = cef::execute_process(Some(args.as_main_args()), None, std::ptr::null_mut());
    if rebyonok >= 0 {
        std::process::exit(rebyonok);
    }

    // Мы — браузерный процесс.
    pustit_proksi();

    // Наше окно — ДО initialize, на главном потоке (winit требует).
    let event_loop = EventLoop::new().expect("спайк: нет оконной системы");
    let okno = winit::window::WindowBuilder::new()
        .with_title("NexusBro CEF-спайк")
        .with_inner_size(winit::dpi::LogicalSize::new(1024.0, 700.0))
        .build(&event_loop)
        .expect("спайк: окно не создалось");
    let (w, h): (u32, u32) = okno.inner_size().into();
    let _ = RAZMER.set((w, h));

    // Хэндл окна — через re-export raw-window-handle из winit (0.6).
    #[cfg(target_os = "linux")]
    let roditel: u64 = {
        use winit::raw_window_handle::{HasRawWindowHandle, RawWindowHandle};
        match okno.raw_window_handle().unwrap_or_else(|_| {
            eprintln!("спайк: не получил хэндл окна");
            std::process::exit(1);
        }) {
            RawWindowHandle::Xlib(x) => x.window,
            RawWindowHandle::Xcb(x) => x.window.get() as u64,
            _ => {
                eprintln!("спайк: нужно X11-окно (Wayland не в спайке)");
                std::process::exit(1);
            }
        }
    };
    #[cfg(target_os = "windows")]
    let roditel: u64 = {
        use winit::raw_window_handle::{HasRawWindowHandle, RawWindowHandle};
        match okno.raw_window_handle().unwrap_or_else(|_| {
            eprintln!("спайк: не получил хэндл окна");
            std::process::exit(1);
        }) {
            RawWindowHandle::Win32(x) => x.hwnd.get() as u64,
            _ => {
                eprintln!("спайк: не получил HWND");
                std::process::exit(1);
            }
        }
    };
    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    let roditel: u64 = {
        eprintln!("спайк: спайк заточен под Windows и Linux");
        std::process::exit(1);
    };
    let _ = RODITEL.set(roditel);

    // Файлы движка лежат рядом с exe или в CEF_PATH (dev).
    let cef_dir = std::env::var("CEF_PATH").map(std::path::PathBuf::from).unwrap_or_else(|_| {
        std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|x| x.to_path_buf()))
            .unwrap_or_default()
    });
    let mut nastrojki = cef::Settings::default();
    nastrojki.no_sandbox = 1;
    nastrojki.external_message_pump = 1;
    nastrojki.resources_dir_path = cef::CefString::from(cef_dir.to_string_lossy().as_ref());
    nastrojki.locales_dir_path = cef::CefString::from(
        cef_dir.join("locales").to_string_lossy().as_ref(),
    );
    nastrojki.root_cache_path =
        cef::CefString::from(std::env::temp_dir().join("nb-cef-spike").to_string_lossy().as_ref());

    let mut prilozhenie = SpikeApp::new();
    if cef::initialize(
        Some(args.as_main_args()),
        Some(&nastrojki),
        Some(&mut prilozhenie),
        std::ptr::null_mut(),
    ) != 1
    {
        eprintln!("спайк: CEF не инициализировался (где файлы движка?)");
        std::process::exit(1);
    }

    let itog = event_loop.run(move |sobytie, cel| {
        cel.set_control_flow(ControlFlow::WaitUntil(
            std::time::Instant::now() + std::time::Duration::from_millis(8),
        ));
        match sobytie {
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => {
                // Вежливо гасим движок, потом выходим после цикла.
                if let Some(d) = DVIZHOK.lock().unwrap().take() {
                    if let Some(h) = d.host() {
                        h.close_browser(1);
                    }
                }
                PROSIT_VYHOD.store(true, Ordering::SeqCst);
                cel.exit();
            }
            Event::WindowEvent {
                event: WindowEvent::Resized(_razmer),
                ..
            } => {
                if let Some(d) = DVIZHOK.lock().unwrap().as_ref() {
                    if let Some(h) = d.host() {
                        h.notify_move_or_resize_started();
                        h.was_resized();
                    }
                }
            }
            _ => {}
        }
        // Внешняя петля сообщений CEF.
        cef::do_message_loop_work();
        if PROSIT_VYHOD.load(Ordering::SeqCst) {
            std::process::exit(0);
        }
    });
    let _ = itog;
    cef::shutdown();
}
