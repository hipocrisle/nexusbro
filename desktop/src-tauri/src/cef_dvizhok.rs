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

    impl App {
        fn on_before_command_line_processing(
            &self,
            _tip_processa: Option<&CefString>,
            komandnaya_stroka: Option<&mut CommandLine>,
        ) {
            if let Some(ks) = komandnaya_stroka {
                // ⛔ GPU-процесс падал в цикле на машине user'а (12.11.2026):
                // программный рендер — медленнее, но стабилен везде.
                ks.append_switch(Some(&CefString::from("disable-gpu")));
                // Движок не должен стучаться за обновлениями к Google.
                ks.append_switch(Some(&CefString::from("disable-component-update")));
                // Движок — тёмный, как вся программа.
                ks.append_switch(Some(&CefString::from("force-dark-mode")));
            }
        }
    }
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
    // Журнал этапов: родитель смотрит этот файл, если окно не появилось.
    // На Windows stderr уже перенаправлен родителем в этот же файл.
    #[cfg(target_os = "linux")]
    if let Ok(f) = std::fs::File::create(std::path::Path::new(&profil).join("cef.log")) {
        use std::os::unix::io::AsRawFd;
        let _ = unsafe { libc::dup2(f.as_raw_fd(), 2) };
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

    eprintln!("cef: initialize…");
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

    #[cfg(target_os = "windows")]
    let roditel_handle: sys::cef_window_handle_t =
        sys::HWND(roditel as usize as *mut sys::HWND__);
    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    let roditel_handle: sys::cef_window_handle_t =
        roditel as usize as sys::cef_window_handle_t;

    #[cfg(target_os = "linux")]
    let (svyaz, konteyner) = match sozdat_konteyner(roditel, x, y, w, h) {
        Ok(k) => k,
        Err(e) => {
            eprintln!("cef: контейнер окна: {e}");
            return 1;
        }
    };
    #[cfg(target_os = "linux")]
    let (roditel_handle, x, y) = (konteyner as sys::cef_window_handle_t, 0, 0);
    eprintln!("cef: окно движка…");
    let info = WindowInfo::default().set_as_child(
        roditel_handle,
        &Rect { x, y, width: w, height: h },
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
    eprintln!("cef: браузер создан");
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
    #[cfg(target_os = "linux")]
    let okno_obolochki = konteyner;
    #[cfg(not(target_os = "linux"))]
    let okno_obolochki = okno;
    println!("NB-OKNO {okno_obolochki}");
    let _ = std::io::stdout().flush();

    // Внешняя петля сообщений: движок живёт, пока родитель не убьёт процесс.
    loop {
        #[cfg(target_os = "linux")]
        {
            use x11rb::connection::Connection;
            use x11rb::protocol::{Event, xproto::*};
            while let Ok(Some(sobytie)) = svyaz.poll_for_event() {
                if let Event::ConfigureNotify(r) = sobytie {
                    if r.window == konteyner {
                        let _ = svyaz.configure_window(okno, &ConfigureWindowAux::new()
                            .width(r.width as u32).height(r.height as u32));
                        let _ = svyaz.flush();
                    }
                }
            }
        }
        do_message_loop_work();
        std::thread::sleep(std::time::Duration::from_millis(8));
    }
}

#[cfg(target_os = "linux")]
fn sozdat_konteyner(
    roditel: u64, x: i32, y: i32, w: i32, h: i32,
) -> anyhow::Result<(x11rb::rust_connection::RustConnection, u32)> {
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::*;
    let (svyaz, ekran) = x11rb::connect(None)?;
    let ekran = &svyaz.setup().roots[ekran];
    let okno = svyaz.generate_id()?;
    // CEF наследует colormap родителя, но выбирает root visual; у GTK visual может отличаться.
    svyaz.create_window(ekran.root_depth, okno, roditel as u32,
        x as i16, y as i16, w as u16, h as u16, 0,
        WindowClass::INPUT_OUTPUT, ekran.root_visual,
        &CreateWindowAux::new().colormap(ekran.default_colormap)
            .event_mask(EventMask::STRUCTURE_NOTIFY))?.check()?;
    svyaz.flush()?;
    Ok((svyaz, okno))
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
    crate::brauzer::ochistit_blokirovki(id);
    let stdout = std::process::Stdio::piped();
    // ⛔ stderr — в файл: при зависании/падении дочка причина видна
    // (раньше был null, и диагностика слепая).
    let stderr = std::fs::File::create(profil.join("cef.log"))
        .map(std::process::Stdio::from)
        .unwrap_or(std::process::Stdio::null());
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("--nb-dvizhok")
        .arg(format!("--user-data-dir={}", profil.display()))
        .arg(format!("--proxy-server={}", proxy))
        .arg(format!("--nb-roditel={roditel}"))
        .arg(format!("--nb-rect={},{},{},{}", m.x, m.y, m.w, m.h))
        .stdout(stdout)
        .stderr(stderr);
    if let Some(u) = url {
        cmd.arg(format!("--nb-url={u}"));
    }
    #[cfg(target_os = "linux")]
    unsafe {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
        cmd.pre_exec(|| {
            libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL);
            Ok(())
        });
    }
    let ditya = crate::brauzer::zapustit_cef(cmd)?;
    #[cfg(target_os = "windows")]
    let mut ditya = ditya;
    #[cfg(target_os = "windows")]
    if let Err(e) = crate::brauzer::privyazat_k_zadanie_pub(&ditya) {
        let _ = ditya.kill();
        let _ = ditya.wait();
        return Err(e);
    }
    Ok(ditya)
}

pub fn ostanovit_process(mut ditya: std::process::Child) {
    #[cfg(target_os = "linux")]
    unsafe {
        libc::kill(-(ditya.id() as i32), libc::SIGKILL);
    }
    let _ = ditya.kill();
    let _ = ditya.wait();
}

/// Дождаться строки «NB-OKNO <hwnd>» от процесса супер-вкладки.
/// ⛔ Таймаут честный: чтение stdout идёт в отдельном потоке, основной
/// ждёт через recv_timeout — иначе при молчащем дочке ждали бы вечно
/// (намертво висло при создании вкладки, 12.11.2026). Вызывать ТОЛЬКО
/// из фонового потока: команды Tauri крутятся на главном потоке, а
/// дочка при создании окна ждёт ответа родительского — взаимный
/// дедлок иначе неизбежен.
pub fn dozhdatsya_okna(
    stdout: std::process::ChildStdout,
    sek: u64,
) -> Option<u32> {
    let (tx, rx) = std::sync::mpsc::channel::<Option<u32>>();
    std::thread::spawn(move || {
        let chitat = std::io::BufReader::new(stdout);
        for stroka in chitat.lines() {
            match stroka {
                Ok(s) => {
                    if let Some(h) = s.strip_prefix("NB-OKNO ") {
                        let _ = tx.send(h.trim().parse().ok());
                        return;
                    }
                }
                Err(_) => break,
            }
        }
        let _ = tx.send(None);
    });
    match rx.recv_timeout(std::time::Duration::from_secs(sek)) {
        Ok(okno) => okno,
        Err(_) => None,
    }
}
