//! Optional metadata actor. Its completion is never route/playback authority.
//! The CEF UI caller must recheck the exact operation/document grant on delivery.
use crate::{
    tab_routes::NativeRoute,
    youtube_light::VideoId,
    youtube_light_http::{self as http, ApiError, AuxiliaryApi, DislikeEstimate, SkipSegment},
};
use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Mutex, OnceLock},
    time::{Duration, Instant},
};
use tokio::sync::{mpsc, watch};
const MAX_ACTIVE: usize = 4;
const WINDOW: Duration = Duration::from_secs(60);
const MAX_STARTS: usize = 30;
const MAX_DOCUMENT: usize = 4096;
/// No Debug/Serialize: document is an internal capability, not report data.
pub struct Ready {
    pub browser: i32,
    pub operation: u64,
    pub document: String,
    pub segments: Option<Vec<SkipSegment>>,
    pub dislikes: Option<DislikeEstimate>,
    pub segment_error: Option<&'static str>,
    pub rating_error: Option<&'static str>,
}
struct Entry {
    identity: u64,
    operation: u64,
    cancel: watch::Sender<bool>,
}
#[derive(Default)]
struct Manager {
    entries: HashMap<i32, Entry>,
    starts: VecDeque<Instant>,
    next: u64,
    backoff: Option<Instant>,
}
impl Manager {
    fn reserve(
        &mut self,
        browser: i32,
        operation: u64,
        now: Instant,
    ) -> Result<(u64, watch::Receiver<bool>), &'static str> {
        while self.starts.front().is_some_and(|at| {
            now.checked_duration_since(*at)
                .is_some_and(|age| age >= WINDOW)
        }) {
            self.starts.pop_front();
        }
        if self.backoff.is_some_and(|until| now < until) {
            return Err("rate_limited");
        }
        if self.entries.contains_key(&browser) {
            return Err("browser_busy");
        }
        if self.entries.len() >= MAX_ACTIVE {
            return Err("auxiliary_busy");
        }
        if self.starts.len() >= MAX_STARTS {
            return Err("rate_limited");
        }
        self.next = self.next.checked_add(1).ok_or("operation_exhausted")?;
        let (tx, rx) = watch::channel(false);
        let identity = self.next;
        self.entries.insert(
            browser,
            Entry {
                identity,
                operation,
                cancel: tx,
            },
        );
        self.starts.push_back(now);
        Ok((identity, rx))
    }
    fn current(&self, browser: i32, identity: u64, operation: u64) -> bool {
        self.entries.get(&browser).is_some_and(|e| {
            e.identity == identity && e.operation == operation && !*e.cancel.borrow()
        })
    }
    fn finish(&mut self, browser: i32, identity: u64) {
        if self
            .entries
            .get(&browser)
            .is_some_and(|e| e.identity == identity)
        {
            self.entries.remove(&browser);
        }
    }
    fn cancel(&self, browser: i32) {
        if let Some(entry) = self.entries.get(&browser) {
            let _ = entry.cancel.send(true);
        }
    }
    fn cooldown(&mut self, now: Instant) {
        self.backoff = now.checked_add(WINDOW);
    }
}
type Callback = Box<dyn FnOnce(Ready) -> bool + Send + 'static>;
struct Work {
    browser: i32,
    operation: u64,
    identity: u64,
    document: String,
    video: VideoId,
    duration: f64,
    route: NativeRoute,
    cancel: watch::Receiver<bool>,
    callback: Callback,
}
struct Handle {
    tx: mpsc::Sender<Work>,
    manager: Arc<Mutex<Manager>>,
}
static ACTOR: OnceLock<Result<Handle, &'static str>> = OnceLock::new();
fn actor() -> Result<&'static Handle, &'static str> {
    ACTOR.get_or_init(||{
  let runtime=tokio::runtime::Builder::new_current_thread().max_blocking_threads(1).enable_all().build().map_err(|_|"worker_unavailable")?;
  let(tx,mut rx)=mpsc::channel::<Work>(MAX_ACTIVE);let manager=Arc::new(Mutex::new(Manager::default()));let owned=manager.clone();
  std::thread::Builder::new().name("youtube-auxiliary".into()).spawn(move||runtime.block_on(async move{
   let mut jobs=tokio::task::JoinSet::new();
   loop{tokio::select!{
    work=rx.recv()=>match work{Some(work)=>{let manager=owned.clone();jobs.spawn(async move{execute(work,manager).await;});},None=>break},
    _=jobs.join_next(),if !jobs.is_empty()=>{},
   }}
   jobs.abort_all();while jobs.join_next().await.is_some(){}
  })).map_err(|_|"worker_unavailable")?;
  Ok(Handle{tx,manager})
 }).as_ref().map_err(|error|*error)
}
/// Starts at most two fixed API requests using the caller's retained native
/// route lease. Failure/429 affects optional metadata only; never retries.
pub fn start(
    browser: i32,
    operation: u64,
    document: String,
    video: VideoId,
    duration: f64,
    route: NativeRoute,
    callback: impl FnOnce(Ready) -> bool + Send + 'static,
) -> Result<(), &'static str> {
    if browser <= 0
        || operation == 0
        || document.is_empty()
        || document.len() > MAX_DOCUMENT
        || document.chars().any(char::is_control)
        || !duration.is_finite()
        || duration <= 0.0
        || duration > 7.0 * 86400.0
    {
        return Err("invalid_auxiliary_input");
    }
    if !route.validate() {
        return Err("route_unavailable");
    }
    let handle = actor()?;
    let (identity, cancel) = handle
        .manager
        .lock()
        .map_err(|_| "worker_unavailable")?
        .reserve(browser, operation, Instant::now())?;
    let work = Work {
        browser,
        operation,
        identity,
        document,
        video,
        duration,
        route,
        cancel,
        callback: Box::new(callback),
    };
    if handle.tx.try_send(work).is_err() {
        if let Ok(mut manager) = handle.manager.lock() {
            manager.finish(browser, identity);
        }
        return Err("worker_unavailable");
    }
    Ok(())
}
/// Mark cancellation but retain the slot until both HTTP futures are dropped.
pub fn cancel(browser: i32) {
    if let Some(Ok(actor)) = ACTOR.get() {
        if let Ok(manager) = actor.manager.lock() {
            manager.cancel(browser);
        }
    }
}
pub fn cancel_all() {
    if let Some(Ok(actor)) = ACTOR.get() {
        if let Ok(manager) = actor.manager.lock() {
            for browser in manager.entries.keys() {
                manager.cancel(*browser);
            }
        }
    }
}
async fn cancellation(cancel: &mut watch::Receiver<bool>) {
    loop {
        if *cancel.borrow() {
            return;
        }
        if cancel.changed().await.is_err() {
            return;
        }
    }
}
fn api_error(error: ApiError) -> &'static str {
    match error {
        ApiError::InvalidInput => "invalid_input",
        ApiError::RouteUnavailable => "route_unavailable",
        ApiError::Unavailable => "api_unavailable",
        ApiError::RateLimited => "rate_limited",
        ApiError::Oversized => "api_oversized",
        ApiError::InvalidResponse => "api_invalid_response",
    }
}
/// RAII release also handles a panicking callback; old jobs cannot remove a
/// later operation. Declared before the request future so drop order is safe.
struct Slot {
    manager: Arc<Mutex<Manager>>,
    browser: i32,
    identity: u64,
}
impl Drop for Slot {
    fn drop(&mut self) {
        if let Ok(mut m) = self.manager.lock() {
            m.finish(self.browser, self.identity);
        }
    }
}
async fn execute(mut work: Work, manager: Arc<Mutex<Manager>>) {
    let _slot = Slot {
        manager: manager.clone(),
        browser: work.browser,
        identity: work.identity,
    };
    let outcome = {
        let requests = async {
            tokio::join!(
                fetch_observed(
                    &work.route,
                    AuxiliaryApi::SponsorBlock,
                    &work.video,
                    &manager
                ),
                fetch_observed(&work.route, AuxiliaryApi::Dislikes, &work.video, &manager)
            )
        };
        tokio::select! {biased;_=cancellation(&mut work.cancel)=>None,values=requests=>Some(values)}
    }; // cancellation drops BOTH request futures before releasing the slot
    let Some((segments, dislikes)) = outcome else {
        return;
    };
    let segments = segments.and_then(|value| http::skip_segments(&value, work.duration));
    let dislikes = dislikes.and_then(|value| http::dislike_estimate(&value));
    {
        let Ok(state) = manager.lock() else {
            return;
        };
        if !state.current(work.browser, work.identity, work.operation) {
            return;
        }
    }
    if *work.cancel.borrow() {
        return;
    }
    let (segment_values, segment_error) = match segments {
        Ok(v) => (Some(v), None),
        Err(e) => (None, Some(api_error(e))),
    };
    let (rating_values, rating_error) = match dislikes {
        Ok(v) => (Some(v), None),
        Err(e) => (None, Some(api_error(e))),
    };
    // Callback must ONLY enqueue the typed observation. Cancellation may race an
    // already executing callback, therefore the UI MUST revalidate the grant.
    let _ = (work.callback)(Ready {
        browser: work.browser,
        operation: work.operation,
        document: work.document,
        segments: segment_values,
        dislikes: rating_values,
        segment_error,
        rating_error,
    });
}

