//! Запуск движка. Движок у нас ЧУЖОЙ и сменный: на Windows и Linux кладём
//! готовый ungoogled-chromium, на маке потом возьмём установленный.
//!
//! ⛔ Прокси в Chromium задаётся НА ПРОЦЕСС, а не на вкладку. Отсюда и вся
//! затея с супер-вкладками: одна супер-вкладка = свой процесс со своим
//! профилем и своим выходом.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{channel, sync_channel, Sender, SyncSender};
use std::sync::OnceLock;

/// Куда сборка кладёт свой движок (Windows). Задаётся при запуске.
static PAPKA_RESURSOV: OnceLock<PathBuf> = OnceLock::new();

pub fn zadat_papku_resursov(p: PathBuf) {
    let _ = PAPKA_RESURSOV.set(p);
}

/// Где лежит движок. Сначала свой — положенный сборкой, потом рядом с
/// программой, потом системный.
pub fn najti_dvizhok() -> Option<PathBuf> {
    // ⛔ Движок переименован при сборке: в диспетчере задач человек должен
    // видеть НАШУ программу, а не «chrome.exe» — иначе выглядит так, будто
    // у него завёлся чужой браузер. Старое имя оставляем запасным.
    let imena: &[&str] = if cfg!(target_os = "windows") {
        &["nexusbro-engine.exe", "chrome.exe"]
    } else {
        &["nexusbro-engine", "chrome"]
    };
    if let Some(res) = PAPKA_RESURSOV.get() {
        for papka in ["chromium", "chromium/ungoogled-chromium"] {
            for imya in imena {
                let put = res.join(papka).join(imya);
                if put.exists() {
                    return Some(put);
                }
            }
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(ryadom) = exe.parent() {
            let svoi: &[&str] = if cfg!(target_os = "windows") {
                &["chromium/nexusbro-engine.exe", "chromium/chrome.exe",
                  "engine/nexusbro-engine.exe", "engine/chrome.exe"]
            } else {
                &["chromium/nexusbro-engine", "chromium/chrome",
                  "engine/nexusbro-engine", "engine/chrome"]
            };
            for p in svoi {
                let put = ryadom.join(p);
                if put.exists() {
                    return Some(put);
                }
            }
        }
    }
    let sistemnye: &[&str] = if cfg!(target_os = "windows") {
        &[
            r"C:\Program Files\Chromium\Application\chrome.exe",
            r"C:\Program Files\Google\Chrome\Application\chrome.exe",
        ]
    } else {
        &[
            "/usr/bin/ungoogled-chromium",
            "/usr/bin/chromium",
            "/usr/bin/chromium-browser",
            "/usr/bin/google-chrome",
        ]
    };
    sistemnye.iter().map(Path::new).find(|p| p.exists()).map(|p| p.to_path_buf())
}

/// Папка с профилями супер-вкладок.
pub fn papka_profilej() -> PathBuf {
    let baza = dirs::data_local_dir().unwrap_or_else(|| PathBuf::from("."));
    baza.join("NexusBro").join("profili")
}

enum Zakaz {
    Dvizhok {
        id: String,
        proxy: Option<String>,
        adres: Option<String>,
        vosstanavlivat: bool,
        otvet: SyncSender<anyhow::Result<Child>>,
    },
    /// Любая другая программа — например xray для подписки.
    /// ⛔ Тоже ЧЕРЕЗ ЭТОТ поток: привязка «умри вместе с родителем»
    /// крепится к потоку, см. пояснение ниже.
    Prosto {
        programma: PathBuf,
        dovody: Vec<String>,
        otvet: SyncSender<anyhow::Result<Child>>,
    },
}

/// ⛔ Запускать движки обязан ОДИН долгоживущий поток.
/// «Умри вместе с родителем» в Linux привязана к ПОТОКУ, а не к программе:
/// поток, запустивший браузер, завершился — система тут же убила браузер
/// (поймали 06.11.2026, в журнале остался `[chrome] <defunct>`).
/// Этот поток живёт столько же, сколько программа, поэтому привязка верна.
static ZAPUSKATEL: OnceLock<Sender<Zakaz>> = OnceLock::new();

fn zapuskatel() -> &'static Sender<Zakaz> {
    ZAPUSKATEL.get_or_init(|| {
        let (tx, rx) = channel::<Zakaz>();
        std::thread::Builder::new()
            .name("zapuskatel-dvizhkov".into())
            .spawn(move || {
                while let Ok(z) = rx.recv() {
                    match z {
                        Zakaz::Dvizhok { id, proxy, adres, vosstanavlivat, otvet } => {
                            let _ = otvet.send(zapustit_zdes(&id, proxy.as_deref(), adres.as_deref(), vosstanavlivat));
                        }
                        Zakaz::Prosto { programma, dovody, otvet } => {
                            let _ = otvet.send(zapustit_prosto(&programma, &dovody));
                        }
                    }
                }
            })
            .expect("не завёлся поток запуска движков");
        tx
    })
}

