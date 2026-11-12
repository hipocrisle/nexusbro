//! CEF-движок: отдельный процесс на супер-вкладку, окно рождается
//! ДОЧЕРНИМ для нашего (WindowInfo::set_as_child). Заменяет схему
//! «встраивание чужого окна chromium» — класс багов «окно живёт своей
//! жизнью» исчезает конструктивно (12.11.2026, спайк одобрен user'ом).
//!
//! ⛔ Один exe на всё: подпроцессы движка и процессы супер-вкладок —
//! перезапуски этого же exe. main() проверяет eto_process_cefa() ДО Tauri.

use cef::*;
use std::io::{BufRead, Write};

/// Это перезапуск нашего exe как процесса движка? (--type=... — подпроцесс
/// CEF; --nb-dvizhok — процесс супер-вкладки.)
pub fn eto_process_cefa() -> bool {
    std::env::args().any(|a| a.starts_with("--type=") || a == "--nb-dvizhok")
}

fn znachenie(perfix: &str) -> Option<String> {
    std::env::args()
        .find_map(|a| a.strip_prefix(perfix).map(|s| s.to_string()))
}

/// Вход для процессов движка. Вызывается из main ДО Tauri.
/// Код возврата — код выхода процесса.
pub fn process_vhod() -> i32 {
    // ⛔ Без инициализации версии API libcef падает с trap (спайк).
    let _ = api_hash(sys::CEF_API_VERSION_LAST, 0);
    let args = args::Args::new();
    let Some(komandnaya_stroka) = args.as_cmd_line() else {
        eprintln!("dvizhok: не разобрал аргументы");
        return 1;
    };
    // Подпроцесс движка (renderer/gpu/utility/...): выполнить и выйти.
    if std::env::args().any(|a| a.starts_with("--type=")) {
        let kod = execute_process(Some(args.as_main_args()), None, std::ptr::null_mut());
        return if kod >= 0 { kod } else { 0 };
    }
    // Процесс супер-вкладки.
    process_supertab(args.as_main_args(), &komandnaya_stroka)
}

wrap_app! {
    struct DvizhokApp;

    impl App {}
}

wrap_client! {
    struct DvizhokClient;

    impl Client {}
}

/// Процесс одной супер-вкладки: поднимает CEF с СВОИМ профилем и прокси,
/// создаёт окно дочерним к нашему, сообщает хэндл в stdout, крутит петлю.
fn process_supertab(main_args: &MainArgs, _komandnaya_stroka: &CommandLine) -> i32 {
    let roditel: u64 = znachenie("--nb-roditel=")
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let rect: Vec<i32> = znachenie("--nb-rect=")
        .map(|s| s.split(',').filter_map(|x| x.parse().ok()).collect())
        .unwrap_or_default();
    let (x, y, w, h) = match rect.as_slice() {
        [x, y, w, h] => (*x, *y, *w, *h),
        _ => (0, 0, 1200, 700),
    };
    let profil = znachenie("--user-data-dir=").unwrap_or_else(|| "nb-cefp".into());
    let url = znachenie("--nb-url=").unwrap_or_else(|| "about:blank".into());
    if roditel == 0 {
        eprintln!("dvizhok: нет --nb-roditel");
        return 1;
    }

    // Файлы движка: рядом с exe или CEF_PATH (dev).
    let cef_dir = std::env::var("CEF_PATH")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| {
            std::env::current_exe()
                .ok()
                .and_then(|p| p.parent().map(|x| x.to_path_buf()))
                .unwrap_or_default()
        });
    let mut nastrojki = Settings::default();
    nastrojki.no_sandbox = 1;
    nastrojki.external_message_pump = 1;
    nastrojki.resources_dir_path = CefString::from(cef_dir.to_string_lossy().as_ref());
    nastrojki.locales_dir_path =
        CefString::from(cef_dir.join("locales").to_string_lossy().as_ref());
    nastrojki.root_cache_path = CefString::from(profil.as_str());
    nastrojki.cache_path = CefString::from(
        std::path::Path::new(&profil)
            .join("kesh")
            .to_string_lossy()
            .as_ref(),
    );

    let mut prilozhenie = DvizhokApp::new();
    if initialize(
        Some(main_args),
        Some(&nastrojki),
        Some(&mut prilozhenie),
        std::ptr::null_mut(),
    ) != 1
    {
        eprintln!("dvizhok: CEF не инициализировался (файлы движка?)");
        return 1;
    }

    #[cfg(target_os = "linux")]
    let roditel_handle: sys::cef_window_handle_t = roditel;
    #[cfg(target_os = "windows")]
    let roditel_handle: sys::cef_window_handle_t =
        sys::HWND(roditel as usize as *mut sys::HWND__);
    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    let roditel_handle: sys::cef_window_handle_t =
        roditel as usize as sys::cef_window_handle_t;

    let info = WindowInfo::default().set_as_child(
        roditel_handle,
        &Rect {
            x,
            y,
            width: w,
            height: h,
        },
    );
    let mut klient = DvizhokClient::new();
    let url_cef = CefString::from(url.as_str());
    let nastrojki_brauzera = BrowserSettings::default();
    let Some(dvizhok) = browser_host_create_browser_sync(
        Some(&info),
        Some(&mut klient),
        Some(&url_cef),
        Some(&nastrojki_brauzera),
        None,
        None,
    ) else {
        eprintln!("dvizhok: браузер не создался");
        return 1;
    };
    // Хэндл окна приводим к u32 независимо от платформы: на Linux это
    // XID (u64), на Windows — HWND (указатель).
    let okno = dvizhok
        .host()
        .map(|x| {
            #[cfg(target_os = "linux")]
            {
                x.window_handle() as u32
            }
            #[cfg(target_os = "windows")]
            {
                x.window_handle().0 as usize as u32
            }
            #[cfg(not(any(target_os = "linux", target_os = "windows")))]
            {
                x.window_handle() as usize as u32
            }
        })
        .unwrap_or(0);
    // Родитель ждёт эту строку, чтобы знать хэндл окна.
    println!("NB-OKNO {okno}");
    let _ = std::io::stdout().flush();

    // Внешняя петля сообщений: движок живёт, пока родитель не убьёт процесс.
    loop {
        do_message_loop_work();
        std::thread::sleep(std::time::Duration::from_millis(8));
    }
}

