//! Native-only page save: Ctrl+S -> parent-owned save dialog -> CDP
//! `Page.captureSnapshot` (format mhtml) -> bounded atomic file write.
//! The renderer never supplies a path, HTML, or a URL. Only the single
//! active owned browser is admitted; generation/document/browser binding is
//! re-verified after the dialog returns and before any byte is published.
//! Nothing here logs paths, URLs, HTML, titles, or secrets: the UI learns
//! only compact typed state events.
use crate::cdp;
use crate::cef_vkladki::Sobytie;
use cef::*;
use rand_core::{OsRng, RngCore};
use serde_json::{json, Value};
use std::{
    cell::RefCell,
    collections::HashMap,
    io::Write,
    path::{Path, PathBuf},
    sync::{mpsc::Sender, Arc, Mutex, OnceLock},
    time::{Duration, Instant},
};
use tokio::sync::watch;
use zeroize::Zeroizing;

const MAX_JOBS: usize = 2;
const MAX_DOCUMENT_BYTES: usize = 8192;
const MAX_PATH_BYTES: usize = 4096;
const MAX_MHTML_BYTES: usize = 128 * 1024 * 1024;
const DIALOG_TIMEOUT: Duration = Duration::from_secs(120);
const CAPTURE_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum SaveError {
    Cancelled,
    Busy,
    Locked,
    PageChanged,
    WrongThread,
    CdpMethodFailed,
    CdpAgentDetached,
    CdpResultAbsent,
    InvalidResult,
    TooLarge,
    Storage,
    CaptureTimeout,
    StateUnavailable,
    RandomUnavailable,
}
impl SaveError {
    fn code(self) -> &'static str {
        match self {
            SaveError::Cancelled => "cancelled",
            SaveError::Busy => "busy",
            SaveError::Locked => "locked",
            SaveError::PageChanged => "page_changed",
            SaveError::WrongThread => "wrong_thread",
            SaveError::CdpMethodFailed => "cdp_method_failed",
            SaveError::CdpAgentDetached => "cdp_agent_detached",
            SaveError::CdpResultAbsent => "cdp_result_absent",
            SaveError::InvalidResult => "invalid_result",
            SaveError::TooLarge => "too_large",
            SaveError::Storage => "storage",
            SaveError::CaptureTimeout => "capture_timeout",
            SaveError::StateUnavailable => "state_unavailable",
            SaveError::RandomUnavailable => "random_unavailable",
        }
    }
}

struct Completion {
    done: bool,
    reply: Option<Result<String, SaveError>>,
}

struct Entry {
    request: String,
    tab: u64,
    route_generation: u64,
    document: String,
    frame_id: String,
    created: Instant,
    started: Option<Instant>,
    cancel: watch::Sender<bool>,
    completion: Option<Arc<Mutex<Completion>>>,
    path: Option<PathBuf>,
    result: Option<Result<u64, SaveError>>,
}

fn jobs() -> &'static Mutex<HashMap<i32, Entry>> {
    static JOBS: OnceLock<Mutex<HashMap<i32, Entry>>> = OnceLock::new();
    JOBS.get_or_init(Default::default)
}

