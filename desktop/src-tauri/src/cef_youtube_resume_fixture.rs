//! CI-only asynchronous preparation of the real resume service/context.
//! No browser/frame crosses threads; no scope/path/position is serialized.
use crate::{cef_vkladki::Sobytie, tab_routes::NativeRoute, youtube_light_resume::ResumeScope};
use cef::*;
use std::{
    cell::RefCell,
    collections::HashMap,
    panic::{catch_unwind, AssertUnwindSafe},
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        mpsc::Sender,
        Arc,
    },
};

const REFUSED: &str = "Подготовка позиции видео недоступна";
const LIMIT: usize = 4;
static ACTIVE: AtomicUsize = AtomicUsize::new(0);
static OPERATION: AtomicU64 = AtomicU64::new(1);

// This permit lives across worker execution AND queued Ready delivery.
struct Permit;
impl Permit {
    fn acquire() -> Option<Self> {
        let previous = ACTIVE.fetch_add(1, Ordering::AcqRel);
        if previous >= LIMIT {
            ACTIVE.fetch_sub(1, Ordering::AcqRel);
            None
        } else {
            Some(Self)
        }
    }
}
impl Drop for Permit {
    fn drop(&mut self) {
        ACTIVE.fetch_sub(1, Ordering::AcqRel);
    }
}
struct Pending {
    operation: u64,
    cancelled: Arc<AtomicBool>,
}
thread_local! {
    static PENDING: RefCell<HashMap<i32, Pending>> = RefCell::new(HashMap::new());
}

// A blank Browser can exist before memory-bootstrap replaces its first
// document. Only native load completion AND authenticated renderer commit
// for the same current main frame authorize asynchronous preparation.
#[derive(Default)]
struct Commit {
    loaded: Option<(String, String)>,
    authenticated: Option<(String, String)>,
}
impl Commit {
    fn ready(&self, current: &(String, String)) -> bool {
        self.loaded.as_ref() == Some(current) && self.authenticated.as_ref() == Some(current)
    }
}
struct Waiting {
    identity: i32,
    browser: Browser,
    tab: u64,
    route_generation: u64,
    route: NativeRoute,
    private: bool,
    sender: Sender<Sobytie>,
    since: std::time::Instant,
    _permit: Permit,
}
thread_local! {
    static COMMITS:RefCell<HashMap<i32,Commit>>=Default::default();
    static WAITING:RefCell<Vec<Waiting>>=Default::default();
}
fn committed(browser: &Browser) -> bool {
    let Ok(current) = document(browser) else {
        return false;
    };
    let identity = browser.identifier();
    COMMITS.with(|s| s.borrow().get(&identity).is_some_and(|c| c.ready(&current)))
}
pub fn loaded(browser: &Browser, frame: &Frame) {
    if !enabled()
        || currently_on(ThreadId::UI) == 0
        || frame.is_main() == 0
        || frame.is_valid() == 0
    {
        return;
    }
    let Ok(current) = document(browser) else {
        return;
    };
    if CefString::from(&frame.identifier()).to_string() != current.0
        || CefString::from(&frame.url()).to_string() != current.1
    {
        return;
    }
    let identity = browser.identifier();
    COMMITS.with(|s| s.borrow_mut().entry(identity).or_default().loaded = Some(current));
}
pub fn authenticated_document(browser: &Browser, frame: &Frame) {
    if !enabled()
        || currently_on(ThreadId::UI) == 0
        || frame.is_main() == 0
        || frame.is_valid() == 0
    {
        return;
    }
    let Ok(current) = document(browser) else {
        return;
    };
    if CefString::from(&frame.identifier()).to_string() != current.0
        || CefString::from(&frame.url()).to_string() != current.1
    {
        return;
    }
    let identity = browser.identifier();
    COMMITS.with(|s| s.borrow_mut().entry(identity).or_default().authenticated = Some(current));
}
fn waiting_error(waiting: &Waiting, code: &'static str) {
    let _=waiting.sender.send(Sobytie::Sluzhebnoe(serde_json::json!({"tip":"youtube-result","id":waiting.tab,"start":false,"error":REFUSED,"code":code})));
}
pub fn any_pending() -> bool {
    ACTIVE.load(Ordering::Acquire) != 0
}
pub fn poll() {
    if currently_on(ThreadId::UI) == 0 || crate::cef_app_shutdown::active() {
        return;
    }
    let waiting = WAITING.with(|s| std::mem::take(&mut *s.borrow_mut()));
    for pending in waiting {
        if pending.browser.is_valid() == 0 {
            waiting_error(&pending, "resume_native_commit_closed");
            continue;
        }
        if pending.since.elapsed() >= std::time::Duration::from_secs(5) {
            waiting_error(&pending, "resume_native_commit_timeout");
            continue;
        }
        if committed(&pending.browser) {
            let Waiting {
                browser,
                tab,
                route_generation,
                route,
                private,
                sender,
                _permit,
                ..
            } = pending;
            let result = prepare(
                &browser,
                tab,
                route_generation,
                &route,
                private,
                sender.clone(),
                _permit,
            );
            if let Err(code) = result {
                let _=sender.send(Sobytie::Sluzhebnoe(serde_json::json!({"tip":"youtube-result","id":tab,"start":false,"error":REFUSED,"code":failure_code(code)})));
            }
        } else {
            WAITING.with(|s| s.borrow_mut().push(pending));
        }
    }
}
pub fn closed(browser: i32) {
    cancel(browser);
    let removed = WAITING.with(|s| {
        let all = std::mem::take(&mut *s.borrow_mut());
        let (removed, keep) = all
            .into_iter()
            .partition::<Vec<_>, _>(|w| w.identity == browser);
        *s.borrow_mut() = keep;
        removed
    });
    for waiting in &removed {
        waiting_error(waiting, "resume_native_commit_closed");
    }
    drop(removed);
}

