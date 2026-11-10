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
        adres: String,
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
                        Zakaz::Dvizhok { id, proxy, adres, otvet } => {
                            let _ = otvet.send(zapustit_zdes(&id, proxy.as_deref(), &adres));
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

/// Запустить движок для одной супер-вкладки.
pub fn zapustit(id: &str, proxy: Option<&str>, adres: &str) -> anyhow::Result<Child> {
    let (otvet, zhdem) = sync_channel(1);
    zapuskatel().send(Zakaz::Dvizhok {
        id: id.to_string(),
        proxy: proxy.map(|s| s.to_string()),
        adres: adres.to_string(),
        otvet,
    })?;
    // ⛔ Ожидание без края превращало неудачный запуск в вечное «Запускаю…».
    // Если поток запуска не ответил — говорим об этом прямо.
    zhdem
        .recv_timeout(std::time::Duration::from_secs(25))
        .map_err(|_| anyhow::anyhow!("поток запуска движков молчит 25 с — вероятно, антивирус держит exe"))?
}

fn zapustit_zdes(id: &str, proxy: Option<&str>, adres: &str) -> anyhow::Result<Child> {
    let dvizhok = najti_dvizhok()
        .ok_or_else(|| anyhow::anyhow!("движок не найден: положи chromium рядом с программой"))?;
    let profil = papka_profilej().join(id);
    std::fs::create_dir_all(&profil)?;
    let zhurnal = profil.join("engine.log");
    let _ = std::fs::rename(&zhurnal, profil.join("engine.log.1"));
    let stdout = std::fs::File::create(&zhurnal)?;
    let stderr = stdout.try_clone()?;

    let mut cmd = Command::new(dvizhok);
    cmd.arg(format!("--user-data-dir={}", profil.display()))
        .arg("--no-first-run")
        .arg("--no-default-browser-check")
        .arg("--disable-features=Translate,MediaRouter")
        .arg("--enable-logging=stderr")
        // ⛔ Своя метка в заголовке окна не годится: заголовок = имя страницы.
        // Окно ищем по номеру процесса, см. okna.rs.
        .arg(adres)
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr));

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
        CreateToolhelp32Snapshot, Process32First, Process32Next, PROCESSENTRY32,
        TH32CS_SNAPPROCESS,
    };
    use windows::Win32::System::Threading::{OpenProcess, TerminateProcess, PROCESS_TERMINATE};

    unsafe {
        let Ok(snimok) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else {
            return;
        };
        let mut e = PROCESSENTRY32::default();
        e.dwSize = std::mem::size_of::<PROCESSENTRY32>() as u32;
        let mut zhertvy = Vec::new();
        if Process32First(snimok, &mut e).is_ok() {
            loop {
                let imya = String::from_utf16_lossy(&e.szExeFile);
                if imya.trim_end_matches('\0') == "nexusbro-engine.exe" {
                    zhertvy.push(e.th32ProcessID);
                }
                if Process32Next(snimok, &mut e).is_err() {
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
            }
        }
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