/// Запустить движок для одной супер-вкладки. `adres` — только для режима
/// «открывать пустой» или первого запуска; `vosstanavlivat` — вернуть
/// вкладки прошлой сессии.
pub fn zapustit(id: &str, proxy: Option<&str>, adres: Option<&str>, vosstanavlivat: bool) -> anyhow::Result<Child> {
    let (otvet, zhdem) = sync_channel(1);
    zapuskatel().send(Zakaz::Dvizhok {
        id: id.to_string(),
        proxy: proxy.map(|s| s.to_string()),
        adres: adres.map(|s| s.to_string()),
        vosstanavlivat,
        otvet,
    })?;
    // ⛔ Ожидание без края превращало неудачный запуск в вечное «Запускаю…».
    // Если поток запуска не ответил — говорим об этом прямо.
    zhdem
        .recv_timeout(std::time::Duration::from_secs(25))
        .map_err(|_| anyhow::anyhow!("поток запуска движков молчит 25 с — вероятно, антивирус держит exe"))?
}

/// Остановить движок: СНАЧАЛА чисто (Windows: WM_CLOSE с честным
/// таймаутом 3 с; Linux: SIGTERM), ждём выхода до 5 с, в крайнем
/// случае добиваем. ⛔ Восстановление вкладок работает ТОЛЬКО после
/// чистого выхода: после kill -9 Chromium не восстанавливает сессию
/// ни с каким флагом (проверено опытом 10.11.2026). ⛔ Убийство
/// прикреплённого (AttachThreadInput) процесса вешает нашу очередь
/// ввода — поэтому вызывать только с нашего UI-потока после открепления.
pub fn ostanovit_dvizhka(ditya: &mut Child, okno: Option<u32>) {
    let pid = ditya.id();

    #[cfg(target_os = "windows")]
    if let Some(o) = okno {
        unsafe {
            let mut otvet: usize = 0;
            // ⛔ ТОЛЬКО SMTO_ABORTIFHUNG: флаг NOTIMEOUTIFNOTHUNG ждёт
            // бесконечно, пока чужой поток жив — им и сидели в вечном
            // зависании (10.11.2026).
            let _ = windows::Win32::UI::WindowsAndMessaging::SendMessageTimeoutW(
                windows::Win32::Foundation::HWND(o as usize as *mut core::ffi::c_void),
                windows::Win32::UI::WindowsAndMessaging::WM_CLOSE,
                windows::Win32::Foundation::WPARAM(0),
                windows::Win32::Foundation::LPARAM(0),
                windows::Win32::UI::WindowsAndMessaging::SMTO_ABORTIFHUNG,
                3000,
                Some(std::ptr::from_mut(&mut otvet)),
            );
        }
    }

    #[cfg(target_os = "linux")]
    {
        // Chromium обрабатывает SIGTERM как чистое завершение.
        unsafe {
            libc::kill(pid as i32, libc::SIGTERM);
        }
    }

    let srok = std::time::Instant::now();
    loop {
        if matches!(ditya.try_wait(), Ok(Some(_))) {
            return;
        }
        if srok.elapsed().as_secs() >= 5 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(150));
    }
    let _ = ditya.kill();
    let _ = ditya.wait();
}

/// Профиль ещё пуст: ни куки, ни истории. Свежую вкладку открываем
/// по настройке «открыть сразу»; у жилой по умолчанию восстанавливаем
/// вкладки прошлой сессии.
pub fn profil_svety(id: &str) -> bool {
    let profil = papka_profilej().join(id).join("Default");
    !profil.join("Cookies").exists() && !profil.join("History").exists()
}

