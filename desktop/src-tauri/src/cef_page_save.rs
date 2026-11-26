//! Native-only page save: Ctrl+S -> parent-owned save dialog -> CDP
//! `Page.captureSnapshot` (format mhtml) -> bounded temp write -> atomic
//! publish. The renderer never supplies a path, HTML, or a URL. Only the
//! single active owned browser is admitted; generation/document/browser
//! binding is re-verified on the CEF UI thread after the dialog returns and
//! again immediately before the atomic rename, with no UI loop in between.
//! The worker thread writes only a private temp file; the rename that
//! publishes bytes to the user path happens exclusively in poll on the UI
//! thread. Lock/navigation/close cancellation is owned by the Root process
//! (on_before_browse / BrowserLocked / close call [`cancel`]); this module
//! deliberately does not treat `locked_global` as shared state inside the
//! CEF engine process. Nothing here logs paths, URLs, HTML, titles, or
//! secrets: the UI learns only compact typed state events.
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
/// Cap for the decoded MHTML payload itself.
const MAX_MHTML_BYTES: usize = 128 * 1024 * 1024;
/// Cap for the raw CDP JSON payload, checked before parsing; the small
/// allowance above the payload cap covers JSON framing/escapes.
const MAX_RESULT_BYTES: usize = MAX_MHTML_BYTES + 64 * 1024;
const DIALOG_TIMEOUT: Duration = Duration::from_secs(120);
const CAPTURE_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum SaveError {
    Cancelled,
    Busy,
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
    reply: Option<Result<Zeroizing<String>, SaveError>>,
}

struct Entry {
    sender: Sender<Sobytie>,
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
    result: Option<Result<(PathBuf, u64), SaveError>>,
}

fn jobs() -> &'static Mutex<HashMap<i32, Entry>> {
    static JOBS: OnceLock<Mutex<HashMap<i32, Entry>>> = OnceLock::new();
    JOBS.get_or_init(Default::default)
}

thread_local! {
    /// CDP observer registrations live on the creating CEF UI thread only.
    static REGISTRATIONS: RefCell<HashMap<i32, Registration>> = RefCell::new(HashMap::new());
}

