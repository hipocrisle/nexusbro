//! Per-CEF-process extraction coordinator. Call ONLY inside the contained CEF
//! engine: yt-dlp/Deno inherit its Windows Job or Linux seccomp process group.
//! This module reaps via sidecar::extract; it never claims all descendants exit.
//! A queued Ready remains active until native consumption calls finish(), even
//! when cancelled. ReapFailed/panic latches protection until CEF process exit.
use crate::{
    tab_routes::NativeRoute,
    youtube_light::VideoId,
    youtube_light_extract::{self, ExtractedVideo},
    youtube_light_sidecar::{self as sidecar, Components},
};
use std::{
    collections::BTreeMap,
    fmt,
    panic::{catch_unwind, AssertUnwindSafe},
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex, OnceLock,
    },
};
use tokio::sync::watch;

pub const MAX_JOBS: usize = 4;
const STACK_BYTES: usize = 1024 * 1024;
const INVALID_INPUT: &str = "Некорректный запрос извлечения видео";
const BUSY: &str = "Извлечение уже выполняется или достигнут лимит задач";
const UNAVAILABLE: &str = "Не удалось запустить извлечение видео";
const CANCELLED: &str = "Извлечение отменено";
const CLEANUP_UNKNOWN:&str="Завершение помощника не подтверждено; задача остаётся защищённой до закрытия процесса браузера";
static NEXT_OPERATION: AtomicU64 = AtomicU64::new(1);

