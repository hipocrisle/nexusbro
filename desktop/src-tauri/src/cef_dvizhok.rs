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
    std::env::args().find_map(|a| a.strip_prefix(perfix).map(|s| s.to_string()))
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
        let mut prilozhenie = DvizhokApp::new();
        let kod = execute_process(Some(args.as_main_args()), Some(&mut prilozhenie), std::ptr::null_mut());
        return if kod >= 0 { kod } else { 0 };
    }
    crate::password_cef::enabled(std::env::args().any(|a|a=="--nb-passwords"));
    // Процесс супер-вкладки.
    process_supertab(args.as_main_args(), &komandnaya_stroka)
}

wrap_app! {
    struct DvizhokApp;

    impl App {
        fn render_process_handler(&self) -> Option<RenderProcessHandler> {
            Some(crate::cef_memory_renderer::handler())
        }
        fn on_before_command_line_processing(
            &self,
            _tip_processa: Option<&CefString>,
            komandnaya_stroka: Option<&mut CommandLine>,
        ) {
            // Child processes previously received no CefApp. Preserve their
            // inherited flags; the new renderer observer must not alter GPU
            // or networking switches in a subprocess.
            if _tip_processa.is_some_and(|p| !p.to_string().is_empty()) { return; }
            if let Some(ks) = komandnaya_stroka {
                // ⛔ GPU-процесс падал в цикле на машине user'а (12.11.2026):
                // программный рендер — медленнее, но стабилен везде.
                if !std::env::args().any(|a| a == "--nb-gpu") {
                    ks.append_switch(Some(&CefString::from("disable-gpu")));
                }
                // Движок не должен стучаться за обновлениями к Google.
                ks.append_switch(Some(&CefString::from("disable-component-update")));
                // 17.11.2026: фоновый обмен с Google (метрики, «нужно ли
                // обновиться») и промо-подсказки страниц — выключаем, как в
                // ungoogled-chromium. Плашки «загрузите Chrome/Firefox» —
                // это разметка самих сайтов, их убирает borba_s_reklamoy.
                for flag in [
                    "disable-background-networking",
                    "disable-breakpad",
                    "disable-domain-reliability",
                    "disable-sync",
                    "metrics-recording-only",
                    "no-pings",
                    "no-default-browser-check",
                    "disable-prompt-on-repost",
                    "disable-features=Translate,MediaRouter,OptimizationHints,InterestFeedContentSuggestions,AutofillServerCommunication,PrivacySandboxSettings4,TranslateUI",
                ] {
                    ks.append_switch(Some(&CefString::from(flag)));
                }
                // Route transports are HTTP CONNECT/SOCKS TCP; prohibit direct
                // UDP paths that would otherwise bypass a per-context proxy.
                ks.append_switch(Some(&CefString::from("disable-quic")));
                // Speculative DNS is disabled per profile before admission.
                // Keep ordinary Direct WebRTC/STUN hostname resolution intact.
                // Движок — тёмный, как вся программа.
                ks.append_switch(Some(&CefString::from("force-dark-mode")));
            }
        }
    }
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
    let url = znachenie("--nb-url=");
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
    #[cfg(target_os = "linux")]
    {
        nastrojki.external_message_pump = 1;
    }
    // Windows — CefRunMessageLoop, без external pump и без второй UI-нити.
    #[cfg(target_os = "windows")]
    {
        nastrojki.external_message_pump = 0;
    }
    nastrojki.log_file = CefString::from(
        std::path::Path::new(&profil)
            .join("cef-engine.log")
            .to_string_lossy()
            .as_ref(),
    );
    nastrojki.resources_dir_path = CefString::from(cef_dir.to_string_lossy().as_ref());
    nastrojki.locales_dir_path =
        CefString::from(cef_dir.join("locales").to_string_lossy().as_ref());
    nastrojki.root_cache_path = CefString::from(profil.as_str());
    // Every Browser uses an explicit guarded context. Keep root (Windows key
    // material), leave global disk cache empty, reopen actual old Default unchanged.
    nastrojki.cache_path = CefString::default();
    let bootstrap = (|| -> anyhow::Result<crate::tab_routes::RouteBootstrap> {
        let path=znachenie("--nb-route-bootstrap=").ok_or_else(||anyhow::anyhow!("Нет начальных маршрутов"))?;
        let file=std::fs::File::open(path)?;
        anyhow::ensure!(file.metadata()?.len()<=131072,"Снимок маршрутов слишком велик");
        Ok(serde_json::from_reader(file)?)
    })();
    let bootstrap=match bootstrap{Ok(value)=>value,Err(_)=>{eprintln!("cef: безопасный маршрут не подготовлен");return 1;}};
    // Журнал CEF глобален для процесса, а в нём могут быть приватные вкладки.
    // Наш журнал жизненного цикла не включает содержимое приватных страниц.
    nastrojki.log_severity=LogSeverity::DISABLE;
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

    eprintln!(
        "cef: initialized, версия {}, native_loop={}",
        env!("CARGO_PKG_VERSION"),
        cfg!(target_os = "windows")
    );
    match crate::cef_vkladki::rabotat(
        roditel,
        crate::okna::Mesto {
            x,
            y,
            w: w as u32,
            h: h as u32,
        },
        std::path::Path::new(&profil),
        url.as_deref(),
        bootstrap,
    ) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("cef: {e}");
            1
        }
    }
}