/// Shutdown/hibernation gate for the owner: true while any save is pending.
pub fn active(browser: i32) -> bool {
    jobs().lock().map_or(true, |all| all.contains_key(&browser))
}
pub fn any_active() -> bool {
    WRITERS.load(std::sync::atomic::Ordering::SeqCst) > 0
        || jobs().lock().map_or(true, |all| !all.is_empty())
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

/// Privacy-safe suggested name: no title, no URL, only the UTC timestamp.
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

/// Validation for the suggested default name only.
fn valid_suggested_filename(name: &str) -> bool {
    name.len() <= 64
        && name.starts_with("page-")
        && name.ends_with(".mhtml")
        && name
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-'))
}

/// The user may pick any ordinary .mhtml name in the native dialog; only
/// shape checks apply to the chosen basename.
fn valid_chosen_filename(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 255
        && !name.starts_with('.')
        && name
            .rsplit_once('.')
            .is_some_and(|(_, ext)| ext.eq_ignore_ascii_case("mhtml"))
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
/// The raw payload length is capped before JSON parsing.
fn extract_mhtml_with_caps(
    bytes: &[u8],
    raw_cap: usize,
    data_cap: usize,
) -> Result<Zeroizing<String>, SaveError> {
    if bytes.is_empty() {
        return Err(SaveError::CdpResultAbsent);
    }
    if bytes.len() > raw_cap {
        return Err(SaveError::TooLarge);
    }
    let raw = Zeroizing::new(bytes.to_vec());
    let tree: Value = serde_json::from_slice(&raw).map_err(|_| SaveError::InvalidResult)?;
    let data = tree
        .get("data")
        .and_then(Value::as_str)
        .ok_or(SaveError::InvalidResult)?;
    if data.is_empty() {
        return Err(SaveError::InvalidResult);
    }
    if data.len() > data_cap {
        return Err(SaveError::TooLarge);
    }
    let head = &data.as_bytes()[..data.len().min(4096)];
    if !head
        .windows(b"MIME-Version".len())
        .any(|w| w == b"MIME-Version")
    {
        return Err(SaveError::InvalidResult);
    }
    Ok(Zeroizing::new(data.to_owned()))
}

fn extract_mhtml(bytes: &[u8]) -> Result<Zeroizing<String>, SaveError> {
    extract_mhtml_with_caps(bytes, MAX_RESULT_BYTES, MAX_MHTML_BYTES)
}

/// Worker stage: write only a private sibling temp file (create_new, 0600 on
/// unix, fsync). A cancel observed before returning removes the temp. The
/// temp path is published to the user path exclusively by poll on the UI
/// thread after fresh ownership checks.
fn write_temp(
    target: &Path,
    bytes: &[u8],
    cancel: &watch::Receiver<bool>,
) -> Result<(PathBuf, u64), SaveError> {
    if !target.is_absolute() {
        return Err(SaveError::Storage);
    }
    let name = target
        .file_name()
        .and_then(|n| n.to_str())
        .filter(|n| valid_chosen_filename(n))
        .ok_or(SaveError::Storage)?;
    let parent = target
        .parent()
        .filter(|p| p.is_dir())
        .ok_or(SaveError::Storage)?;
    let mut nonce = [0u8; 8];
    OsRng
        .try_fill_bytes(&mut nonce)
        .map_err(|_| SaveError::RandomUnavailable)?;
    let temp = parent.join(format!(
        ".{}.part-{}-{}",
        name,
        std::process::id(),
        nonce.iter().map(|b| format!("{b:02x}")).collect::<String>()
    ));
    let written = (|| -> Result<u64, SaveError> {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temp).map_err(|_| SaveError::Storage)?;
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
    Ok((temp, bytes_written))
}

/// Publish immediately after UI-thread document validation. The temporary
/// file was fsynced in the same directory; never delete the old destination.
#[cfg(not(windows))]
fn publish_replace(temp: &Path, target: &Path) -> Result<(), SaveError> {
    std::fs::rename(temp, target).map_err(|_| SaveError::Storage)
}
#[cfg(windows)]
fn publish_replace(temp: &Path, target: &Path) -> Result<(), SaveError> {
    use std::os::windows::ffi::OsStrExt;
    use windows::{
        core::PCWSTR,
        Win32::Storage::FileSystem::{
            MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
        },
    };
    let source: Vec<u16> = temp.as_os_str().encode_wide().chain(Some(0)).collect();
    let destination: Vec<u16> = target.as_os_str().encode_wide().chain(Some(0)).collect();
    if source[..source.len() - 1].contains(&0) || destination[..destination.len() - 1].contains(&0)
    {
        return Err(SaveError::Storage);
    }
    unsafe {
        MoveFileExW(
            PCWSTR(source.as_ptr()),
            PCWSTR(destination.as_ptr()),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    }
    .map_err(|_| SaveError::Storage)
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
        if all.len() >= MAX_JOBS
            || WRITERS.load(std::sync::atomic::Ordering::SeqCst) >= MAX_JOBS
            || all.contains_key(&identity)
        {
            return false;
        }
        all.insert(
            identity,
            Entry {
                sender: sender.clone(),
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

/// Called by the owner for lock/close/navigation/route-change cancellation
/// (including same-URL reload via on_before_browse). A pending dialog entry
/// is dropped silently; a running writer observes the flag before finishing;
/// an already-written temp is removed here.
pub fn cancel(browser: i32) {
    if let Ok(mut all) = jobs().lock() {
        if let Some(mut entry) = all.remove(&browser) {
            // send() does not retain the value with zero receivers (before
            // writer startup). send_replace() always latches cancellation.
            entry.cancel.send_replace(true);
            if let Some(Ok((temp, _))) = entry.result.take() {
                let _ = std::fs::remove_file(temp);
            }
        }
    }
    REGISTRATIONS.with(|regs| regs.borrow_mut().remove(&browser));
}

static WRITERS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
struct WriterGuard;
impl Drop for WriterGuard {
    fn drop(&mut self) {
        WRITERS.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}
fn spawn_writer(
    identity: i32,
    request: String,
    target: PathBuf,
    data: Zeroizing<String>,
    cancel: watch::Receiver<bool>,
) -> bool {
    WRITERS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let guard = WriterGuard;
    std::thread::Builder::new()
        .name("page-save-write".into())
        .stack_size(512 * 1024)
        .spawn(move || {
            let _guard = guard;
            let mut result = Some(
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    write_temp(&target, data.as_bytes(), &cancel)
                }))
                .unwrap_or(Err(SaveError::Storage)),
            );
            if let Ok(mut all) = jobs().lock() {
                if let Some(entry) = all.get_mut(&identity).filter(|e| e.request == request) {
                    entry.result = result.take();
                }
            }
            if let Some(Ok((temp, _))) = result {
                let _ = std::fs::remove_file(temp);
            }
        })
        .is_ok()
}

/// Called from the owner cycle next to cef_youtube_download::poll on the CEF
/// UI thread: dialog timeout, CDP completion, temp publish, typed events. A
/// browser absent from this local map belongs to another window's cycle and
/// is skipped untouched, never cancelled by this one.
pub fn poll(browsers: &HashMap<u64, Browser>, _sender: &Sender<Sobytie>) {
    let mut updates: Vec<(Sender<Sobytie>, u64, Value)> = Vec::new();
    let mut drop_registration: Vec<i32> = Vec::new();
    {
        let Ok(mut all) = jobs().lock() else {
            return;
        };
        let mut remove: Vec<i32> = Vec::new();
        for (&identity, entry) in all.iter_mut() {
            // A crashed browser may disappear without on_before_close. Do not
            // retain its ready temp, observer or shutdown blocker forever.
            if entry.created.elapsed() > DIALOG_TIMEOUT + CAPTURE_TIMEOUT {
                entry.cancel.send_replace(true);
                if let Some(Ok((temp, _))) = entry.result.take() {
                    let _ = std::fs::remove_file(temp);
                }
                updates.push((
                    entry.sender.clone(),
                    entry.tab,
                    json!({"error":SaveError::CaptureTimeout.code()}),
                ));
                remove.push(identity);
                drop_registration.push(identity);
                continue;
            }
            if let Some(result) = entry.result.take() {
                match result {
                    Err(SaveError::Cancelled) => {
                        updates.push((entry.sender.clone(), entry.tab, json!({"cancelled":true})));
                        remove.push(identity);
                        drop_registration.push(identity);
                    }
                    Err(error) => {
                        updates.push((
                            entry.sender.clone(),
                            entry.tab,
                            json!({"error":error.code()}),
                        ));
                        remove.push(identity);
                        drop_registration.push(identity);
                    }
                    Ok((temp, bytes)) => {
                        if *entry.cancel.borrow() {
                            let _ = std::fs::remove_file(&temp);
                            updates.push((
                                entry.sender.clone(),
                                entry.tab,
                                json!({"cancelled":true}),
                            ));
                            remove.push(identity);
                            drop_registration.push(identity);
                            continue;
                        }
                        let Some(owner) = browsers.values().find(|b| b.identifier() == identity)
                        else {
                            // Owned by another window's cycle: put the result
                            // back untouched and let that cycle publish it.
                            entry.result = Some(Ok((temp, bytes)));
                            continue;
                        };
                        if !document_matches(owner, identity, &entry.frame_id, &entry.document) {
                            let _ = std::fs::remove_file(&temp);
                            updates.push((
                                entry.sender.clone(),
                                entry.tab,
                                json!({"error":SaveError::PageChanged.code()}),
                            ));
                            remove.push(identity);
                            drop_registration.push(identity);
                            continue;
                        }
                        let Some(target) = entry.path.take() else {
                            let _ = std::fs::remove_file(&temp);
                            updates.push((
                                entry.sender.clone(),
                                entry.tab,
                                json!({"error":SaveError::StateUnavailable.code()}),
                            ));
                            remove.push(identity);
                            drop_registration.push(identity);
                            continue;
                        };
                        // Fresh checks above; the atomic publish follows with
                        // no UI loop in between.
                        match publish_replace(&temp, &target) {
                            Ok(()) => updates.push((
                                entry.sender.clone(),
                                entry.tab,
                                json!({"done":true,"bytes":bytes}),
                            )),
                            Err(error) => {
                                let _ = std::fs::remove_file(&temp);
                                updates.push((
                                    entry.sender.clone(),
                                    entry.tab,
                                    json!({"error":error.code()}),
                                ));
                            }
                        }
                        remove.push(identity);
                        drop_registration.push(identity);
                    }
                }
                continue;
            }
            let Some(started) = entry.started else {
                if entry.created.elapsed() > DIALOG_TIMEOUT {
                    updates.push((entry.sender.clone(), entry.tab, json!({"cancelled":true})));
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
                    let Some(owner) = browsers.values().find(|b| b.identifier() == identity) else {
                        // Owned by another window's cycle: put the reply back
                        // untouched and let that cycle drive the writer.
                        if let Some(completion) = entry.completion.as_ref() {
                            if let Ok(mut c) = completion.lock() {
                                c.reply = Some(Ok(data));
                            }
                        }
                        continue;
                    };
                    if !document_matches(owner, identity, &entry.frame_id, &entry.document) {
                        updates.push((
                            entry.sender.clone(),
                            entry.tab,
                            json!({"error":SaveError::PageChanged.code()}),
                        ));
                        remove.push(identity);
                        drop_registration.push(identity);
                        continue;
                    }
                    let path = entry.path.clone();
                    if let Some(path) = path {
                        let (cancel_tx, cancel_rx) = watch::channel(false);
                        entry.cancel = cancel_tx;
                        if !spawn_writer(identity, entry.request.clone(), path, data, cancel_rx) {
                            updates.push((
                                entry.sender.clone(),
                                entry.tab,
                                json!({"error":SaveError::Storage.code()}),
                            ));
                            remove.push(identity);
                            drop_registration.push(identity);
                        }
                        entry.completion = None;
                    }
                }
                Some(Err(error)) => {
                    updates.push((
                        entry.sender.clone(),
                        entry.tab,
                        json!({"error":error.code()}),
                    ));
                    remove.push(identity);
                    drop_registration.push(identity);
                }
                None => {
                    if started.elapsed() > CAPTURE_TIMEOUT {
                        updates.push((
                            entry.sender.clone(),
                            entry.tab,
                            json!({"error":SaveError::CaptureTimeout.code()}),
                        ));
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
    for (owner, tab, state) in updates {
        send_state(&owner, tab, state);
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
        assert!(valid_suggested_filename(&make_filename(unix_now())));
    }

    #[test]
    fn suggested_filename_bounds() {
        assert!(valid_suggested_filename("page-20261126-114530.mhtml"));
        assert!(!valid_suggested_filename("page-.mhtmlx"));
        assert!(!valid_suggested_filename("evil.mhtml"));
        assert!(!valid_suggested_filename("page-a b.mhtml"));
        assert!(!valid_suggested_filename("page-../x.mhtml"));
        assert!(!valid_suggested_filename(&format!(
            "page-{}.mhtml",
            "a".repeat(64)
        )));
    }

    #[test]
    fn chosen_filename_accepts_ordinary_names() {
        assert!(valid_chosen_filename("page-20261126-114530.mhtml"));
        assert!(valid_chosen_filename("Отчёт за ноябрь.mhtml"));
        assert!(valid_chosen_filename("a b (final).MHTML"));
        assert!(!valid_chosen_filename("noext"));
        assert!(!valid_chosen_filename(".hidden.mhtml"));
        assert!(!valid_chosen_filename("x.html"));
        assert!(!valid_chosen_filename(&format!(
            "{}.mhtml",
            "a".repeat(255)
        )));
    }

    #[test]
    fn request_validation() {
        assert!(valid_request("0123456789abcdef0123456789abcdef"));
        assert!(!valid_request("0123456789abcdef0123456789abcde"));
        assert!(!valid_request("0123456789abcdef0123456789abcdez"));
        assert!(!valid_request(""));
    }

    #[test]
    fn mhtml_extraction_caps() {
        let good = br#"{"data":"From: <Saved by Blink>\nMIME-Version: 1.0\nContent-Type: multipart/related\n\nbody"}"#;
        assert!(extract_mhtml_with_caps(good, 1 << 20, 1 << 20).is_ok());
        assert_eq!(
            extract_mhtml_with_caps(b"not json", 1 << 20, 1 << 20).unwrap_err(),
            SaveError::InvalidResult
        );
        assert_eq!(
            extract_mhtml_with_caps(br#"{"data":""}"#, 1 << 20, 1 << 20).unwrap_err(),
            SaveError::InvalidResult
        );
        assert_eq!(
            extract_mhtml_with_caps(br#"{"other":1}"#, 1 << 20, 1 << 20).unwrap_err(),
            SaveError::InvalidResult
        );
        // bare HTML is not an MHTML archive
        assert_eq!(
            extract_mhtml_with_caps(
                br#"{"data":"<html><body>hi</body></html>"}"#,
                1 << 20,
                1 << 20
            )
            .unwrap_err(),
            SaveError::InvalidResult
        );
        // raw payload cap fires before JSON parsing
        assert_eq!(
            extract_mhtml_with_caps(good, 8, 1 << 20).unwrap_err(),
            SaveError::TooLarge
        );
        // decoded payload cap
        assert_eq!(
            extract_mhtml_with_caps(
                br#"{"data":"MIME-Version: 1.0 xxxxxxxxxxxxxxxx"}"#,
                1 << 20,
                32
            )
            .unwrap_err(),
            SaveError::TooLarge
        );
        assert_eq!(
            extract_mhtml_with_caps(b"", 1 << 20, 1 << 20).unwrap_err(),
            SaveError::CdpResultAbsent
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
    fn write_temp_then_publish_roundtrip() {
        let dir = temp_dir("ok");
        let target = dir.join("Отчёт.mhtml");
        let (_tx, rx) = watch::channel(false);
        let (temp, n) = write_temp(&target, b"MIME-Version: 1.0\nx", &rx).unwrap();
        assert_eq!(n, 19);
        assert!(temp.exists());
        assert!(!target.exists(), "worker must not publish by itself");
        publish_replace(&temp, &target).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"MIME-Version: 1.0\nx");
        assert!(!temp.exists());
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn write_temp_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = temp_dir("mode");
        let target = dir.join("page-20261126-000000.mhtml");
        let (_tx, rx) = watch::channel(false);
        let (temp, _) = write_temp(&target, b"MIME-Version: 1.0", &rx).unwrap();
        assert_eq!(
            std::fs::metadata(&temp).unwrap().permissions().mode() & 0o777,
            0o600
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn write_temp_cancel_removes_temp() {
        let dir = temp_dir("cancel");
        let target = dir.join("page-20261126-000000.mhtml");
        let (tx, rx) = watch::channel(false);
        tx.send(true).unwrap();
        assert_eq!(
            write_temp(&target, b"MIME-Version: 1.0", &rx).unwrap_err(),
            SaveError::Cancelled
        );
        assert!(!target.exists());
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn write_temp_rejects_bad_targets() {
        let dir = temp_dir("bad");
        let (_tx, rx) = watch::channel(false);
        assert_eq!(
            write_temp(Path::new("relative/page-1.mhtml"), b"x", &rx).unwrap_err(),
            SaveError::Storage
        );
        assert_eq!(
            write_temp(&dir.join("not-a-page.html"), b"x", &rx).unwrap_err(),
            SaveError::Storage
        );
        assert_eq!(
            write_temp(&dir.join("missing").join("page-1.mhtml"), b"x", &rx).unwrap_err(),
            SaveError::Storage
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn publish_replaces_existing_atomically() {
        let dir = temp_dir("overwrite");
        let target = dir.join("page-20261126-000000.mhtml");
        std::fs::write(&target, b"old").unwrap();
        let (_tx, rx) = watch::channel(false);
        let (temp, _) = write_temp(&target, b"MIME-Version: 1.0\nnew", &rx).unwrap();
        publish_replace(&temp, &target).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"MIME-Version: 1.0\nnew");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn cancel_before_worker_receiver_is_retained() {
        let dir = temp_dir("latched-cancel");
        let target = dir.join("my-page.mhtml");
        let (tx, rx) = watch::channel(false);
        drop(rx);
        tx.send_replace(true);
        let receiver = tx.subscribe();
        assert_eq!(
            write_temp(&target, b"private page", &receiver).unwrap_err(),
            SaveError::Cancelled
        );
        assert!(!target.exists());
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