async fn fetch_observed(
    route: &NativeRoute,
    api: AuxiliaryApi,
    video: &VideoId,
    manager: &Mutex<Manager>,
) -> Result<serde_json::Value, ApiError> {
    let result = http::fetch(route, api, video).await;
    // Start backoff as soon as either API returns 429, even if its sibling
    // request is still pending or this operation is cancelled afterwards.
    if result.as_ref().is_err_and(|e| *e == ApiError::RateLimited) {
        if let Ok(mut state) = manager.lock() {
            state.cooldown(Instant::now());
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bounded_slots_preserve_cancelled_ownership_until_actual_finish() {
        let mut manager = Manager::default();
        let now = Instant::now();
        let (first, rx) = manager.reserve(1, 1, now).unwrap();
        for browser in 2..=4 {
            manager.reserve(browser, 1, now).unwrap();
        }
        assert_eq!(manager.reserve(5, 1, now).unwrap_err(), "auxiliary_busy");
        manager.cancel(1);
        assert!(*rx.borrow());
        assert_eq!(manager.reserve(1, 2, now).unwrap_err(), "browser_busy");
        manager.finish(1, first);
        let (next, _) = manager.reserve(1, 2, now).unwrap();
        manager.finish(1, first);
        assert!(manager.current(1, next, 2));
    }
    #[test]
    fn sliding_window_and_429_backoff_are_independent_and_bounded() {
        let mut manager = Manager::default();
        let now = Instant::now();
        for operation in 1..=30 {
            let (id, _) = manager.reserve(1, operation, now).unwrap();
            manager.finish(1, id);
        }
        assert_eq!(manager.starts.len(), 30);
        assert_eq!(manager.reserve(1, 31, now).unwrap_err(), "rate_limited");
        let later = now + WINDOW;
        let (id, _) = manager.reserve(1, 31, later).unwrap();
        manager.finish(1, id);
        manager.cooldown(later);
        assert_eq!(
            manager
                .reserve(1, 32, later + Duration::from_secs(59))
                .unwrap_err(),
            "rate_limited"
        );
        assert!(manager.reserve(1, 32, later + WINDOW).is_ok());
    }
    #[tokio::test]
    async fn cancellation_drops_both_inflight_futures_before_slot_release() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        struct Guard(Arc<AtomicUsize>);
        impl Drop for Guard {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
        }
        let dropped = Arc::new(AtomicUsize::new(0));
        let guard1 = Guard(dropped.clone());
        let guard2 = Guard(dropped.clone());
        let (tx, mut rx) = watch::channel(false);
        let requests = async {
            tokio::join!(
                async {
                    let _g = guard1;
                    std::future::pending::<()>().await
                },
                async {
                    let _g = guard2;
                    std::future::pending::<()>().await
                }
            )
        };
        tx.send(true).unwrap();
        {
            tokio::select! {biased;_=cancellation(&mut rx)=>{},_=requests=>panic!("pending requests completed")}
        }
        assert_eq!(dropped.load(Ordering::SeqCst), 2);
    }
}