// Only fixed categories cross the fixture boundary; no underlying error text.
#[derive(Clone, Copy)]
enum PrepareFailure {
    Busy,
    Full,
    Unavailable,
    Storage,
    InvalidPosition,
    Timeout,
    Panic,
}
impl From<crate::youtube_light_resume_service::ServiceError> for PrepareFailure {
    fn from(error: crate::youtube_light_resume_service::ServiceError) -> Self {
        use crate::youtube_light_resume_service::ServiceError;
        match error {
            ServiceError::Busy => Self::Busy,
            ServiceError::Full => Self::Full,
            ServiceError::Unavailable => Self::Unavailable,
            ServiceError::Storage => Self::Storage,
            ServiceError::InvalidPosition => Self::InvalidPosition,
            ServiceError::Timeout => Self::Timeout,
        }
    }
}
impl PrepareFailure {
    fn code(self) -> &'static str {
        match self {
            Self::Busy => "resume_prepare_busy",
            Self::Full => "resume_prepare_full",
            Self::Unavailable => "resume_prepare_unavailable",
            Self::Storage => "resume_prepare_storage",
            Self::InvalidPosition => "resume_prepare_invalid_position",
            Self::Timeout => "resume_prepare_timeout",
            Self::Panic => "resume_prepare_panic",
        }
    }
}

/// Sanitize terminal fixture/native-open failures without exposing their text.
pub fn failure_code(error: &str) -> &'static str {
    match error {
        "resume_prepare_busy" => "resume_prepare_busy",
        "resume_prepare_full" => "resume_prepare_full",
        "resume_prepare_unavailable" => "resume_prepare_unavailable",
        "resume_prepare_storage" => "resume_prepare_storage",
        "resume_prepare_invalid_position" => "resume_prepare_invalid_position",
        "resume_prepare_timeout" => "resume_prepare_timeout",
        "resume_prepare_panic" => "resume_prepare_panic",
        "resume_admit_wrong_thread" => "resume_admit_wrong_thread",
        "resume_admit_stale" => "resume_admit_stale",
        "resume_admit_disabled" => "resume_admit_disabled",
        "resume_admit_cancelled" => "resume_admit_cancelled",
        "resume_admit_browser_changed" => "resume_admit_browser_changed",
        "resume_admit_route_changed" => "resume_admit_route_changed",
        "resume_admit_private_changed" => "resume_admit_private_changed",
        "resume_admit_document_unavailable" => "resume_admit_document_unavailable",
        "resume_admit_document_changed" => "resume_admit_document_changed",
        "resume_admit_scope_unavailable" => "resume_admit_scope_unavailable",
        "resume_admit_scope_changed" => "resume_admit_scope_changed",
        _ => "resume_open_refused",
    }
}