/// Запустить процесс супер-вкладки. Прокси движку — командной строкой
/// (⛔ прокси в CEF только так: на весь процесс).
pub fn zapustit_cep(
    id: &str,
    proxy: &str,
    url: Option<&str>,
    roditel: u64,
    m: crate::okna::Mesto,
) -> anyhow::Result<std::process::Child> {
    let exe = std::env::current_exe()?;
    let profil = crate::brauzer::papka_profilej().join(id);
    std::fs::create_dir_all(&profil)?;
    let stdout = std::process::Stdio::piped();
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("--nb-dvizhok")
        .arg(format!("--user-data-dir={}", profil.display()))
        .arg(format!("--proxy-server={}", proxy))
        .arg(format!("--nb-roditel={roditel}"))
        .arg(format!("--nb-rect={},{},{},{}", m.x, m.y, m.w, m.h))
        .stdout(stdout)
        .stderr(std::process::Stdio::null());
    if let Some(u) = url {
        cmd.arg(format!("--nb-url={u}"));
    }
    #[cfg(target_os = "linux")]
    unsafe {
        use std::os::unix::process::CommandExt;
        cmd.pre_exec(|| {
            libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL);
            Ok(())
        });
    }
    let ditya = cmd.spawn()?;
    #[cfg(target_os = "windows")]
    crate::brauzer::privyazat_k_zadanie_pub(&ditya)?;
    Ok(ditya)
}

/// Дождаться строки «NB-OKNO <hwnd>» от процесса супер-вкладки.
/// Хэндл окна движка — чтобы показывать/прятать/двигать его как раньше.
pub fn zhdat_okno(ditya: &mut std::process::Child, sek: u64) -> Option<u32> {
    let srok = std::time::Instant::now();
    let stdout = ditya.stdout.take()?;
    let chitat = std::io::BufReader::new(stdout);
    for stroka in chitat.lines() {
        if let Ok(s) = stroka {
            if let Some(h) = s.strip_prefix("NB-OKNO ") {
                return h.trim().parse().ok();
            }
        }
        if srok.elapsed().as_secs() >= sek {
            return None;
        }
        if matches!(ditya.try_wait(), Ok(Some(_))) {
            return None;
        }
    }
    None
}
