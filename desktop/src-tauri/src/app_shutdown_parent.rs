//! One parent-wide normal shutdown coordinator. Timers/EOF never prove exit.
//! Root integration must prevent close/exit and ALL owner-mutating commands
//! while draining, and intercept restart BEFORE Tauri's unpreventable restart.
use crate::app_shutdown_protocol::{Ack, Request, ACK_TIP};
use crate::super_process_owner::{ExitProbe, ProcessOwner};
use serde::Serialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    io::{Seek, SeekFrom, Write},
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex, OnceLock,
    },
    time::{Duration, Instant},
};
use tauri::{Emitter, Manager};

const GRACE: Duration = Duration::from_secs(10);
const RETRY: Duration = Duration::from_millis(150);
const POLL: Duration = Duration::from_millis(50);
const MAX_SENDS: u8 = 3;
const MAX_OWNERS: usize = 1024;
const RECEIPT: &str = "app-shutdown-receipt.json";
static DRAINING: AtomicBool = AtomicBool::new(false);
static MAY_EXIT: AtomicBool = AtomicBool::new(false);
// Only an integration violation can orphan one of our temporarily held owners.
// Retain it rather than releasing its lease or claiming a clean exit.
static RETAINED: Mutex<Vec<ProcessOwner>> = Mutex::new(Vec::new());
type Key = (String, u64);
#[derive(Clone)]
struct Entry {
    request: Option<Request>,
    acked: bool,
    timely_ack: bool,
    sends: u8,
    last_send: Option<Instant>,
    forced: bool,
    done: bool,
    clean: bool,
}
impl Entry {
    fn new(engine: u64) -> Self {
        Self {
            request: Request::fresh(engine).ok(),
            acked: false,
            timely_ack: false,
            sends: 0,
            last_send: None,
            forced: false,
            done: false,
            clean: false,
        }
    }
    fn send_due(&self, now: Instant, expired: bool) -> bool {
        !expired
            && !self.done
            && !self.acked
            && self.request.is_some()
            && self.sends < MAX_SENDS
            && self
                .last_send
                .is_none_or(|last| now.saturating_duration_since(last) >= RETRY)
    }
    fn action(&self, exited: bool, expired: bool) -> Action {
        if self.done {
            Action::Wait
        } else if exited && (self.acked || expired) {
            Action::Complete
        } else if expired && !self.forced {
            Action::Force
        } else {
            Action::Wait
        }
    }
    fn completed(&mut self) {
        self.done = true;
        self.clean = self.timely_ack && !self.forced;
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Action {
    Wait,
    Complete,
    Force,
}
#[derive(Default)]
struct Coordinator {
    exit_code: i32,
    exit_requested: bool,
    prepared_update: bool,
    preparation: Option<bool>,
    started: Option<Instant>,
    entries: BTreeMap<Key, Entry>,
    // Absence after an in-flight start/stop is not a clean AppShutdown ACK.
    unobserved_starts: BTreeSet<Key>,
    blocked: bool,
    exit_scheduled: bool,
}
impl Coordinator {
    fn request(&mut self, exit_code: Option<i32>) {
        if let Some(code) = exit_code {
            self.exit_code = merge_code(self.exit_code, code);
            self.exit_requested = true;
        } else if !self.exit_requested {
            self.prepared_update = true;
        }
    }
    fn clean_for_preparation(&self) -> bool {
        self.unobserved_starts.is_empty()
            && self.entries.values().all(|entry| entry.done && entry.clean)
    }
    fn can_schedule_exit(&self) -> bool {
        self.exit_requested && self.preparation.is_some() && !self.blocked && !self.exit_scheduled
    }
}
fn coordinator() -> &'static Mutex<Coordinator> {
    static STATE: OnceLock<Mutex<Coordinator>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(Coordinator::default()))
}
fn merge_code(old: i32, new: i32) -> i32 {
    if old == tauri::RESTART_EXIT_CODE || new == tauri::RESTART_EXIT_CODE {
        tauri::RESTART_EXIT_CODE
    } else if old != 0 {
        old
    } else {
        new
    }
}
pub(crate) fn may_exit() -> bool {
    MAY_EXIT.load(Ordering::SeqCst)
}
pub(crate) fn is_draining() -> bool {
    DRAINING.load(Ordering::SeqCst)
}

fn ordinary_metadata(metadata: &std::fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return false;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return false;
        }
    }
    true
}