// Deliberately no Debug/Serialize/Clone: contains private native context data
// and the nonduplicable process-wide permit.
pub struct Ready {
    pub tab: u64,
    pub browser: i32,
    operation: u64,
    route_generation: u64,
    route_key: String,
    private: bool,
    frame: String,
    document: String,
    scope: ResumeScope,
    root: Option<PathBuf>,
    cancelled: Arc<AtomicBool>,
    result: Result<(), PrepareFailure>,
    _permit: Permit,
}

fn enabled() -> bool {
    std::env::var("GITHUB_ACTIONS").as_deref() == Ok("true")
        && std::env::var_os("NEXUSBRO_TEST_DATA").is_some()
        && std::env::var("NEXUSBRO_TEST_YOUTUBE_RESUME").as_deref() == Ok("1")
}
fn document(browser: &Browser) -> Result<(String, String), &'static str> {
    let frame = browser.main_frame().ok_or(REFUSED)?;
    if frame.is_main() == 0 || frame.is_valid() == 0 {
        return Err(REFUSED);
    }
    let id = CefString::from(&frame.identifier()).to_string();
    let url = CefString::from(&frame.url()).to_string();
    if id.is_empty() || id.len() > 256 || url.len() > 2048 {
        return Err(REFUSED);
    }
    Ok((id, url))
}
fn take_exact(map: &mut HashMap<i32, Pending>, browser: i32, operation: u64) -> bool {
    if map.get(&browser).is_some_and(|p| p.operation == operation) {
        map.remove(&browser);
        true
    } else {
        false
    }
}

/// CEF UI only. Caller already enforces active/unlocked/not-closing and checks
/// that the supplied route is the browser's actual registry binding.
pub fn start(
    browser: &Browser,
    tab: u64,
    route_generation: u64,
    route: &NativeRoute,
    private: bool,
    sender: Sender<Sobytie>,
) -> Result<(), &'static str> {
    if !enabled() || currently_on(ThreadId::UI) == 0 || browser.is_valid() == 0 || !route.validate()
    {
        return Err(REFUSED);
    }
    let permit = Permit::acquire().ok_or(REFUSED)?;
    if committed(browser) {
        return prepare(
            browser,
            tab,
            route_generation,
            route,
            private,
            sender,
            permit,
        );
    }
    let identity = browser.identifier();
    let pending = Waiting {
        identity,
        browser: browser.clone(),
        tab,
        route_generation,
        route: route.clone(),
        private,
        sender,
        since: std::time::Instant::now(),
        _permit: permit,
    };
    let mut pending = Some(pending);
    let accepted = WAITING.with(|s| {
        let mut s = s.borrow_mut();
        if s.iter().any(|w| w.identity == identity) {
            false
        } else {
            s.push(pending.take().unwrap());
            true
        }
    });
    drop(pending);
    if accepted {
        Ok(())
    } else {
        Err(REFUSED)
    }
}