pub struct Ready {
    pub browser: i32,
    pub operation: u64,
    pub route_generation: u64,
    /// Canonical watch URL; tracking, timestamp and playlist data never survive.
    pub source_url: String,
    pub result: Result<ExtractedVideo, &'static str>,
}
impl fmt::Debug for Ready {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Ready")
            .field("browser", &self.browser)
            .field("operation", &self.operation)
            .field("route_generation", &self.route_generation)
            .field("success", &self.result.is_ok())
            .finish()
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct JobStatus {
    pub operation: u64,
    pub cancelled: bool,
    pub completed: bool,
    pub cleanup_unknown: bool,
}
struct Entry {
    status: JobStatus,
    cancel: watch::Sender<bool>,
}
#[derive(Default)]
struct Manager {
    entries: BTreeMap<i32, Entry>,
}
impl Manager {
    fn reserve(
        &mut self,
        browser: i32,
        operation: u64,
    ) -> Result<watch::Receiver<bool>, &'static str> {
        if browser <= 0 || operation == 0 {
            return Err(INVALID_INPUT);
        }
        if self.entries.contains_key(&browser) || self.entries.len() >= MAX_JOBS {
            return Err(BUSY);
        }
        let (cancel, receiver) = watch::channel(false);
        self.entries.insert(
            browser,
            Entry {
                status: JobStatus {
                    operation,
                    cancelled: false,
                    completed: false,
                    cleanup_unknown: false,
                },
                cancel,
            },
        );
        Ok(receiver)
    }
    fn cancel(&mut self, browser: i32) {
        if let Some(entry) = self.entries.get_mut(&browser) {
            entry.status.cancelled = true;
            entry.cancel.send_replace(true);
        }
    }
    fn cancel_all(&mut self) {
        for entry in self.entries.values_mut() {
            entry.status.cancelled = true;
            entry.cancel.send_replace(true);
        }
    }
    fn is_current(&self, browser: i32, operation: u64) -> bool {
        self.entries.get(&browser).is_some_and(|entry| {
            entry.status.operation == operation
                && !entry.status.cancelled
                && !entry.status.cleanup_unknown
        })
    }
    fn complete(
        &mut self,
        browser: i32,
        operation: u64,
        cleanup_unknown: bool,
    ) -> Option<JobStatus> {
        let entry = self
            .entries
            .get_mut(&browser)
            .filter(|e| e.status.operation == operation)?;
        entry.status.completed = true;
        entry.status.cleanup_unknown |= cleanup_unknown;
        Some(entry.status)
    }
    fn finish(&mut self, browser: i32, operation: u64) -> bool {
        if !self.entries.get(&browser).is_some_and(|e| {
            e.status.operation == operation && e.status.completed && !e.status.cleanup_unknown
        }) {
            return false;
        }
        self.entries.remove(&browser);
        true
    }
    fn spawn_failed(&mut self, browser: i32, operation: u64) {
        // Builder failure guarantees the worker/sidecar never existed.
        self.complete(browser, operation, false);
        self.finish(browser, operation);
    }
}
fn registry() -> &'static Mutex<Manager> {
    static REGISTRY: OnceLock<Mutex<Manager>> = OnceLock::new();
    REGISTRY.get_or_init(Default::default)
}
fn manager() -> std::sync::MutexGuard<'static, Manager> {
    registry().lock().unwrap_or_else(|e| e.into_inner())
}
/// Cancellation signals the worker, but does not assert cleanup or remove it.
pub fn cancel(browser: i32) {
    manager().cancel(browser)
}
pub fn cancel_all() {
    manager().cancel_all()
}
/// Includes completed-but-unconsumed, cancelled and cleanup-unknown jobs.
pub fn active(browser: i32) -> bool {
    manager().entries.contains_key(&browser)
}
/// Includes cleanup latches for Browser IDs that no longer have a live tab.
/// Whole-Super sleep must consult this process-wide guard.
pub fn any_active() -> bool {
    !manager().entries.is_empty()
}
pub fn statuses() -> Vec<(i32, JobStatus)> {
    manager()
        .entries
        .iter()
        .map(|(browser, entry)| (*browser, entry.status))
        .collect()
}
pub fn is_current(browser: i32, operation: u64) -> bool {
    manager().is_current(browser, operation)
}
/// Native Ready handler must call this even when it ignores a stale/cancelled
/// result. It can never remove a newer operation or a still-running worker.
pub fn finish(browser: i32, operation: u64) -> bool {
    manager().finish(browser, operation)
}
/// Redacted read-only diagnostic; no source URL, component path or media data.
pub fn status(browser: i32) -> Option<JobStatus> {
    manager().entries.get(&browser).map(|e| e.status)
}
fn sidecar_error(error: sidecar::ExtractError) -> &'static str {
    use sidecar::ExtractError::*;
    match error {
        InvalidComponents => "Проверенные компоненты извлечения недоступны",
        InvalidRoute => "Путь видео недоступен",
        Cancelled => CANCELLED,
        OutputLimit => "Ответ извлечения превышает лимит",
        TimedOut => "Истекло время извлечения",
        ReapFailed => CLEANUP_UNKNOWN,
        UnsupportedInvocation => "Компоненты лёгкого режима несовместимы",
        HttpForbidden => "Сервер отклонил запрос видео (403)",
        RateLimited => "Сервис ограничил частоту запросов; повторите позже",
        TlsFailure => "Не удалось проверить защищённое соединение с видеосервисом",
        ChallengeRequired => {
            "YouTube потребовал дополнительную проверку. Используйте обычный режим."
        }
        VideoUnavailable => "Видео недоступно для анонимного просмотра",
        HttpBadRequest => "Источник отклонил параметры запроса видео",
        HttpNotFound => "Видео не найдено",
        HttpServerError => "Видеосервис временно недоступен",
        NetworkFailure => "Соединение с видеосервисом прервано",
        RuntimeFailure | ChallengeSolverUnavailable => {
            "Не удалось обработать проверку видеосервиса"
        }
        FormatUnavailable => "Подходящий формат видео недоступен",
        FfmpegUnavailable => "Компонент обработки видео недоступен",
        ExtractionFailure => "Не удалось извлечь публичные данные видео",
        ProcessFailed | EmptyOutput => "Видео недоступно для анонимного извлечения",
        SpawnFailed | ReadFailed | WaitFailed => "Не удалось получить публичные данные видео",
    }
}
/// Rejects an already registered browser; never replaces/cancels it implicitly.
/// components_factory runs on the bounded worker after registration; put
/// manifest/digest verification INSIDE that closure, never before this call.
/// callback returns true ONLY when it queued Ready for native consumption. On
/// false/panic normal completed work is finished automatically. Cleanup-unknown
/// remains latched in all cases; only CEF process death clears that condition.
#[allow(clippy::too_many_arguments)]
pub fn spawn<F, G>(
    browser: i32,
    source_url: String,
    route_generation: u64,
    components_factory: G,
    video: VideoId,
    route: NativeRoute,
    callback: F,
) -> Result<u64, &'static str>
where
    F: FnOnce(Ready) -> bool + Send + 'static,
    G: FnOnce() -> Result<Components, &'static str> + Send + 'static,
{
    let source_url = zeroize::Zeroizing::new(source_url);
    if browser <= 0
        || VideoId::from_url(&source_url).ok().as_ref() != Some(&video)
        || !route.validate()
    {
        return Err(INVALID_INPUT);
    }
    let source_url = video.watch_url();
    let (operation, cancel) = {
        let mut registry = manager();
        if registry.entries.contains_key(&browser) || registry.entries.len() >= MAX_JOBS {
            return Err(BUSY);
        }
        let operation = NEXT_OPERATION
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1))
            .map_err(|_| UNAVAILABLE)?;
        (operation, registry.reserve(browser, operation)?)
    };
    let worker = std::thread::Builder::new()
        .name("youtube-extract".into())
        .stack_size(STACK_BYTES)
        .spawn(move || {
            // If a panic interrupts extraction, no actor can truthfully assert its
            // cleanup. The registry remains latched instead of freeing the slot.
            let work = catch_unwind(AssertUnwindSafe(|| {
                // Expensive component hashing is work, not UI preparation.
                // Registration protects the browser even throughout this phase.
                if *cancel.borrow() {
                    return Err((CANCELLED, false));
                }
                let components = components_factory().map_err(|_| (UNAVAILABLE, false))?;
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|_| (UNAVAILABLE, false))?;
                let bytes = runtime
                    .block_on(sidecar::extract(&components, &video, &route, cancel))
                    .map_err(|error| {
                        (
                            sidecar_error(error),
                            error == sidecar::ExtractError::ReapFailed,
                        )
                    })?;
                youtube_light_extract::parse(&bytes, &video).map_err(|error| {
                    let text = if error == youtube_light_extract::ExtractError::NoSupportedFormats {
                        "Поддерживаемые прямые форматы видео не найдены"
                    } else {
                        "Не удалось безопасно прочитать данные видео"
                    };
                    (text, false)
                })
            }));
            let (result, unknown) = match work {
                Ok(Ok(video)) => (Ok(video), false),
                Ok(Err((error, unknown))) => (Err(error), unknown),
                Err(_) => (Err(CLEANUP_UNKNOWN), true),
            };
            let Some(completion) = manager().complete(browser, operation, unknown) else {
                return;
            };
            let result = if completion.cancelled && !completion.cleanup_unknown {
                Err(CANCELLED)
            } else {
                result
            };
            let ready = Ready {
                browser,
                operation,
                route_generation,
                source_url,
                result,
            };
            if !catch_unwind(AssertUnwindSafe(|| callback(ready))).unwrap_or(false) {
                finish(browser, operation);
            }
        });
    if worker.is_err() {
        manager().spawn_failed(browser, operation);
        return Err(UNAVAILABLE);
    }
    Ok(operation)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn global_bound_and_same_browser_rejection_include_queued_completions() {
        let mut manager = Manager::default();
        let mut receivers = Vec::new();
        for browser in 1..=4 {
            receivers.push(manager.reserve(browser, browser as u64).unwrap());
        }
        assert_eq!(manager.reserve(1, 100).unwrap_err(), BUSY);
        assert_eq!(manager.reserve(5, 5).unwrap_err(), BUSY);
        manager.complete(1, 1, false);
        assert_eq!(manager.reserve(5, 5).unwrap_err(), BUSY);
        assert!(manager.finish(1, 1));
        assert!(manager.reserve(5, 5).is_ok());
        assert_eq!(manager.entries.len(), 4);
    }
    #[test]
    fn cancel_is_observable_and_never_releases_a_running_slot() {
        let mut manager = Manager::default();
        let cancel = manager.reserve(1, 7).unwrap();
        manager.cancel(1);
        assert!(*cancel.borrow());
        assert!(!manager.is_current(1, 7));
        assert!(!manager.finish(1, 7));
        assert_eq!(manager.reserve(1, 8).unwrap_err(), BUSY);
        assert!(manager.complete(1, 7, false).unwrap().cancelled);
        assert!(manager.finish(1, 7));
    }
    #[test]
    fn stale_cancelled_completion_cannot_remove_a_future_operation() {
        let mut manager = Manager::default();
        let _old = manager.reserve(1, 7).unwrap();
        manager.cancel(1);
        manager.complete(1, 7, false);
        assert!(manager.finish(1, 7));
        let _new = manager.reserve(1, 8).unwrap();
        assert!(!manager.finish(1, 7));
        assert!(manager.complete(1, 7, false).is_none());
        assert!(manager.is_current(1, 8));
        assert!(!manager.finish(1, 8));
    }
    #[test]
    fn queued_result_can_be_invalidated_without_losing_cleanup_ack() {
        let mut manager = Manager::default();
        let _cancel = manager.reserve(1, 1).unwrap();
        manager.complete(1, 1, false);
        assert!(manager.is_current(1, 1));
        manager.cancel(1);
        assert!(!manager.is_current(1, 1));
        assert!(manager.finish(1, 1));
    }
    #[test]
    fn cleanup_unknown_is_a_permanent_protection_latch() {
        let mut manager = Manager::default();
        let _cancel = manager.reserve(1, 1).unwrap();
        manager.complete(1, 1, true);
        assert!(!manager.finish(1, 1));
        manager.cancel_all();
        assert!(manager.entries.contains_key(&1));
        assert!(!manager.is_current(1, 1));
        assert!(!manager.finish(1, 1));
        assert_eq!(manager.reserve(1, 2).unwrap_err(), BUSY);
        // Later duplicate normal completion cannot downgrade unproven cleanup.
        assert!(manager.complete(1, 1, false).unwrap().cleanup_unknown);
        assert!(!manager.finish(1, 1));
    }
    #[test]
    fn callback_delivery_failure_and_thread_creation_failure_release_only_matching_safe_work() {
        let mut manager = Manager::default();
        let _first = manager.reserve(1, 1).unwrap();
        manager.spawn_failed(1, 999);
        assert!(manager.entries.contains_key(&1));
        manager.spawn_failed(1, 1);
        assert!(!manager.entries.contains_key(&1));
        let _next = manager.reserve(1, 2).unwrap();
        manager.complete(1, 2, false);
        assert!(manager.finish(1, 2));
    }
    #[test]
    fn cancel_all_signals_each_entry_and_keeps_capacity_reserved() {
        let mut manager = Manager::default();
        let one = manager.reserve(1, 1).unwrap();
        let two = manager.reserve(2, 2).unwrap();
        manager.cancel_all();
        assert!(*one.borrow());
        assert!(*two.borrow());
        assert_eq!(manager.entries.len(), 2);
        assert!(!manager.is_current(1, 1));
        assert!(!manager.is_current(2, 2));
    }
}