fn test_root() -> Option<PathBuf> {
    if std::env::var("GITHUB_ACTIONS").as_deref() != Ok("true") {
        return None;
    }
    let root = PathBuf::from(std::env::var_os("NEXUSBRO_TEST_DATA")?);
    let metadata = std::fs::symlink_metadata(&root).ok()?;
    (root.is_absolute() && metadata.is_dir() && ordinary_metadata(&metadata)).then_some(root)
}
/// Call once during setup, before any test receipt is consumed for this process.
/// A test must collect the old parent's receipt BEFORE launching a restart.
pub(crate) fn init(_app: &tauri::AppHandle) {
    static INITIALIZED: AtomicBool = AtomicBool::new(false);
    if INITIALIZED.swap(true, Ordering::SeqCst) {
        return;
    }
    let Some(root) = test_root() else {
        return;
    };
    let path = root.join(RECEIPT);
    if let Ok(metadata) = std::fs::symlink_metadata(&path) {
        if metadata.is_file() && ordinary_metadata(&metadata) {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// Nonblocking to UI. The worker snapshots recovery BEFORE sending any close.
pub(crate) fn begin(app: &tauri::AppHandle, exit_code: i32) {
    request_drain(app, Some(exit_code));
}
/// Windows updater install can call process::exit directly. Drain BEFORE install,
/// without scheduling parent exit; never reopen writers after preparation.
pub(crate) fn prepare_update(app: &tauri::AppHandle) {
    request_drain(app, None);
}
/// None means not yet proven, including retained/unknown ownership. False means
/// actual completed ownership but forced/unacknowledged: installer MUST refuse.
pub(crate) fn preparation_result() -> Option<bool> {
    coordinator().lock().ok().and_then(|state| {
        // A concurrent ordinary close/restart wins over beginning an install.
        if state.exit_requested {
            None
        } else {
            state.preparation
        }
    })
}
fn request_drain(app: &tauri::AppHandle, exit_code: Option<i32>) {
    crate::ZAVERSHENIE.store(true, Ordering::SeqCst);
    {
        let Ok(mut state) = coordinator().lock() else {
            return;
        };
        state.request(exit_code);
    }
    if DRAINING.swap(true, Ordering::SeqCst) {
        // A close/restart promotes an already completed prepare-only drain.
        schedule_exit(app);
        return;
    }
    let app = app.clone();
    if std::thread::Builder::new()
        .name("app-shutdown".into())
        .spawn(move || drain(app))
        .is_err()
    {
        eprintln!("app shutdown coordinator unavailable");
    }
}
fn schedule_exit(app: &tauri::AppHandle) {
    let schedule = coordinator()
        .lock()
        .map(|mut state| {
            if state.can_schedule_exit() {
                state.exit_scheduled = true;
                true
            } else {
                false
            }
        })
        .unwrap_or(false);
    if !schedule {
        return;
    }
    let handle = app.clone();
    if app
        .run_on_main_thread(move || {
            // Honor a repeated restart request until the final UI commit boundary.
            let code = coordinator()
                .lock()
                .map(|state| state.exit_code)
                .unwrap_or(1);
            MAY_EXIT.store(true, Ordering::SeqCst);
            if code == tauri::RESTART_EXIT_CODE {
                // exit(MAX) alone does NOT set Tauri's restart_on_exit flag.
                handle.request_restart();
            } else {
                handle.exit(code);
            }
        })
        .is_err()
    {
        blocked(app);
    }
}

fn parse_ack(data: &serde_json::Value) -> Option<Ack> {
    // Bound shape BEFORE cloning/deserializing. Unknown fields never enter UI.
    let outer = data.as_object()?;
    if outer.len() != 2 || outer.get("tip")?.as_str()? != ACK_TIP {
        return None;
    }
    let ack = outer.get("ack")?.as_object()?;
    if ack.len() != 4 {
        return None;
    }
    let req = ack.get("request")?.as_object()?;
    if req.len() != 2 {
        return None;
    }
    let nonce = req.get("nonce")?.as_str()?;
    if nonce.len() != 32 || !nonce.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let request = Request {
        nonce: nonce.into(),
        engine: req.get("engine")?.as_u64()?,
    };
    if !request.valid() {
        return None;
    }
    Some(Ack {
        request,
        browsers_closed: ack.get("browsers_closed")?.as_bool()?,
        helpers_reaped: ack.get("helpers_reaped")?.as_bool()?,
        cef_shutdown_completed: ack.get("cef_shutdown_completed")?.as_bool()?,
    })
}
/// Always consumes a recognized ACK marker, even malformed/stale/unsolicited.
/// Root must route this BEFORE forwarding native data to the webview.
pub(crate) fn receive(
    app: &tauri::AppHandle,
    super_id: &str,
    engine: u64,
    data: &serde_json::Value,
) -> bool {
    if data.get("tip").and_then(|v| v.as_str()) != Some(ACK_TIP) {
        return false;
    }
    if !is_draining() {
        return true;
    }
    let Some(ack) = parse_ack(data) else {
        return true;
    };
    if ack.request.engine != engine {
        return true;
    }
    let s = app.state::<crate::Sostoyanie>();
    let current = s
        .vkladki
        .lock()
        .ok()
        .is_some_and(|map| map.get(super_id).is_some_and(|v| v.zapusk_id == engine));
    if !current {
        return true;
    }
    let Ok(mut state) = coordinator().lock() else {
        return true;
    };
    let timely = state.started.is_some_and(|start| start.elapsed() <= GRACE);
    if let Some(entry) = state.entries.get_mut(&(super_id.into(), engine)) {
        if !entry.done && entry.request.as_ref().is_some_and(|req| ack.matches(req)) {
            entry.acked = true;
            entry.timely_ack |= timely;
        }
    }
    true
}

#[derive(Serialize)]
struct Receipt {
    schema: u8,
    prepared_update: bool,
    parent_pid: u32,
    owners: usize,
    clean: usize,
    forced: usize,
    unacknowledged: usize,
    unclean: usize,
    pending_starts: usize,
    complete: bool,
}
fn receipt(pending: usize, complete: bool) -> Option<Receipt> {
    let state = coordinator().lock().ok()?;
    Some(Receipt {
        schema: 1,
        prepared_update: state.prepared_update,
        parent_pid: std::process::id(),
        owners: state.entries.len() + state.unobserved_starts.len(),
        clean: state.entries.values().filter(|e| e.done && e.clean).count(),
        forced: state.entries.values().filter(|e| e.forced).count(),
        unacknowledged: state.entries.values().filter(|e| !e.acked).count()
            + state.unobserved_starts.len(),
        unclean: state
            .entries
            .values()
            .filter(|e| e.done && !e.clean)
            .count()
            + state.unobserved_starts.len(),
        pending_starts: pending,
        complete,
    })
}
struct ReceiptFile(Option<File>);
impl ReceiptFile {
    fn create() -> Self {
        let file = test_root().and_then(|root| {
            let mut options = std::fs::OpenOptions::new();
            options.write(true).read(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            options.open(root.join(RECEIPT)).ok()
        });
        Self(file)
    }
    fn write(&mut self, value: &Receipt) {
        let Some(file) = self.0.as_mut() else {
            return;
        };
        let Ok(bytes) = serde_json::to_vec(value) else {
            return;
        };
        if bytes.len() > 4096 {
            return;
        }
        // Retain our exact opened inode. Never reopen/follow a replaced path.
        let _ = (|| -> std::io::Result<()> {
            file.seek(SeekFrom::Start(0))?;
            file.set_len(0)?;
            file.write_all(&bytes)?;
            file.sync_all()
        })();
    }
}
fn blocked(app: &tauri::AppHandle) {
    let first = coordinator()
        .lock()
        .map(|mut state| {
            let first = !state.blocked;
            state.blocked = true;
            state.preparation = None;
            first
        })
        .unwrap_or(false);
    if !first {
        return;
    }
    eprintln!("app shutdown ownership completion unavailable");
    let _ = app.emit(
        "oshibka-dvizhka",
        "Завершение движков ещё не подтверждено. Владение процессами сохранено.",
    );
}

/// Register newly materialized starts too. Missing owner is not exit evidence.
fn discover(app: &tauri::AppHandle) -> usize {
    let s = app.state::<crate::Sostoyanie>();
    let observed = match s.vkladki.lock() {
        Ok(map) => map
            .iter()
            .map(|(id, v)| {
                (
                    id.clone(),
                    v.zapusk_id,
                    v.proc.is_some(),
                    v.zapusk_idet,
                    v.hibernate.machine.phase() == crate::super_lifecycle::Phase::Sleeping,
                    v.nash_proxy.is_some() || v.tunnel.is_some(),
                )
            })
            .collect::<Vec<_>>(),
        Err(_) => {
            blocked(app);
            return 1;
        }
    };
    let Ok(mut state) = coordinator().lock() else {
        return 1;
    };
    let mut pending = 0;
    for (id, engine, owned, starting, sleeping, residual_network) in observed {
        if owned {
            state.unobserved_starts.remove(&(id.clone(), engine));
            if state
                .entries
                .get(&(id.clone(), engine))
                .is_some_and(|entry| entry.done)
            {
                state.blocked = true;
                pending += 1;
                continue;
            }
            if !state.entries.contains_key(&(id.clone(), engine)) {
                if state.entries.len() + state.unobserved_starts.len() >= MAX_OWNERS {
                    state.blocked = true;
                    pending += 1;
                    continue;
                }
                state.entries.insert((id, engine), Entry::new(engine));
            }
        } else if starting || !sleeping || residual_network {
            if !state.entries.contains_key(&(id.clone(), engine))
                && !state.unobserved_starts.contains(&(id.clone(), engine))
            {
                if state.entries.len() + state.unobserved_starts.len() >= MAX_OWNERS {
                    state.blocked = true;
                } else {
                    state.unobserved_starts.insert((id, engine));
                }
            }
            pending += 1;
        }
    }
    pending
}
fn send_all(app: &tauri::AppHandle, now: Instant, expired: bool) {
    let plans = {
        let Ok(mut state) = coordinator().lock() else {
            return;
        };
        state
            .entries
            .iter_mut()
            .filter_map(|(key, entry)| {
                if !entry.send_due(now, expired) {
                    return None;
                }
                entry.sends += 1;
                entry.last_send = Some(now);
                Some((key.clone(), entry.request.clone()?))
            })
            .collect::<Vec<_>>()
    };
    let s = app.state::<crate::Sostoyanie>();
    // Every owner gets its first attempt before polling/waiting on ANY owner.
    for ((id, engine), request) in plans {
        let channel = s.vkladki.lock().ok().and_then(|map| {
            map.get(&id)
                .filter(|v| v.zapusk_id == engine)
                .and_then(|v| v.kanal.clone())
        });
        if let Some(channel) = channel {
            let _ = channel.try_send(crate::sessiya::Komanda::AppShutdown { request });
        }
    }
}
fn restore_owner(app: &tauri::AppHandle, key: &Key, owner: ProcessOwner) {
    let s = app.state::<crate::Sostoyanie>();
    let mut owner = Some(owner);
    if let Ok(mut map) = s.vkladki.lock() {
        if let Some(v) = map
            .get_mut(&key.0)
            .filter(|v| v.zapusk_id == key.1 && v.proc.is_none())
        {
            v.proc = owner.take();
        }
    }
    if let Some(owner) = owner {
        // Never drop a still-owned Job/lease on an impossible-generation race.
        if let Ok(mut retained) = RETAINED.lock() {
            retained.push(owner);
        } else {
            std::mem::forget(owner);
        }
        blocked(app);
    }
}
fn released(app: &tauri::AppHandle, key: &Key) {
    let s = app.state::<crate::Sostoyanie>();
    let resources = s.vkladki.lock().ok().and_then(|mut map| {
        map.get_mut(&key.0)
            .filter(|v| v.zapusk_id == key.1 && v.proc.is_none())
            .map(|v| {
                v.kanal = None;
                v.okno = None;
                v.zapas_pid = None;
                v.zapusk_idet = false;
                v.hibernate = Default::default();
                (v.nash_proxy.take(), v.tunnel.take())
            })
    });
    let Some((proxy, tunnel)) = resources else {
        blocked(app);
        return;
    };
    // Only after fresh complete() / terminate_explicit() proved tree exit.
    if let Some(proxy) = proxy {
        proxy.ostanovit();
    }
    if let Some(mut tunnel) = tunnel {
        let _ = tunnel.kill();
        if tunnel.wait().is_err() {
            if let Ok(mut map) = s.vkladki.lock() {
                if let Some(v) = map
                    .get_mut(&key.0)
                    .filter(|v| v.zapusk_id == key.1 && v.tunnel.is_none())
                {
                    v.tunnel = Some(tunnel);
                } else {
                    std::mem::forget(tunnel);
                }
            } else {
                std::mem::forget(tunnel);
            }
            blocked(app);
            return;
        }
    }
    crate::release_route_resources(app, &s, &key.0, key.1);
    if let Ok(mut state) = coordinator().lock() {
        if let Some(entry) = state.entries.get_mut(key) {
            entry.completed();
        }
    }
}
fn poll_owners(app: &tauri::AppHandle, expired: bool) {
    let entries = {
        let Ok(state) = coordinator().lock() else {
            return;
        };
        state
            .entries
            .iter()
            .filter(|(_, e)| !e.done)
            .map(|(key, e)| (key.clone(), e.clone()))
            .collect::<Vec<_>>()
    };
    let s = app.state::<crate::Sostoyanie>();
    for (key, entry) in entries {
        // Linux probe can reap an exited leader and release its numeric PID
        // reservation. Keep that reservation until ACK admission (or explicit
        // deadline fallback), instead of needlessly probing ahead of the ACK.
        if !entry.acked && !expired {
            continue;
        }
        let (action, owner) = {
            let Ok(mut map) = s.vkladki.lock() else {
                blocked(app);
                return;
            };
            let Some(v) = map.get_mut(&key.0).filter(|v| v.zapusk_id == key.1) else {
                drop(map);
                blocked(app);
                continue;
            };
            let Some(owner) = v.proc.as_mut() else {
                drop(map);
                blocked(app);
                continue;
            };
            let exited = owner.probe() == ExitProbe::Exited;
            let action = entry.action(exited, expired);
            (
                action,
                if action != Action::Wait {
                    v.proc.take()
                } else {
                    None
                },
            )
        };
        let Some(owner) = owner else {
            continue;
        };
        if action == Action::Force {
            if let Ok(mut state) = coordinator().lock() {
                if let Some(e) = state.entries.get_mut(&key) {
                    e.forced = true;
                }
            }
            match owner.terminate_explicit() {
                Ok(()) => released(app, &key),
                Err(owner) => restore_owner(app, &key, owner),
            }
        } else {
            match owner.complete() {
                Ok(()) => released(app, &key),
                Err(owner) => restore_owner(app, &key, owner),
            }
        }
    }
}
fn ready_to_exit(pending: usize) -> bool {
    coordinator()
        .lock()
        .is_ok_and(|state| pending == 0 && !state.blocked && state.entries.values().all(|e| e.done))
}
fn drain(app: tauri::AppHandle) {
    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        crate::okna_app::before_exit(&app)
    }))
    .is_err()
    {
        blocked(&app);
        if let Some(value) = receipt(0, false) {
            ReceiptFile::create().write(&value);
        }
        return;
    }
    let start = Instant::now();
    if let Ok(mut state) = coordinator().lock() {
        state.started = Some(start);
    }
    let mut output = ReceiptFile::create();
    let mut warned = false;
    loop {
        let pending = discover(&app);
        let now = Instant::now();
        let expired = now.saturating_duration_since(start) >= GRACE;
        send_all(&app, now, expired);
        poll_owners(&app, expired);
        // Fresh observation AFTER every owner completion catches pending starts.
        let pending = pending.max(discover(&app));
        if ready_to_exit(pending) {
            // Official Windows updater install exits the process directly. Its
            // prepare-only path must perform existing final cleanup NOW, after
            // every owner completed, rather than relying on RunEvent::Exit.
            let cleaned = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                crate::perevod::otmenit_vse();
                app.state::<crate::Sostoyanie>().routes.stop_all();
                crate::privatnost::ochistit();
                matches!(std::fs::symlink_metadata(crate::privatnost::papka("")),
                    Err(error) if error.kind()==std::io::ErrorKind::NotFound)
            }))
            .unwrap_or(false);
            if !cleaned {
                blocked(&app);
                if let Some(value) = receipt(0, false) {
                    output.write(&value);
                }
                return;
            }
            if let Some(value) = receipt(0, true) {
                output.write(&value);
            }
            // Publish preparation ONLY after the final receipt is synced. A
            // concurrent begin() must not schedule exit ahead of that write.
            if let Ok(mut state) = coordinator().lock() {
                state.preparation = Some(state.clean_for_preparation());
            }
            schedule_exit(&app);
            return;
        }
        if expired && !warned {
            warned = true;
            if let Some(value) = receipt(pending, false) {
                output.write(&value);
            }
            eprintln!("app shutdown fallback: clean completion not confirmed");
            let _ = app.emit(
                "oshibka-dvizhka",
                "Движки завершаются принудительно; чистое завершение не подтверждено.",
            );
        }
        std::thread::sleep(if expired {
            Duration::from_millis(100)
        } else {
            POLL
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn restart_survives_later_close_and_failure_requests() {
        assert_eq!(
            merge_code(0, tauri::RESTART_EXIT_CODE),
            tauri::RESTART_EXIT_CODE
        );
        assert_eq!(
            merge_code(tauri::RESTART_EXIT_CODE, 0),
            tauri::RESTART_EXIT_CODE
        );
        assert_eq!(
            merge_code(tauri::RESTART_EXIT_CODE, 1),
            tauri::RESTART_EXIT_CODE
        );
        assert_eq!(merge_code(7, 0), 7);
    }
    #[test]
    fn prepare_only_waits_for_install_and_close_promotes_it_without_demotion() {
        let mut state = Coordinator::default();
        state.request(None);
        state.preparation = Some(true);
        assert!(state.prepared_update);
        assert!(!state.can_schedule_exit());
        state.request(Some(tauri::RESTART_EXIT_CODE));
        assert!(state.can_schedule_exit());
        state.request(None);
        state.request(Some(0));
        assert!(state.exit_requested);
        assert_eq!(state.exit_code, tauri::RESTART_EXIT_CODE);
        state.exit_scheduled = true;
        assert!(!state.can_schedule_exit());
        let mut pending = Coordinator::default();
        pending.request(Some(0));
        pending.request(None);
        assert!(!pending.prepared_update);
        assert!(!pending.can_schedule_exit());
        pending.preparation = Some(false);
        assert!(pending.can_schedule_exit());
        pending.blocked = true;
        assert!(!pending.can_schedule_exit());
    }
    #[test]
    fn unobserved_start_never_becomes_clean_by_disappearing() {
        let mut state = Coordinator::default();
        state.unobserved_starts.insert(("fixture".into(), 9));
        assert!(!state.clean_for_preparation());
        // Materialization replaces the conservative placeholder; an actual
        // ACK and fresh owner completion are still both mandatory.
        state.unobserved_starts.remove(&("fixture".into(), 9));
        let mut entry = Entry::new(9);
        state.entries.insert(("fixture".into(), 9), entry.clone());
        assert!(!state.clean_for_preparation());
        entry.acked = true;
        entry.timely_ack = true;
        entry.completed();
        state.entries.insert(("fixture".into(), 9), entry);
        assert!(state.clean_for_preparation());
    }
    #[test]
    fn ack_without_tree_exit_and_eof_without_ack_cannot_be_clean() {
        let mut entry = Entry::new(9);
        assert_eq!(entry.action(true, false), Action::Wait);
        entry.acked = true;
        entry.timely_ack = true;
        assert_eq!(entry.action(false, false), Action::Wait);
        assert_eq!(entry.action(true, false), Action::Complete);
        entry.completed();
        assert!(entry.clean);
        let mut eof = Entry::new(10);
        assert_eq!(eof.action(true, true), Action::Complete);
        eof.completed();
        assert!(!eof.clean);
    }
    #[test]
    fn forced_or_late_ack_is_never_relabelled_clean_and_force_is_once() {
        let mut entry = Entry::new(9);
        entry.acked = true;
        assert_eq!(entry.action(false, true), Action::Force);
        entry.forced = true;
        assert_eq!(entry.action(false, true), Action::Wait);
        entry.timely_ack = true;
        entry.completed();
        assert!(!entry.clean);
        let mut late = Entry::new(10);
        late.acked = true;
        late.completed();
        assert!(!late.clean);
    }
    #[test]
    fn retries_are_bounded_and_deadline_stops_sending() {
        let mut entry = Entry::new(9);
        let now = Instant::now();
        for n in 0..MAX_SENDS {
            assert!(entry.send_due(now + RETRY * u32::from(n), false));
            entry.sends += 1;
            entry.last_send = Some(now + RETRY * u32::from(n));
        }
        assert!(!entry.send_due(now + Duration::from_secs(1), false));
        assert!(!Entry::new(10).send_due(now, true));
    }
    #[test]
    fn malformed_extra_and_cross_generation_ack_are_refused() {
        let expected = Request::fresh(9).unwrap();
        let ack = Ack::after_cef_shutdown(expected.clone());
        let mut wire = serde_json::json!({"tip":ACK_TIP,"ack":ack});
        assert!(parse_ack(&wire).unwrap().matches(&expected));
        wire["extra"] = true;
        assert!(parse_ack(&wire).is_none());
        wire.as_object_mut().unwrap().remove("extra");
        wire["ack"]["request"]["nonce"] = "x".repeat(65536).into();
        assert!(parse_ack(&wire).is_none());
        wire["ack"]["request"] = serde_json::to_value(Request::fresh(10).unwrap()).unwrap();
        assert!(!parse_ack(&wire).unwrap().matches(&expected));
    }
}