/// Запустить процесс с подготовленным снимком локальных маршрутов.
/// CLI proxy intentionally absent: it disables per-context preferences.
pub fn zapustit_cep(
    id: &str,
    _proxy: &str,
    url: Option<&str>,
    roditel: u64,
    rezat_reklamu: bool,
    m: crate::okna::Mesto,
    bootstrap: &crate::tab_routes::RouteBootstrap,
) -> anyhow::Result<std::process::Child> {
    let exe = std::env::current_exe()?;
    let profil = crate::brauzer::papka_profilya(id);
    std::fs::create_dir_all(&profil)?;
    crate::brauzer::ochistit_blokirovki(id);
    anyhow::ensure!(bootstrap.inherit.validate(),"Недопустимый начальный маршрут");
    let snapshot=serde_json::to_vec(bootstrap)?;
    anyhow::ensure!(snapshot.len()<=131072,"Снимок маршрутов слишком велик");
    let snapshot_path=profil.join("route-bootstrap.json");
    std::fs::write(&snapshot_path,snapshot)?;
    let stdout = std::process::Stdio::piped();
    // ⛔ stderr — в файл: при зависании/падении дочка причина видна
    // (раньше был null, и диагностика слепая).
    let stderr = std::fs::File::create(profil.join("cef.log"))
        .map(std::process::Stdio::from)
        .unwrap_or(std::process::Stdio::null());
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("--nb-dvizhok")
        .arg(format!("--user-data-dir={}", profil.display()))
        .arg(format!("--nb-route-bootstrap={}", snapshot_path.display()))
        .arg(format!("--nb-roditel={roditel}"))
        .arg(format!("--nb-rect={},{},{},{}", m.x, m.y, m.w, m.h))
        .stdout(stdout)
        .stdin(std::process::Stdio::piped())
        .stderr(stderr);
    if crate::privatnost::eto_id(id){cmd.arg("--nb-private-profile");}
    if crate::passwords::enabled(){cmd.arg("--nb-passwords");}
    let (browser_locked,lock_generation)=crate::browser_lock::engine_state();
    if browser_locked{cmd.arg("--nb-browser-locked");}
    cmd.arg(format!("--nb-lock-generation={lock_generation}"));
    if rezat_reklamu { cmd.arg("--nb-rezat-reklamu"); }
    if crate::nastrojki::zagruzit().apparatnoe_uskorenie {
        cmd.arg("--nb-gpu");
    }
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
    sessiya: impl Fn(crate::sessiya::Sessiya) + Send + 'static,
    adresnyi_vvod: impl Fn() + Send + 'static,
    sluzhebnoe: impl Fn(serde_json::Value) + Send + 'static,
) -> Option<u32> {
    let (tx, rx) = std::sync::mpsc::channel::<Option<u32>>();
    std::thread::spawn(move || {
        let chitat = std::io::BufReader::new(stdout);
        for stroka in chitat.lines() {
            match stroka {
                Ok(s) => {
                    if let Some(json) = s.strip_prefix("NB-UI ") {
                        if let Ok(data) = serde_json::from_str(json) { sluzhebnoe(data); }
                    } else if s == "NB-ADRES" { adresnyi_vvod(); }
                    else if let Some(h) = s.strip_prefix("NB-OKNO ") {
                        let _ = tx.send(h.trim().parse().ok());
                    } else if let Some(json) = s.strip_prefix("NB-VKLADKI ") {
                        if let Ok(s) = serde_json::from_str(json) {
                            sessiya(s);
                        }
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

pub fn kanal_komand(
    mut stdin: std::process::ChildStdin,
) -> std::sync::mpsc::SyncSender<crate::sessiya::Komanda> {
    let (tx, rx) = std::sync::mpsc::sync_channel::<crate::sessiya::Komanda>(32);
    std::thread::spawn(move || {
        for k in rx {
            let Ok(json) = serde_json::to_string(&k) else {
                continue;
            };
            if writeln!(stdin, "{json}")
                .and_then(|_| stdin.flush())
                .is_err()
            {
                break;
            }
        }
    });
    tx
}