fn zapustit_zdes(id: &str, proxy: Option<&str>, adres: Option<&str>, vosstanavlivat: bool) -> anyhow::Result<Child> {
    let dvizhok = najti_dvizhok()
        .ok_or_else(|| anyhow::anyhow!("движок не найден: положи chromium рядом с программой"))?;
    let profil = papka_profilej().join(id);
    let zhurnal = profil.join("engine.log");
    std::fs::create_dir_all(&profil)?;
    // ⛔ Блокировки профиля: после жёсткого убийства остаются, и движок
    // на старте решает, что профиль занят, — открывает окно ОШИБКИ и
    // выходит с кодом 0 (поймал user 10.11.2026: «движок умер сразу»,
    // «новое окно с ошибкой хром»). Живого владельца в этот момент
    // быть не должно — сироты режет pribit_proshlyh_dvizhkov.
    for imya in ["SingletonLock", "SingletonSocket", "SingletonCookie"] {
        let _ = std::fs::remove_file(profil.join(imya));
    }
    let _ = std::fs::rename(&zhurnal, profil.join("engine.log.1"));
    let stdout = std::fs::File::create(&zhurnal)?;
    let stderr = stdout.try_clone()?;

    let mut cmd = Command::new(dvizhok);
    cmd.arg(format!("--user-data-dir={}", profil.display()))
        .arg("--no-first-run")
        .arg("--no-default-browser-check")
        .arg("--disable-features=Translate,MediaRouter")
        // Тёмная тема интерфейса и принудительное затемнение сайтов:
        // движок должен выглядеть как часть NexusBro, а не белым пятном
        // посреди тёмного окна (10.11.2026, задал user).
        .arg("--force-dark-mode")
        .arg("--enable-features=WebContentsForceDark")
        .arg("--enable-logging=stderr")
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr));
    // Восстановление вкладок: ⛔ настройку restore_on_startup в профиль
    // записать НЕЛЬЗЯ — движок 157 сбрасывает externally-written префы
    // (защита tracked-prefs; проверено CDP на живой машине 10.11.2026).
    // Рабочий механизм: ЧИСТЫЙ выход движка + этот флаг при старте.
    if vosstanavlivat {
        cmd.arg("--restore-last-session");
    }
    // ⛔ Своя метка в заголовке окна не годится: заголовок = имя страницы.
    // Окно ищем по номеру процесса, см. okna.rs.
    // Адрес подаём ТОЛЬКО когда сказано «открывать пустой» (или профиль
    // ещё свежий): иначе Chromium открыл бы его вместо восстановления
    // вкладок прошлой сессии.
    if let Some(a) = adres {
        cmd.arg(a);
    }

    if let Some(p) = proxy {
        if !p.trim().is_empty() {
            cmd.arg(format!("--proxy-server={}", p));
        }
    }
    // Под root Chromium молча не стартует без этого — ловили на опытах.
    if cfg!(target_os = "linux") && unsafe { libc_geteuid() } == 0 {
        cmd.arg("--no-sandbox");
    }

    // ⛔ Движок ДОЛЖЕН умирать вместе с нами. Своего обработчика выхода мало:
    // при обрыве процесса он не срабатывает, и в памяти остаются висеть
    // тринадцать процессов браузера на каждую вкладку (ловили 06.11.2026).
    // Поэтому просим СИСТЕМУ убить ребёнка, когда умрёт родитель.
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
    privyazat_k_zadanie(&ditya)?;

    Ok(ditya)
}

/// Windows: тот же приём, что у нас для xray в NexuSSH — задание с флагом
/// «убить всё при закрытии». Система сама погасит браузер, даже если наша
/// программа упала.
#[cfg(target_os = "windows")]
fn privyazat_k_zadanie(ditya: &Child) -> anyhow::Result<()> {
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::JobObjects::*;
    unsafe {
        let zadanie = CreateJobObjectW(None, None)?;
        let mut predel = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        predel.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        SetInformationJobObject(
            zadanie,
            JobObjectExtendedLimitInformation,
            &predel as *const _ as *const core::ffi::c_void,
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        )?;
        AssignProcessToJobObject(zadanie, HANDLE(ditya.as_raw_handle() as _))?;
        // ⛔ Ручку задания НЕ закрываем намеренно: закроется — снимется и
        // ограничение, и браузер переживёт нашу программу. Она живёт до
        // конца работы, система закроет её сама.
    }
    Ok(())
}

#[cfg(target_os = "linux")]
unsafe fn libc_geteuid() -> u32 {
    extern "C" {
        fn geteuid() -> u32;
    }
    geteuid()
}

#[cfg(not(target_os = "linux"))]
unsafe fn libc_geteuid() -> u32 {
    1000
}


/// Запустить вспомогательную программу (xray) так же надёжно, как движок.
pub fn zapustit_sputnik(programma: &Path, dovody: &[String]) -> anyhow::Result<Child> {
    let (otvet, zhdem) = sync_channel(1);
    zapuskatel().send(Zakaz::Prosto {
        programma: programma.to_path_buf(),
        dovody: dovody.to_vec(),
        otvet,
    })?;
    zhdem.recv()?
}

