//! CEF resume cache with one bounded IO worker. prepare() belongs on extraction
//! workers; lookup/position/record NEVER perform filesystem IO or wait for it.
//! Queue acceptance is NOT a durable-write receipt. flush() is worker/shutdown
//! ONLY and waits for a bounded barrier. No roots/positions derive Debug/Serialize.
use crate::{
    youtube_light::VideoId,
    youtube_light_resume::{ResumeBook, ResumePoint, ResumeScope, MAX_SECONDS},
};
use std::{
    collections::BTreeMap,
    fmt,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, SyncSender, TrySendError},
        Arc, Mutex, OnceLock,
    },
    time::Duration,
};
pub const MAX_CONTEXTS: usize = 16;
pub const QUEUE_CAPACITY: usize = 64;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServiceError {
    Busy,
    Full,
    Unavailable,
    Storage,
    InvalidPosition,
    Timeout,
}
impl fmt::Display for ServiceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Busy => "Сохранение позиции занято",
            Self::Full => "Достигнут лимит контекстов позиций",
            Self::Unavailable => "Сервис позиций недоступен",
            Self::Storage => "Не удалось сохранить или прочитать позиции",
            Self::InvalidPosition => "Некорректная позиция видео",
            Self::Timeout => "Сохранение позиции ещё не завершено",
        })
    }
}
impl std::error::Error for ServiceError {}
struct State {
    id: u64,
    snapshot: Mutex<Arc<Vec<ResumePoint>>>,
    error: AtomicBool,
    alive: Arc<AtomicBool>,
}
#[derive(Clone)]
pub struct Handle {
    state: Arc<State>,
    sender: SyncSender<Command>,
}
enum SlotState {
    Pending,
    Ready(Handle),
}
struct Slot {
    id: u64,
    scope: ResumeScope,
    root: Option<PathBuf>,
    state: SlotState,
}
enum Command {
    Install {
        id: u64,
        book: ResumeBook,
        state: Arc<State>,
    },
    Record {
        id: u64,
        video: VideoId,
        position: f64,
        duration: Option<f64>,
        when: u64,
    },
    Barrier(SyncSender<Result<(), ServiceError>>),
}
struct Store {
    book: ResumeBook,
    state: Arc<State>,
}
struct Service {
    cache: Mutex<Vec<Slot>>,
    sender: SyncSender<Command>,
    next: AtomicU64,
    alive: Arc<AtomicBool>,
}
impl Service {
    fn new() -> Result<Self, ServiceError> {
        let (sender, receiver) = mpsc::sync_channel(QUEUE_CAPACITY);
        let alive = Arc::new(AtomicBool::new(true));
        let worker_alive = alive.clone();
        std::thread::Builder::new()
            .name("youtube-resume-io".into())
            .stack_size(1024 * 1024)
            .spawn(move || {
                struct Life(Arc<AtomicBool>);
                impl Drop for Life {
                    fn drop(&mut self) {
                        self.0.store(false, Ordering::Release)
                    }
                }
                let _life = Life(worker_alive);
                let mut books = BTreeMap::<u64, Store>::new();
                while let Ok(first) = receiver.recv() {
                    let mut batch = vec![first];
                    batch.extend(receiver.try_iter().take(QUEUE_CAPACITY - 1));
                    let mut dirty = std::collections::BTreeSet::new();
                    let mut barriers = Vec::new();
                    for command in batch {
                        match command {
                            Command::Install { id, book, state } => {
                                books.insert(id, Store { book, state });
                            }
                            Command::Record {
                                id,
                                video,
                                position,
                                duration,
                                when,
                            } => {
                                if let Some(store) = books.get_mut(&id) {
                                    if store.book.record(&video, position, duration, when).is_ok() {
                                        dirty.insert(id);
                                    } else {
                                        store.state.error.store(true, Ordering::Release);
                                    }
                                }
                            }
                            Command::Barrier(reply) => barriers.push(reply),
                        }
                    }
                    for id in dirty {
                        let store = books.get(&id).unwrap();
                        let points = Arc::new(store.book.points());
                        let published = if let Ok(mut snapshot) = store.state.snapshot.lock() {
                            *snapshot = points;
                            true
                        } else {
                            false
                        };
                        // Neither snapshot nor global-cache mutex is held during serialization/fsync.
                        let saved = store.book.save().is_ok();
                        store
                            .state
                            .error
                            .store(!published || !saved, Ordering::Release);
                    }
                    let status = if books
                        .values()
                        .any(|store| store.state.error.load(Ordering::Acquire))
                    {
                        Err(ServiceError::Storage)
                    } else {
                        Ok(())
                    };
                    for barrier in barriers {
                        let _ = barrier.send(status);
                    }
                }
            })
            .map_err(|_| ServiceError::Unavailable)?;
        Ok(Self {
            cache: Mutex::new(Vec::new()),
            sender,
            next: AtomicU64::new(1),
            alive,
        })
    }
    fn try_lookup(
        &self,
        scope: &ResumeScope,
        root: Option<&Path>,
    ) -> Result<Option<Handle>, ServiceError> {
        if !self.alive.load(Ordering::Acquire) {
            return Err(ServiceError::Unavailable);
        }
        let cache = self.cache.try_lock().map_err(lock_error)?;
        Ok(cache
            .iter()
            .find(|slot| slot.scope == *scope && slot.root.as_deref() == root)
            .and_then(|slot| match &slot.state {
                SlotState::Ready(handle) => Some(handle.clone()),
                SlotState::Pending => None,
            }))
    }
    fn remove_pending(&self, id: u64) {
        if let Ok(mut cache) = self.cache.lock() {
            cache.retain(|slot| slot.id != id);
        }
    }
    fn prepare(&self, scope: ResumeScope, root: Option<PathBuf>) -> Result<Handle, ServiceError> {
        if !self.alive.load(Ordering::Acquire) {
            return Err(ServiceError::Unavailable);
        }
        let id = {
            let mut cache = self.cache.try_lock().map_err(lock_error)?;
            if let Some(slot) = cache
                .iter()
                .find(|slot| slot.scope == scope && slot.root == root)
            {
                return match &slot.state {
                    SlotState::Ready(handle) => Ok(handle.clone()),
                    SlotState::Pending => Err(ServiceError::Busy),
                };
            }
            if root.is_some()
                && cache
                    .iter()
                    .any(|slot| slot.root == root && slot.scope != scope)
            {
                return Err(ServiceError::Storage);
            }
            if cache.len() >= MAX_CONTEXTS {
                return Err(ServiceError::Full);
            }
            let id = self
                .next
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1))
                .map_err(|_| ServiceError::Full)?;
            cache.push(Slot {
                id,
                scope: scope.clone(),
                root: root.clone(),
                state: SlotState::Pending,
            });
            id
        };
        // ALL load/JSON copying occurs outside the cache lock, on this worker caller.
        let book = match ResumeBook::load(scope, root.as_deref()) {
            Ok(book) => book,
            Err(_) => {
                self.remove_pending(id);
                return Err(ServiceError::Storage);
            }
        };
        let state = Arc::new(State {
            id,
            snapshot: Mutex::new(Arc::new(book.points())),
            error: AtomicBool::new(false),
            alive: self.alive.clone(),
        });
        let handle = Handle {
            state: state.clone(),
            sender: self.sender.clone(),
        };
        if let Err(error) = self.sender.try_send(Command::Install { id, book, state }) {
            self.remove_pending(id);
            return Err(send_error(error));
        }
        let mut cache = self.cache.lock().map_err(|_| ServiceError::Unavailable)?;
        let slot = cache
            .iter_mut()
            .find(|slot| slot.id == id)
            .ok_or(ServiceError::Unavailable)?;
        slot.state = SlotState::Ready(handle.clone());
        Ok(handle)
    }
    fn flush(&self, timeout: Duration) -> Result<(), ServiceError> {
        let timeout = timeout.min(Duration::from_secs(30));
        if !self.alive.load(Ordering::Acquire) {
            return Err(ServiceError::Unavailable);
        }
        let (tx, rx) = mpsc::sync_channel(1);
        self.sender
            .try_send(Command::Barrier(tx))
            .map_err(send_error)?;
        rx.recv_timeout(timeout).map_err(|error| match error {
            mpsc::RecvTimeoutError::Timeout => ServiceError::Timeout,
            mpsc::RecvTimeoutError::Disconnected => ServiceError::Unavailable,
        })?
    }
}
fn lock_error<T>(error: std::sync::TryLockError<T>) -> ServiceError {
    match error {
        std::sync::TryLockError::WouldBlock => ServiceError::Busy,
        std::sync::TryLockError::Poisoned(_) => ServiceError::Unavailable,
    }
}
fn send_error<T>(error: TrySendError<T>) -> ServiceError {
    match error {
        TrySendError::Full(_) => ServiceError::Busy,
        TrySendError::Disconnected(_) => ServiceError::Unavailable,
    }
}
fn global() -> Result<&'static Service, ServiceError> {
    static SERVICE: OnceLock<Result<Service, ServiceError>> = OnceLock::new();
    SERVICE.get_or_init(Service::new).as_ref().map_err(|e| *e)
}
// Separate accessor avoids initializing an IO thread from lookup/flush UI paths.
// This OnceLock contains only the reference established by prepare().
static PREPARED: OnceLock<&'static Service> = OnceLock::new();
/// Extraction/IO worker ONLY. Root must be a stable parent-derived exact context
/// path; use None for private mode. No eviction, so active profiles are retained.
pub fn prepare(scope: ResumeScope, root: Option<PathBuf>) -> Result<Handle, ServiceError> {
    let service = global()?;
    let _ = PREPARED.set(service);
    service.prepare(scope, root)
}
/// Nonblocking/no filesystem/no initialization. None means not prepared/busy;
/// use try_lookup if the native caller needs the explicit reason.
pub fn lookup(scope: &ResumeScope, root: Option<&Path>) -> Option<Handle> {
    try_lookup(scope, root).ok().flatten()
}
pub fn try_lookup(
    scope: &ResumeScope,
    root: Option<&Path>,
) -> Result<Option<Handle>, ServiceError> {
    match PREPARED.get() {
        Some(service) => service.try_lookup(scope, root),
        None => Ok(None),
    }
}
pub fn position(handle: &Handle, video: &VideoId) -> Result<Option<ResumePoint>, ServiceError> {
    let snapshot = handle
        .state
        .snapshot
        .try_lock()
        .map_err(lock_error)?
        .clone();
    Ok(snapshot.iter().find(|p| &p.video == video).cloned())
}
/// Bounded enqueue ONLY; success is not an IO/persistence acknowledgement.
pub fn record(
    handle: &Handle,
    video: &VideoId,
    position: f64,
    duration: Option<f64>,
    when: u64,
) -> Result<(), ServiceError> {
    if !position.is_finite()
        || !(0.0..=MAX_SECONDS).contains(&position)
        || duration.is_some_and(|d| !d.is_finite() || d <= 0.0 || d > MAX_SECONDS || position > d)
    {
        return Err(ServiceError::InvalidPosition);
    }
    if !handle.state.alive.load(Ordering::Acquire) {
        return Err(ServiceError::Unavailable);
    }
    handle
        .sender
        .try_send(Command::Record {
            id: handle.state.id,
            video: video.clone(),
            position,
            duration,
            when,
        })
        .map_err(send_error)
}
pub fn storage_error(handle: &Handle) -> bool {
    handle.state.error.load(Ordering::Acquire) || !handle.state.alive.load(Ordering::Acquire)
}
/// Shutdown/worker ONLY. Never call this from a normal CEF UI event handler.
pub fn flush(timeout: Duration) -> Result<(), ServiceError> {
    match PREPARED.get() {
        Some(service) => service.flush(timeout),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand_core::{OsRng, RngCore};
    fn scope(name: &str) -> ResumeScope {
        ResumeScope::new(name, &"a".repeat(64)).unwrap()
    }
    fn video() -> VideoId {
        VideoId::parse("aB3_dE-9xY0").unwrap()
    }
    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            let mut nonce = [0u8; 16];
            OsRng.fill_bytes(&mut nonce);
            let path = std::env::temp_dir().join(format!(
                "nexusbro-resume-service-{:x}",
                u128::from_le_bytes(nonce)
            ));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    #[test]
    fn actual_worker_flush_persists_and_restart_reads_exact_position() {
        let temp = Temp::new();
        let service = Service::new().unwrap();
        let handle = service.prepare(scope("a"), Some(temp.0.clone())).unwrap();
        record(&handle, &video(), 42.5, Some(100.0), 12).unwrap();
        service.flush(Duration::from_secs(5)).unwrap();
        assert!(!storage_error(&handle));
        assert_eq!(
            position(&handle, &video())
                .unwrap()
                .unwrap()
                .position_seconds,
            42.5
        );
        let mut restored = ResumeBook::load(scope("a"), Some(&temp.0)).unwrap();
        assert_eq!(restored.position(&video()).unwrap().position_seconds, 42.5);
    }
    #[test]
    fn ram_only_and_disk_for_same_scope_never_share_or_fallback() {
        let temp = Temp::new();
        let service = Service::new().unwrap();
        let ram = service.prepare(scope("a"), None).unwrap();
        record(&ram, &video(), 10.0, None, 1).unwrap();
        service.flush(Duration::from_secs(5)).unwrap();
        assert_eq!(std::fs::read_dir(&temp.0).unwrap().count(), 0);
        let disk = service.prepare(scope("a"), Some(temp.0.clone())).unwrap();
        assert!(position(&disk, &video()).unwrap().is_none());
        record(&disk, &video(), 20.0, None, 2).unwrap();
        service.flush(Duration::from_secs(5)).unwrap();
        assert_eq!(
            position(&ram, &video()).unwrap().unwrap().position_seconds,
            10.0
        );
        let other = service.prepare(scope("b"), None).unwrap();
        assert!(position(&other, &video()).unwrap().is_none());
    }
    #[test]
    fn queue_bound_is_nonblocking_and_acceptance_is_not_a_snapshot_update() {
        let (tx, _rx) = mpsc::sync_channel(1);
        let handle = Handle {
            state: Arc::new(State {
                id: 1,
                snapshot: Mutex::new(Arc::new(Vec::new())),
                error: AtomicBool::new(false),
                alive: Arc::new(AtomicBool::new(true)),
            }),
            sender: tx,
        };
        record(&handle, &video(), 1.0, None, 1).unwrap();
        assert!(position(&handle, &video()).unwrap().is_none());
        assert_eq!(
            record(&handle, &video(), 2.0, None, 2),
            Err(ServiceError::Busy)
        );
        let guard = handle.state.snapshot.lock().unwrap();
        assert!(matches!(
            position(&handle, &video()),
            Err(ServiceError::Busy)
        ));
        drop(guard);
    }
    #[test]
    fn full_cache_never_evicts_a_live_handle_and_lookup_never_waits() {
        let service = Service::new().unwrap();
        let first = service.prepare(scope("s0"), None).unwrap();
        for n in 1..MAX_CONTEXTS {
            service.prepare(scope(&format!("s{n}")), None).unwrap();
        }
        assert!(matches!(
            service.prepare(scope("overflow"), None),
            Err(ServiceError::Full)
        ));
        assert!(Arc::ptr_eq(
            &first.state,
            &service
                .try_lookup(&scope("s0"), None)
                .unwrap()
                .unwrap()
                .state
        ));
        let guard = service.cache.lock().unwrap();
        assert!(matches!(
            service.try_lookup(&scope("s0"), None),
            Err(ServiceError::Busy)
        ));
        drop(guard);
    }
    #[test]
    fn corrupt_load_releases_reservation_and_failed_write_is_reported() {
        let temp = Temp::new();
        std::fs::write(temp.0.join("youtube-resume.json"), b"broken").unwrap();
        let service = Service::new().unwrap();
        assert!(matches!(
            service.prepare(scope("a"), Some(temp.0.clone())),
            Err(ServiceError::Storage)
        ));
        assert!(service.cache.lock().unwrap().is_empty());
        std::fs::remove_file(temp.0.join("youtube-resume.json")).unwrap();
        let handle = service.prepare(scope("a"), Some(temp.0.clone())).unwrap();
        std::fs::create_dir(temp.0.join("youtube-resume.json")).unwrap();
        record(&handle, &video(), 10.0, None, 1).unwrap();
        assert_eq!(
            service.flush(Duration::from_secs(5)),
            Err(ServiceError::Storage)
        );
        assert!(storage_error(&handle));
        assert_eq!(
            position(&handle, &video())
                .unwrap()
                .unwrap()
                .position_seconds,
            10.0
        );
    }
    #[test]
    fn one_disk_root_cannot_be_bound_to_two_distinct_scopes() {
        let temp = Temp::new();
        let service = Service::new().unwrap();
        let _first = service.prepare(scope("a"), Some(temp.0.clone())).unwrap();
        assert!(matches!(
            service.prepare(scope("b"), Some(temp.0.clone())),
            Err(ServiceError::Storage)
        ));
        assert!(service.prepare(scope("b"), None).is_ok());
    }
}