fn prepare(
    browser: &Browser,
    tab: u64,
    route_generation: u64,
    route: &NativeRoute,
    private: bool,
    sender: Sender<Sobytie>,
    permit: Permit,
) -> Result<(), &'static str> {
    if !enabled() || currently_on(ThreadId::UI) == 0 || browser.is_valid() == 0 || !route.validate()
    {
        return Err(REFUSED);
    }
    let identity = browser.identifier();
    let (frame, document) = document(browser)?;
    let (scope, root) = crate::cef_youtube_resume::setup(browser, &route.key, private)?;
    let operation = OPERATION
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1))
        .map_err(|_| REFUSED)?;
    let cancelled = Arc::new(AtomicBool::new(false));
    PENDING.with(|slot| {
        let mut map = slot.try_borrow_mut().map_err(|_| REFUSED)?;
        if !map.contains_key(&identity) && map.len() >= LIMIT {
            return Err(REFUSED);
        }
        if let Some(old) = map.insert(
            identity,
            Pending {
                operation,
                cancelled: cancelled.clone(),
            },
        ) {
            old.cancelled.store(true, Ordering::Release);
        }
        Ok(())
    })?;
    let route_key = route.key.clone();
    let spawned = std::thread::Builder::new()
        .name("youtube-resume-fixture".into())
        .spawn(move || {
            // Drop on cancelled work, send failure or panic releases the permit.
            if cancelled.load(Ordering::Acquire) {
                return;
            }
            let result = catch_unwind(AssertUnwindSafe(|| {
                crate::youtube_light_resume_service::prepare(scope.clone(), root.clone())
            }))
            .map_err(|_| PrepareFailure::Panic)
            .and_then(|answer| answer.map(|_| ()).map_err(PrepareFailure::from));
            let ready = Ready {
                tab,
                browser: identity,
                operation,
                route_generation,
                route_key,
                private,
                frame,
                document,
                scope,
                root,
                cancelled,
                result,
                _permit: permit,
            };
            let _ = sender.send(Sobytie::YoutubeResumeFixtureReady(ready));
        });
    if spawned.is_err() {
        PENDING.with(|slot| {
            if let Ok(mut map) = slot.try_borrow_mut() {
                take_exact(&mut map, identity, operation);
            }
        });
        return Err(REFUSED);
    }
    Ok(())
}

/// Native CEF UI only. Consume exact pending operation, then perform every CEF
/// and actual scope/context check outside the RefCell map borrow.
/// Returns a fresh operation as the player grant generation, not route generation.
pub fn admit(
    ready: Ready,
    browser: &Browser,
    current_route: &NativeRoute,
    route_generation: u64,
    private: bool,
) -> Result<(u64, (ResumeScope, Option<PathBuf>)), &'static str> {
    if currently_on(ThreadId::UI) == 0 {
        return Err("resume_admit_wrong_thread");
    }
    let current = PENDING.with(|slot| {
        slot.try_borrow_mut()
            .map(|mut map| take_exact(&mut map, ready.browser, ready.operation))
            .unwrap_or(false)
    });
    if !current {
        return Err("resume_admit_stale");
    }
    if !enabled() {
        return Err("resume_admit_disabled");
    }
    if ready.cancelled.load(Ordering::Acquire) {
        return Err("resume_admit_cancelled");
    }
    if let Err(error) = ready.result {
        return Err(error.code());
    }
    if browser.is_valid() == 0 || browser.identifier() != ready.browser {
        return Err("resume_admit_browser_changed");
    }
    if !current_route.validate()
        || current_route.key != ready.route_key
        || route_generation != ready.route_generation
    {
        return Err("resume_admit_route_changed");
    }
    if private != ready.private {
        return Err("resume_admit_private_changed");
    }
    let (frame, document) = document(browser).map_err(|_| "resume_admit_document_unavailable")?;
    if frame != ready.frame || document != ready.document {
        return Err("resume_admit_document_changed");
    }
    let actual = crate::cef_youtube_resume::setup(browser, &current_route.key, private)
        .map_err(|_| "resume_admit_scope_unavailable")?;
    if actual.0 != ready.scope || actual.1 != ready.root {
        return Err("resume_admit_scope_changed");
    }
    Ok((ready.operation, (ready.scope, ready.root)))
}