thread_local! {
    /// CDP observer registrations live on the creating CEF UI thread only.
    static REGISTRATIONS: RefCell<HashMap<i32, Registration>> = RefCell::new(HashMap::new());
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Days-from-civil inverse (Howard Hinnant): UTC civil date from epoch days.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Privacy-safe default name: no title, no URL, only the UTC timestamp.
fn make_filename(unix_secs: u64) -> String {
    let (y, mo, d) = civil_from_days((unix_secs / 86400) as i64);
    let secs = unix_secs % 86400;
    format!(
        "page-{:04}{:02}{:02}-{:02}{:02}{:02}.mhtml",
        y,
        mo,
        d,
        secs / 3600,
        (secs % 3600) / 60,
        secs % 60
    )
}

fn valid_request(request: &str) -> bool {
    request.len() == 32 && request.bytes().all(|c| c.is_ascii_hexdigit())
}

fn valid_filename(name: &str) -> bool {
    name.len() <= 64
        && name.starts_with("page-")
        && name.ends_with(".mhtml")
        && name
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-'))
}

fn document_matches(browser: &Browser, identity: i32, frame_id: &str, document: &str) -> bool {
    if browser.identifier() != identity || browser.is_valid() == 0 {
        return false;
    }
    let Some(frame) = browser.main_frame() else {
        return false;
    };
    frame.is_main() != 0
        && CefString::from(&frame.identifier()).to_string() == frame_id
        && CefString::from(&frame.url()).to_string() == document
}

/// The CDP result must be a real MHTML archive, never a bare HTML dump.
fn extract_mhtml_with_cap(bytes: &[u8], cap: usize) -> Result<String, SaveError> {
    if bytes.is_empty() {
        return Err(SaveError::CdpResultAbsent);
    }
    if bytes.len() > cap {
        return Err(SaveError::TooLarge);
    }
    let raw = Zeroizing::new(bytes.to_vec());
    let tree: Value = serde_json::from_slice(&raw).map_err(|_| SaveError::InvalidResult)?;
    let data = tree
        .get("data")
        .and_then(Value::as_str)
        .ok_or(SaveError::InvalidResult)?;
    if data.is_empty() || data.len() > cap {
        return Err(if data.is_empty() {
            SaveError::InvalidResult
        } else {
            SaveError::TooLarge
        });
    }
    let head = &data[..data.len().min(4096)];
    if !head.contains("MIME-Version") {
        return Err(SaveError::InvalidResult);
    }
    Ok(data.to_owned())
}

fn extract_mhtml(bytes: &[u8]) -> Result<String, SaveError> {
    extract_mhtml_with_cap(bytes, MAX_MHTML_BYTES)
}

/// Atomic publish: write a sibling temp file, fsync, rename over the target.
/// A cancel observed before the rename removes the temp and reports Cancelled.
fn atomic_write(
    path: &Path,
    bytes: &[u8],
    cancel: &watch::Receiver<bool>,
) -> Result<u64, SaveError> {
    if !path.is_absolute() {
        return Err(SaveError::Storage);
    }
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .filter(|n| valid_filename(n))
        .ok_or(SaveError::Storage)?;
    let parent = path
        .parent()
        .filter(|p| p.is_dir())
        .ok_or(SaveError::Storage)?;
    let mut nonce = [0u8; 8];
    OsRng
        .try_fill_bytes(&mut nonce)
        .map_err(|_| SaveError::RandomUnavailable)?;
    let temp = parent.join(format!(
        "{}.part-{}-{}",
        name,
        std::process::id(),
        nonce.iter().map(|b| format!("{b:02x}")).collect::<String>()
    ));
    let written = (|| -> Result<u64, SaveError> {
        let mut file = std::fs::File::create(&temp).map_err(|_| SaveError::Storage)?;
        file.write_all(bytes).map_err(|_| SaveError::Storage)?;
        file.sync_all().map_err(|_| SaveError::Storage)?;
        Ok(bytes.len() as u64)
    })();
    let bytes_written = match written {
        Ok(n) => n,
        Err(error) => {
            let _ = std::fs::remove_file(&temp);
            return Err(error);
        }
    };
    if *cancel.borrow() {
        let _ = std::fs::remove_file(&temp);
        return Err(SaveError::Cancelled);
    }
    if std::fs::rename(&temp, path).is_err() {
        // Windows rename does not replace an existing file the user confirmed
        // overwriting in the native dialog: remove then rename, honestly
        // non-atomic only for that replacement instant.
        if std::fs::remove_file(path).is_err() || std::fs::rename(&temp, path).is_err() {
            let _ = std::fs::remove_file(&temp);
            return Err(SaveError::Storage);
        }
    }
    Ok(bytes_written)
}

wrap_dev_tools_message_observer! {
    struct Observer {
        identity: i32,
        frame_id: String,
        document: String,
        message: i32,
        state: Arc<Mutex<Completion>>,
    }
    impl DevToolsMessageObserver {
        fn on_dev_tools_method_result(&self, browser: Option<&mut Browser>, message_id: i32, success: i32, result: Option<&[u8]>) {
            if message_id != self.message {
                return;
            }
            let matches = browser.is_some_and(|b| document_matches(b, self.identity, &self.frame_id, &self.document));
            let Ok(mut state) = self.state.lock() else { return };
            if state.done {
                return;
            }
            state.done = true;
            state.reply = Some(if success == 0 {
                Err(SaveError::CdpMethodFailed)
            } else if !matches {
                Err(SaveError::PageChanged)
            } else {
                result.ok_or(SaveError::CdpResultAbsent).and_then(extract_mhtml)
            });
        }
        fn on_dev_tools_agent_detached(&self, browser: Option<&mut Browser>) {
            if browser.is_some_and(|b| b.identifier() != self.identity) {
                return;
            }
            if let Ok(mut state) = self.state.lock() {
                if !state.done {
                    state.done = true;
                    state.reply = Some(Err(SaveError::CdpAgentDetached));
                }
            }
        }
    }
}

fn send_state(sender: &Sender<Sobytie>, tab: u64, state: Value) {
    let _ = sender.send(Sobytie::Sluzhebnoe(
        json!({"tip":"page-save-state","id":tab,"state":state}),
    ));
}

/// From dispatch of Komanda::SohranitStranitsu. Admits one pending save per
/// browser and asks the parent for a native save path.
pub fn request(browser: &Browser, tab: u64, route_generation: u64, sender: &Sender<Sobytie>) {
    let identity = browser.identifier();
    if crate::browser_lock::locked_global() {
        send_state(sender, tab, json!({"error":SaveError::Locked.code()}));
        return;
    }
    if currently_on(ThreadId::UI) == 0 || browser.is_valid() == 0 {
        send_state(
            sender,
            tab,
            json!({"error":SaveError::StateUnavailable.code()}),
        );
        return;
    }
    let Some(frame) = browser.main_frame() else {
        send_state(
            sender,
            tab,
            json!({"error":SaveError::StateUnavailable.code()}),
        );
        return;
    };
    let document = CefString::from(&frame.url()).to_string();
    let frame_id = CefString::from(&frame.identifier()).to_string();
    if document.is_empty()
        || document.len() > MAX_DOCUMENT_BYTES
        || frame_id.is_empty()
        || frame_id.len() > 256
    {
        send_state(
            sender,
            tab,
            json!({"error":SaveError::StateUnavailable.code()}),
        );
        return;
    }
    let mut nonce = [0u8; 16];
    if OsRng.try_fill_bytes(&mut nonce).is_err() {
        send_state(
            sender,
            tab,
            json!({"error":SaveError::RandomUnavailable.code()}),
        );
        return;
    }
    let request = nonce.iter().map(|b| format!("{b:02x}")).collect::<String>();
    let filename = make_filename(unix_now());
    let (cancel, _) = watch::channel(false);
    let admitted = jobs().lock().is_ok_and(|mut all| {
        if all.len() >= MAX_JOBS || all.contains_key(&identity) {
            return false;
        }
        all.insert(
            identity,
            Entry {
                request: request.clone(),
                tab,
                route_generation,
                document,
                frame_id,
                created: Instant::now(),
                started: None,
                cancel,
                completion: None,
                path: None,
                result: None,
            },
        );
        true
    });
    if !admitted {
        send_state(sender, tab, json!({"error":SaveError::Busy.code()}));
        return;
    }
    if sender
        .send(Sobytie::Sluzhebnoe(json!({
            "tip":"page-save","id":tab,"browser_id":identity,
            "route_generation":route_generation,"request":request,"filename":filename
        })))
        .is_err()
    {
        if let Ok(mut all) = jobs().lock() {
            all.remove(&identity);
        }
    }
}

/// From dispatch of Komanda::PageSaveTarget. path=None is a neutral cancel.
/// The document/generation binding is re-verified before the CDP call.
pub fn target(
    browser: &Browser,
    route_generation: u64,
    request: &str,
    path: Option<String>,
    sender: &Sender<Sobytie>,
) {
    let identity = browser.identifier();
    let pending = jobs().lock().ok().and_then(|all| {
        all.get(&identity)
            .filter(|e| {
                e.started.is_none()
                    && e.request == request
                    && e.route_generation == route_generation
                    && e.created.elapsed() < DIALOG_TIMEOUT
            })
            .map(|e| (e.tab, e.frame_id.clone(), e.document.clone()))
    });
    let Some((tab, frame_id, document)) = pending else {
        return;
    };
    let fail = |error: SaveError| {
        if let Ok(mut all) = jobs().lock() {
            all.remove(&identity);
        }
        send_state(sender, tab, json!({"error":error.code()}));
    };
    let Some(path) = path else {
        if let Ok(mut all) = jobs().lock() {
            all.remove(&identity);
        }
        send_state(sender, tab, json!({"cancelled":true}));
        return;
    };
    if crate::browser_lock::locked_global() {
        return fail(SaveError::Locked);
    }
    if currently_on(ThreadId::UI) == 0 {
        return fail(SaveError::WrongThread);
    }
    if path.len() > MAX_PATH_BYTES || !valid_request(request) {
        return fail(SaveError::Storage);
    }
    if !document_matches(browser, identity, &frame_id, &document) {
        return fail(SaveError::PageChanged);
    }
    let Some(message) = cdp::reserve(1) else {
        return fail(SaveError::StateUnavailable);
    };
    let completion = Arc::new(Mutex::new(Completion {
        done: false,
        reply: None,
    }));
    let Some(registration) = browser.host().and_then(|host| {
        host.add_dev_tools_message_observer(Some(&mut Observer::new(
            identity,
            frame_id.clone(),
            document.clone(),
            message,
            completion.clone(),
        )))
    }) else {
        return fail(SaveError::StateUnavailable);
    };
    let installed = jobs().lock().is_ok_and(|mut all| {
        let Some(entry) = all.get_mut(&identity).filter(|e| {
            e.started.is_none() && e.request == request && e.route_generation == route_generation
        }) else {
            return false;
        };
        entry.started = Some(Instant::now());
        entry.completion = Some(completion);
        entry.path = Some(PathBuf::from(path));
        true
    });
    if !installed {
        return;
    }
    REGISTRATIONS.with(|regs| regs.borrow_mut().insert(identity, registration));
    if !cdp::send(
        browser,
        message,
        "Page.captureSnapshot",
        json!({"format":"mhtml"}),
    ) {
        REGISTRATIONS.with(|regs| regs.borrow_mut().remove(&identity));
        fail(SaveError::StateUnavailable);
    }
}

/// Called where cef_youtube_download::cancel is called (lock/close/nav/route
/// change). A pending dialog entry is dropped silently; a running writer
/// observes the cancel flag before the atomic rename.
pub fn cancel(browser: i32) {
    if let Ok(mut all) = jobs().lock() {
        if let Some(entry) = all.get_mut(&browser) {
            let _ = entry.cancel.send(true);
        }
        if all.get(&browser).is_some_and(|e| e.started.is_none()) {
            all.remove(&browser);
        }
    }
    REGISTRATIONS.with(|regs| regs.borrow_mut().remove(&browser));
}

fn spawn_writer(
    identity: i32,
    request: String,
    path: PathBuf,
    data: String,
    cancel: watch::Receiver<bool>,
) -> bool {
    std::thread::Builder::new()
        .name("page-save-write".into())
        .stack_size(512 * 1024)
        .spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                atomic_write(&path, data.as_bytes(), &cancel)
            }))
            .unwrap_or(Err(SaveError::Storage));
            if let Ok(mut all) = jobs().lock() {
                if let Some(entry) = all.get_mut(&identity).filter(|e| e.request == request) {
                    entry.result = Some(result);
                }
            }
        })
        .is_ok()
}