fn zapustit_prosto(programma: &Path, dovody: &[String]) -> anyhow::Result<Child> {
    let mut cmd = Command::new(programma);
    cmd.args(dovody).stdout(Stdio::null()).stderr(Stdio::null());
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
    privyazat_k_zadanie(&ditya)?;
    Ok(ditya)
}

/// Убить движки, оставшиеся от прошлого запуска программы.
///
/// ⛔ Зачем: если движок остался жить (мы упали, не дождались окна,
/// антивирус помешал), повторный запуск с тем же профилем НЕ поднимает
/// новый процесс — Chromium пересылает команду старому, а тот открывает
/// ОБЫЧНОЕ окно поверх всего. Человек видит «новое окно хрома», а наша
/// оболочка ждёт окна, которое не появилось. Имя у нашего exe уникальное —
/// чужих процессов так не назовут, резать по имени безопасно.
#[cfg(target_os = "windows")]
pub fn pribit_proshlyh_dvizhkov() {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };
    use windows::Win32::System::Threading::{OpenProcess, TerminateProcess, PROCESS_TERMINATE};

    unsafe {
        let Ok(snimok) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else {
            return;
        };
        let mut e = PROCESSENTRY32W::default();
        e.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut zhertvy = Vec::new();
        let mut ubito = 0;
        if Process32FirstW(snimok, &mut e).is_ok() {
            loop {
                let imya = String::from_utf16_lossy(&e.szExeFile);
                if imya.trim_end_matches('\0') == "nexusbro-engine.exe" {
                    zhertvy.push(e.th32ProcessID);
                }
                if Process32NextW(snimok, &mut e).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(snimok);
        for pid in zhertvy {
            if pid == std::process::id() {
                continue;
            }
            if let Ok(h) = OpenProcess(PROCESS_TERMINATE, false, pid) {
                let _ = TerminateProcess(h, 1);
                let _ = CloseHandle(h);
                ubito += 1;
            }
        }
        crate::zapisat_v_zhurnal(format!("при старте убито сирот-движков: {ubito}"));
    }
}

#[cfg(not(target_os = "windows"))]
pub fn pribit_proshlyh_dvizhkov() {}

/// Где лежит xray. Как и движок: сначала свой, потом системный.
pub fn najti_xray() -> Option<PathBuf> {    let imya = if cfg!(target_os = "windows") { "xray.exe" } else { "xray" };
    if let Some(res) = PAPKA_RESURSOV.get() {
        for p in ["", "binaries", "xray"] {
            let put = if p.is_empty() { res.join(imya) } else { res.join(p).join(imya) };
            if put.exists() {
                return Some(put);
            }
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(r) = exe.parent() {
            for p in [imya, &format!("binaries/{}", imya)] {
                let put = r.join(p);
                if put.exists() {
                    return Some(put);
                }
            }
        }
    }
    let sistemnyj = Path::new("/usr/bin/xray");
    sistemnyj.exists().then(|| sistemnyj.to_path_buf())
}

#[cfg(test)]
mod proverki {
    use super::*;

}
/// Попросить движок выйти ЧИСТО, не убивая. ⛔ Восстановление вкладок
/// работает только после чистого выхода (exit_type=Normal), поэтому при
/// завершении программы просим, а не глушим заданием: WM_CLOSE (Windows,
/// короткий таймаут, вызывать с нашего UI-потока после открепления
/// ввода!) или SIGTERM (Linux). Не ждёт — система всё равно добьёт
/// дерево при закрытии, но чистый выход успевает сбросить сессию.
pub fn poprosit_o_chem_vyhode(pid: u32, okno: Option<u32>) {
    #[cfg(target_os = "windows")]
    if let Some(o) = okno {
        crate::okna::sistema::otsoedinit_vvod(o);
        unsafe {
            let mut otvet: usize = 0;
            let _ = windows::Win32::UI::WindowsAndMessaging::SendMessageTimeoutW(
                windows::Win32::Foundation::HWND(o as usize as *mut core::ffi::c_void),
                windows::Win32::UI::WindowsAndMessaging::WM_CLOSE,
                windows::Win32::Foundation::WPARAM(0),
                windows::Win32::Foundation::LPARAM(0),
                windows::Win32::UI::WindowsAndMessaging::SMTO_ABORTIFHUNG,
                2000,
                Some(std::ptr::from_mut(&mut otvet)),
            );
        }
    }
    #[cfg(target_os = "linux")]
    unsafe {
        libc::kill(pid as i32, libc::SIGTERM);
    }
    #[cfg(not(any(target_os = "windows", target_os = "linux")))]
    let _ = (pid, okno);
}