/// Consume stale/locked queued readiness without touching any browser or paths.
pub fn discard(ready: Ready) {
    if currently_on(ThreadId::UI) != 0 {
        PENDING.with(|slot| {
            if let Ok(mut map) = slot.try_borrow_mut() {
                take_exact(&mut map, ready.browser, ready.operation);
            }
        });
    }
    ready.cancelled.store(true, Ordering::Release);
    // Ready (including permit) drops here.
}

/// Called on navigation/close/lock, by native CEF identifier, on the CEF UI.
pub fn cancel(browser: i32) {
    if currently_on(ThreadId::UI) == 0 {
        return;
    }
    COMMITS.with(|s| {
        s.borrow_mut().remove(&browser);
    });
    PENDING.with(|slot| {
        if let Ok(mut map) = slot.try_borrow_mut() {
            if let Some(old) = map.remove(&browser) {
                old.cancelled.store(true, Ordering::Release);
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn load_and_authenticated_commit_must_match_exact_current_document() {
        let a = ("frame-a".into(), "about:blank".into());
        let b = ("frame-b".into(), "about:blank".into());
        let mut commit = Commit::default();
        commit.loaded = Some(a.clone());
        assert!(!commit.ready(&a));
        commit.authenticated = Some(b.clone());
        assert!(!commit.ready(&a));
        assert!(!commit.ready(&b));
        commit.loaded = Some(b.clone());
        assert!(commit.ready(&b));
        assert!(!Commit::default().ready(&b));
    }
    #[test]
    fn stale_operation_cannot_consume_newer_pending_request() {
        let old = Arc::new(AtomicBool::new(false));
        let current = Arc::new(AtomicBool::new(false));
        let mut map = HashMap::new();
        map.insert(
            7,
            Pending {
                operation: 1,
                cancelled: old.clone(),
            },
        );
        let replaced = map
            .insert(
                7,
                Pending {
                    operation: 2,
                    cancelled: current.clone(),
                },
            )
            .unwrap();
        replaced.cancelled.store(true, Ordering::Release);
        assert!(!take_exact(&mut map, 7, 1));
        assert!(old.load(Ordering::Acquire));
        assert!(!current.load(Ordering::Acquire));
        assert!(take_exact(&mut map, 7, 2));
        assert!(map.is_empty());
    }
    #[test]
    fn permits_remain_bounded_until_queued_work_is_dropped() {
        let permits: Vec<_> = (0..LIMIT).map(|_| Permit::acquire().unwrap()).collect();
        assert!(Permit::acquire().is_none());
        assert_eq!(ACTIVE.load(Ordering::Acquire), LIMIT);
        drop(permits);
        assert_eq!(ACTIVE.load(Ordering::Acquire), 0);
        drop(Permit::acquire().unwrap());
        assert_eq!(ACTIVE.load(Ordering::Acquire), 0);
    }
}

#[cfg(test)]
mod failure_tests {
    use super::PrepareFailure;
    #[test]
    fn unknown_error_text_cannot_escape_code_boundary() {
        assert_eq!(
            super::failure_code("private/path secret"),
            "resume_open_refused"
        );
        assert_eq!(
            super::failure_code("resume_prepare_storage"),
            "resume_prepare_storage"
        );
    }
    #[test]
    fn every_service_category_has_unique_bounded_static_code() {
        use crate::youtube_light_resume_service::ServiceError;
        let variants = [
            ServiceError::Busy,
            ServiceError::Full,
            ServiceError::Unavailable,
            ServiceError::Storage,
            ServiceError::InvalidPosition,
            ServiceError::Timeout,
        ];
        let mut codes = std::collections::HashSet::new();
        for error in variants {
            let code = PrepareFailure::from(error).code();
            assert!(code.starts_with("resume_prepare_"));
            assert!(code.len() < 48);
            assert!(code.bytes().all(|b| b.is_ascii_lowercase() || b == b'_'));
            assert!(codes.insert(code));
        }
        assert!(codes.insert(PrepareFailure::Panic.code()));
        assert_eq!(codes.len(), 7);
    }
}