/// Called from the owner cycle next to cef_youtube_download::poll on the CEF
/// UI thread: dialog timeout, CDP completion, writer results, typed events.
pub fn poll(browsers: &HashMap<u64, Browser>, sender: &Sender<Sobytie>) {
    let mut updates: Vec<(u64, Value)> = Vec::new();
    let mut drop_registration: Vec<i32> = Vec::new();
    {
        let Ok(mut all) = jobs().lock() else {
            return;
        };
        let mut remove: Vec<i32> = Vec::new();
        for (&identity, entry) in all.iter_mut() {
            if let Some(result) = entry.result.take() {
                updates.push((
                    entry.tab,
                    match result {
                        Ok(bytes) => json!({"done":true,"bytes":bytes}),
                        Err(SaveError::Cancelled) => json!({"cancelled":true}),
                        Err(error) => json!({"error":error.code()}),
                    },
                ));
                remove.push(identity);
                drop_registration.push(identity);
                continue;
            }
            let Some(started) = entry.started else {
                if entry.created.elapsed() > DIALOG_TIMEOUT {
                    updates.push((entry.tab, json!({"cancelled":true})));
                    remove.push(identity);
                }
                continue;
            };
            let reply = entry.completion.as_ref().and_then(|completion| {
                completion
                    .lock()
                    .ok()
                    .and_then(|mut c| if c.done { c.reply.take() } else { None })
            });
            match reply {
                Some(Ok(data)) => {
                    let still = browsers
                        .values()
                        .any(|b| document_matches(b, identity, &entry.frame_id, &entry.document));
                    let path = entry.path.take();
                    if !still {
                        updates.push((entry.tab, json!({"error":SaveError::PageChanged.code()})));
                        remove.push(identity);
                        drop_registration.push(identity);
                    } else if let Some(path) = path {
                        let (cancel_tx, cancel_rx) = watch::channel(false);
                        std::mem::swap(&mut entry.cancel, &mut cancel_tx);
                        if !spawn_writer(identity, entry.request.clone(), path, data, cancel_rx) {
                            updates.push((entry.tab, json!({"error":SaveError::Storage.code()})));
                            remove.push(identity);
                            drop_registration.push(identity);
                        }
                        entry.completion = None;
                    }
                }
                Some(Err(error)) => {
                    updates.push((entry.tab, json!({"error":error.code()})));
                    remove.push(identity);
                    drop_registration.push(identity);
                }
                None => {
                    if started.elapsed() > CAPTURE_TIMEOUT {
                        updates
                            .push((entry.tab, json!({"error":SaveError::CaptureTimeout.code()})));
                        remove.push(identity);
                        drop_registration.push(identity);
                    }
                }
            }
        }
        for identity in remove {
            all.remove(&identity);
        }
    }
    for identity in drop_registration {
        REGISTRATIONS.with(|regs| regs.borrow_mut().remove(&identity));
    }
    for (tab, state) in updates {
        send_state(sender, tab, state);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filename_known_epochs() {
        assert_eq!(make_filename(0), "page-19700101-000000.mhtml");
        assert_eq!(make_filename(86400), "page-19700102-000000.mhtml");
        // 2000-02-29T00:00:00Z (leap day)
        assert_eq!(make_filename(951782400), "page-20000229-000000.mhtml");
        // 2038-01-19T03:14:07Z
        assert_eq!(make_filename(2147483647), "page-20380119-031407.mhtml");
        assert!(valid_filename(&make_filename(unix_now())));
    }

    #[test]
    fn filename_validation_bounds() {
        assert!(valid_filename("page-20261126-114530.mhtml"));
        assert!(!valid_filename("page-.mhtmlx"));
        assert!(!valid_filename("evil.mhtml"));
        assert!(!valid_filename("page-a b.mhtml"));
        assert!(!valid_filename("page-../x.mhtml"));
        assert!(!valid_filename(&format!("page-{}.mhtml", "a".repeat(64))));
    }

    #[test]
    fn request_validation() {
        assert!(valid_request("0123456789abcdef0123456789abcdef"));
        assert!(!valid_request("0123456789abcdef0123456789abcde"));
        assert!(!valid_request("0123456789abcdef0123456789abcdez"));
        assert!(!valid_request(""));
    }

    #[test]
    fn mhtml_extraction() {
        let good = br#"{"data":"From: <Saved by Blink>\nMIME-Version: 1.0\nContent-Type: multipart/related\n\nbody"}"#;
        assert!(extract_mhtml_with_cap(good, 1 << 20).is_ok());
        assert_eq!(
            extract_mhtml_with_cap(b"not json", 1 << 20),
            Err(SaveError::InvalidResult)
        );
        assert_eq!(
            extract_mhtml_with_cap(br#"{"data":""}"#, 1 << 20),
            Err(SaveError::InvalidResult)
        );
        assert_eq!(
            extract_mhtml_with_cap(br#"{"other":1}"#, 1 << 20),
            Err(SaveError::InvalidResult)
        );
        // bare HTML is not an MHTML archive
        assert_eq!(
            extract_mhtml_with_cap(br#"{"data":"<html><body>hi</body></html>"}"#, 1 << 20),
            Err(SaveError::InvalidResult)
        );
        assert_eq!(
            extract_mhtml_with_cap(br#"{"data":"MIME-Version: 1.0 xxxxxxxxxxxxxxxx"}"#, 32),
            Err(SaveError::TooLarge)
        );
        assert_eq!(
            extract_mhtml_with_cap(b"", 1 << 20),
            Err(SaveError::CdpResultAbsent)
        );
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let mut nonce = [0u8; 8];
        OsRng.try_fill_bytes(&mut nonce).unwrap();
        let dir = std::env::temp_dir().join(format!(
            "nbps-test-{}-{}-{}",
            tag,
            std::process::id(),
            nonce.iter().map(|b| format!("{b:02x}")).collect::<String>()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn atomic_write_publishes_and_cleans_temp() {
        let dir = temp_dir("ok");
        let target = dir.join("page-20261126-000000.mhtml");
        let (_tx, rx) = watch::channel(false);
        let n = atomic_write(&target, b"MIME-Version: 1.0\nx", &rx).unwrap();
        assert_eq!(n, 18);
        assert_eq!(std::fs::read(&target).unwrap(), b"MIME-Version: 1.0\nx");
        let leftovers = std::fs::read_dir(&dir).unwrap().count();
        assert_eq!(leftovers, 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn atomic_write_cancel_removes_temp() {
        let dir = temp_dir("cancel");
        let target = dir.join("page-20261126-000000.mhtml");
        let (tx, rx) = watch::channel(false);
        tx.send(true).unwrap();
        assert_eq!(
            atomic_write(&target, b"MIME-Version: 1.0", &rx),
            Err(SaveError::Cancelled)
        );
        assert!(!target.exists());
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn atomic_write_rejects_bad_targets() {
        let dir = temp_dir("bad");
        let (_tx, rx) = watch::channel(false);
        assert_eq!(
            atomic_write(Path::new("relative/page-1.mhtml"), b"x", &rx),
            Err(SaveError::Storage)
        );
        assert_eq!(
            atomic_write(&dir.join("not-a-page.html"), b"x", &rx),
            Err(SaveError::Storage)
        );
        assert_eq!(
            atomic_write(&dir.join("missing").join("page-1.mhtml"), b"x", &rx),
            Err(SaveError::Storage)
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn atomic_write_overwrites_existing() {
        let dir = temp_dir("overwrite");
        let target = dir.join("page-20261126-000000.mhtml");
        std::fs::write(&target, b"old").unwrap();
        let (_tx, rx) = watch::channel(false);
        atomic_write(&target, b"MIME-Version: 1.0\nnew", &rx).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"MIME-Version: 1.0\nnew");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
