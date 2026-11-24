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
    result: Result<(), &'static str>,
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
    let identity = browser.identifier();
    let (frame, document) = document(browser)?;
    let (scope, root) = crate::cef_youtube_resume::setup(browser, &route.key, private)?;
    let permit = Permit::acquire().ok_or(REFUSED)?;
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
            .map_err(|_| REFUSED)
            .and_then(|answer| answer.map(|_| ()).map_err(|_| REFUSED));
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
        return Err(REFUSED);
    }
    let current = PENDING.with(|slot| {
        slot.try_borrow_mut()
            .map(|mut map| take_exact(&mut map, ready.browser, ready.operation))
            .unwrap_or(false)
    });
    if !current
        || !enabled()
        || ready.cancelled.load(Ordering::Acquire)
        || ready.result.is_err()
        || browser.is_valid() == 0
        || browser.identifier() != ready.browser
        || !current_route.validate()
        || current_route.key != ready.route_key
        || route_generation != ready.route_generation
        || private != ready.private
    {
        return Err(REFUSED);
    }
    let (frame, document) = document(browser)?;
    if frame != ready.frame || document != ready.document {
        return Err(REFUSED);
    }
    let actual = crate::cef_youtube_resume::setup(browser, &current_route.key, private)?;
    if actual.0 != ready.scope || actual.1 != ready.root {
        return Err(REFUSED);
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
