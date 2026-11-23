//! On-demand read-only metrics for this app's verified process tree.
//! Never enumerate by image name or return process command lines or page URLs.
#[path = "browser_tasks_model.rs"]
mod model;
use model::{Process, Root};
use serde::Serialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    time::{Instant, SystemTime, UNIX_EPOCH},
};
const MAX_PROCESSES: usize = 32_768;
const MAX_OWNED: usize = 1024;
#[derive(Serialize)]
pub struct Snapshot {
    sampled_at_ms: u64,
    monotonic_ms: u64,
    logical_cpus: usize,
    #[serde(flatten)]
    metrics: model::Groups,
    complete: bool,
    unavailable_processes: usize,
    scope: &'static str,
    cpu_unit: &'static str,
    memory_note: &'static str,
}
#[tauri::command]
pub async fn browser_tasks_snapshot(
    state: tauri::State<'_, crate::Sostoyanie>,
) -> Result<Snapshot, String> {
    let roots: Vec<_> = {
        let tabs = state.vkladki.lock().map_err(|_| "Супер-вкладки заняты")?;
        tabs.values()
            .flat_map(|tab| {
                let mut roots = Vec::new();
                let mut seen = BTreeSet::new();
                for pid in [
                    tab.proc.as_ref().map(std::process::Child::id),
                    tab.zapas_pid,
                    tab.tunnel.as_ref().map(std::process::Child::id),
                ]
                .into_iter()
                .flatten()
                {
                    if seen.insert(pid) {
                        roots.push(Root {
                            pid,
                            super_id: tab.nastrojki.id.clone(),
                            name: tab.nastrojki.nazvanie.clone(),
                        });
                    }
                }
                roots
            })
            .collect()
    };
    tauri::async_runtime::spawn_blocking(move || snapshot(roots))
        .await
        .map_err(|_| "Не удалось прочитать состояние процессов".to_string())?
}
fn snapshot(roots: Vec<Root>) -> Result<Snapshot, String> {
    static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    let start = START.get_or_init(Instant::now);
    let app = std::process::id();
    let (table, unavailable, truncated) = platform::read(app)?;
    let metrics = model::aggregate(&table, app, &roots)?;
    let complete = !truncated
        && unavailable == 0
        && metrics.excluded_unverified_roots == 0
        && metrics.rejected_ancestry == 0;
    Ok(Snapshot {
        sampled_at_ms: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64,
        monotonic_ms: start.elapsed().as_millis() as u64,
        logical_cpus: std::thread::available_parallelism().map_or(1, usize::from),
        metrics,
        complete,
        unavailable_processes: unavailable,
        scope: "verified_descendants_of_this_instance",
        cpu_unit: "cumulative_process_ms; calculate deltas only for matching process identities",
        memory_note: if cfg!(target_os = "linux") {
            "Working set is RSS (shared pages can be counted in several processes); private is Private_Clean+Private_Dirty+Private_Hugetlb, not commit."
        } else {
            "Working set includes shared pages; private is process private commit. No per-tab attribution for shared processes."
        },
    })
}
fn candidate_pids(parents: &BTreeMap<u32, u32>, app: u32) -> (BTreeSet<u32>, bool) {
    let mut owned = BTreeSet::from([app]);
    for _ in 0..128 {
        let old = owned.len();
        for (&pid, parent) in parents {
            if owned.contains(parent) && owned.len() < MAX_OWNED {
                owned.insert(pid);
            }
        }
        if owned.len() == old {
            break;
        }
    }
    let truncated = parents
        .iter()
        .any(|(pid, parent)| !owned.contains(pid) && owned.contains(parent));
    (owned, truncated)
}
#[cfg(target_os = "linux")]
mod platform {
    use super::*;
    use std::io::Read;
    fn bounded(path: impl AsRef<std::path::Path>, max: usize) -> Option<String> {
        let mut buf = String::new();
        std::fs::File::open(path)
            .ok()?
            .take((max + 1) as u64)
            .read_to_string(&mut buf)
            .ok()?;
        (buf.len() <= max).then_some(buf)
    }
    fn stat(pid: u32, ticks: u64, page: u64) -> Option<Process> {
        let text = bounded(format!("/proc/{pid}/stat"), 8192)?;
        model::linux_stat(&text, pid, ticks, page)
    }
    fn private(pid: u32) -> Option<u64> {
        let text = bounded(format!("/proc/{pid}/smaps_rollup"), 16384)?;
        model::linux_private(&text)
    }
    pub fn read(app: u32) -> Result<(BTreeMap<u32, Process>, usize, bool), String> {
        let ticks = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
        let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
        if ticks <= 0 || page <= 0 {
            return Err("Не удалось получить единицы измерения процессов".into());
        }
        let mut table = BTreeMap::new();
        let mut truncated = false;
        let entries = std::fs::read_dir("/proc").map_err(|_| "Не удалось прочитать процессы")?;
        let mut count = 0;
        for entry in entries.flatten() {
            let Some(pid) = entry
                .file_name()
                .to_str()
                .and_then(|name| name.parse::<u32>().ok())
            else {
                continue;
            };
            count += 1;
            if count > MAX_PROCESSES {
                truncated = true;
                break;
            }
            if let Some(record) = stat(pid, ticks as u64, page as u64) {
                table.insert(pid, record);
            }
        }
        let parents = table.iter().map(|(pid, p)| (*pid, p.parent)).collect();
        let (owned, limit) = candidate_pids(&parents, app);
        truncated |= limit;
        table.retain(|pid, _| owned.contains(pid));
        let mut unavailable = 0;
        for process in table.values_mut() {
            process.private_bytes = private(process.pid);
            if process.private_bytes.is_none() {
                unavailable += 1;
            }
            // The PID could exit/recycle while smaps was read. Reject mixed identities.
            if stat(process.pid, ticks as u64, page as u64)
                .is_none_or(|p| p.created != process.created)
            {
                process.created = 0;
                unavailable += 1;
            }
        }
        table.retain(|_, p| p.created != 0);
        Ok((table, unavailable, truncated))
    }
}
#[cfg(target_os = "windows")]
mod platform {
    use super::*;
    use windows::Win32::{
        Foundation::{CloseHandle, FILETIME, HANDLE},
        System::{
            Diagnostics::ToolHelp::{
                CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
                TH32CS_SNAPPROCESS,
            },
            ProcessStatus::{
                K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX,
            },
            Threading::{GetProcessTimes, OpenProcess, PROCESS_QUERY_INFORMATION, PROCESS_VM_READ},
        },
    };
    struct OwnedHandle(HANDLE);
    impl Drop for OwnedHandle {
        fn drop(&mut self) {
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }
    fn ticks(value: FILETIME) -> u64 {
        (value.dwHighDateTime as u64) << 32 | value.dwLowDateTime as u64
    }
    fn query(pid: u32, parent: u32) -> Option<Process> {
        let handle = OwnedHandle(
            unsafe { OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, false, pid) }.ok()?,
        );
        let (mut created, mut exit, mut kernel, mut user) = (
            FILETIME::default(),
            FILETIME::default(),
            FILETIME::default(),
            FILETIME::default(),
        );
        unsafe { GetProcessTimes(handle.0, &mut created, &mut exit, &mut kernel, &mut user) }
            .ok()?;
        if ticks(exit) != 0 {
            return None;
        }
        let mut memory = PROCESS_MEMORY_COUNTERS_EX::default();
        memory.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
        let memory_ok = unsafe {
            K32GetProcessMemoryInfo(
                handle.0,
                &mut memory as *mut PROCESS_MEMORY_COUNTERS_EX as *mut PROCESS_MEMORY_COUNTERS,
                memory.cb,
            )
        }
        .as_bool();
        Some(Process {
            pid,
            parent,
            created: ticks(created),
            cpu_time_ms: ticks(kernel).saturating_add(ticks(user)) / 10_000,
            working_set_bytes: memory_ok.then_some(memory.WorkingSetSize as u64),
            private_bytes: memory_ok.then_some(memory.PrivateUsage as u64),
        })
    }
    pub fn read(app: u32) -> Result<(BTreeMap<u32, Process>, usize, bool), String> {
        let handle = OwnedHandle(
            unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }
                .map_err(|_| "Не удалось получить снимок процессов")?,
        );
        let mut entry = PROCESSENTRY32W::default();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut parents = BTreeMap::new();
        let mut truncated = false;
        let mut available = unsafe { Process32FirstW(handle.0, &mut entry) }.is_ok();
        while available {
            if parents.len() >= MAX_PROCESSES {
                truncated = true;
                break;
            }
            parents.insert(entry.th32ProcessID, entry.th32ParentProcessID);
            available = unsafe { Process32NextW(handle.0, &mut entry) }.is_ok();
        }
        let (owned, limit) = candidate_pids(&parents, app);
        truncated |= limit;
        let mut table = BTreeMap::new();
        let mut unavailable = 0;
        for pid in owned {
            match query(pid, parents.get(&pid).copied().unwrap_or(0)) {
                Some(process) => {
                    if process.private_bytes.is_none() {
                        unavailable += 1;
                    }
                    table.insert(pid, process);
                }
                None => unavailable += 1,
            }
        }
        Ok((table, unavailable, truncated))
    }
}
#[cfg(not(any(target_os = "linux", target_os = "windows")))]
mod platform {
    use super::*;
    pub fn read(_: u32) -> Result<(BTreeMap<u32, Process>, usize, bool), String> {
        Err("Диспетчер задач пока поддерживает Windows и Linux".into())
    }
}
