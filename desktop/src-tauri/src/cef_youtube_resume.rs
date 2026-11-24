//! CEF UI resume binding. No filesystem calls, worker waits, JSON persistence,
//! timestamps from page messages, or timers. Maintenance reads only an admitted
//! exact-document native Probe; the resume service owns all persistence IO.
use crate::{
    cef_youtube_probe::Probe,
    youtube_light::VideoId,
    youtube_light_resume::{ResumeScope, MAX_SECONDS},
    youtube_light_resume_service::{self as service, Handle},
};
use cef::*;
use std::{
    cell::RefCell,
    collections::HashMap,
    path::PathBuf,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
const FAILED: &str = "Позиция видео сейчас недоступна";
const MAX_BINDINGS: usize = 128;
const MANUAL_THROTTLE: Duration = Duration::from_millis(500);
const PLAY_INTERVAL: Duration = Duration::from_secs(5);
#[derive(Default)]
struct Schedule {
    ready: bool,
    dirty: bool,
    playing: bool,
    last_probe: Option<Instant>,
    due: Option<Instant>,
}
impl Schedule {
    fn notify(&mut self, ready: bool, now: Instant) {
        self.ready |= ready;
        self.dirty = true;
        if self.ready {
            self.due = Some(
                self.last_probe
                    .map_or(now, |last| (last + MANUAL_THROTTLE).max(now)),
            );
        }
    }
    fn startable(&self, now: Instant, pending: bool) -> bool {
        self.ready && !pending && self.due.is_some_and(|due| due <= now)
    }
    fn started(&mut self, now: Instant) {
        self.last_probe = Some(now);
        self.dirty = false;
        self.due = None;
    }
    fn observed(&mut self, playing: bool, now: Instant) {
        self.playing = playing;
        self.due = if self.dirty {
            Some(
                self.last_probe
                    .map_or(now, |last| (last + MANUAL_THROTTLE).max(now)),
            )
        } else if playing {
            Some(
                self.last_probe
                    .map_or(now + PLAY_INTERVAL, |last| (last + PLAY_INTERVAL).max(now)),
            )
        } else {
            None
        };
    }
}
struct Binding {
    handle: Handle,
    document: String,
    video: VideoId,
    schedule: Schedule,
    probe: Option<Probe>,
    pending_record: Option<PendingRecord>,
}
thread_local! {static BINDINGS:RefCell<HashMap<i32,Binding>>=RefCell::new(HashMap::new());}
/// Cheap native setup; no canonicalize/stat/read. Ordinary cache paths MUST be
/// nonempty actual RequestContext.cache_path; private always irrevocably None.
pub fn setup(
    browser: &Browser,
    route_key: &str,
    private: bool,
) -> Result<(ResumeScope, Option<PathBuf>), &'static str> {
    if currently_on(ThreadId::UI) == 0 || browser.is_valid() == 0 {
        return Err(FAILED);
    }
    let mut keys =
        std::env::args().filter_map(|a| a.strip_prefix("--nb-profile-key=").map(str::to_owned));
    let key = keys.next().ok_or(FAILED)?;
    if keys.next().is_some()
        || key.len() != 64
        || !key
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(FAILED);
    }
    let scope = ResumeScope::new(&key, route_key).map_err(|_| FAILED)?;
    let context = browser
        .host()
        .and_then(|h| h.request_context())
        .ok_or(FAILED)?;
    if private || crate::privatnost::process_privatnyj() {
        return Ok((scope, None));
    }
    let path = CefString::from(&context.cache_path()).to_string();
    if path.is_empty() {
        return Err(FAILED);
    }
    let path = PathBuf::from(path);
    if !path.is_absolute() {
        return Err(FAILED);
    }
    Ok((scope, Some(path)))
}
fn logical_matches(browser: i32, document: &str, video: &VideoId) -> bool {
    crate::cef_youtube::logical_url(browser, document)
        .and_then(|url| VideoId::from_url(&url).ok())
        .as_ref()
        == Some(video)
}
fn actual_matches(browser: &Browser, binding: &Binding) -> bool {
    browser.is_valid() != 0
        && browser.main_frame().is_some_and(|frame| {
            frame.is_main() != 0 && CefString::from(&frame.url()).to_string() == binding.document
        })
        && logical_matches(browser.identifier(), &binding.document, &binding.video)
}
/// Call after registering the internal document's logical URL but before its
/// navigation/configuration. Initial real frame may still be the old YouTube
/// page: no probe/record is admitted until READY on the exact new document.
pub fn attach(
    browser: &Browser,
    document: String,
    scope: ResumeScope,
    root: Option<PathBuf>,
    video: VideoId,
) -> Result<f64, &'static str> {
    if currently_on(ThreadId::UI) == 0
        || browser.is_valid() == 0
        || document.len() > 2048
        || !logical_matches(browser.identifier(), &document, &video)
    {
        return Err(FAILED);
    }
    let handle = service::try_lookup(&scope, root.as_deref())
        .map_err(|_| FAILED)?
        .ok_or(FAILED)?;
    let position = service::position(&handle, &video)
        .map_err(|_| FAILED)?
        .map_or(0.0, |p| p.position_seconds);
    BINDINGS.with(|slot| {
        let mut bindings = slot.try_borrow_mut().map_err(|_| FAILED)?;
        let id = browser.identifier();
        if !bindings.contains_key(&id) && bindings.len() >= MAX_BINDINGS {
            return Err(FAILED);
        }
        bindings.insert(
            id,
            Binding {
                handle,
                document,
                video,
                schedule: Schedule::default(),
                probe: None,
                pending_record: None,
            },
        );
        Ok(position)
    })
}
/// ready=true only for the constant native-routed READY marker. No coordinates,
/// duration or position supplied by a console/page message are ever trusted.
pub fn on_tick(browser: &Browser, ready: bool) {
    if currently_on(ThreadId::UI) == 0 {
        return;
    }
    BINDINGS.with(|slot| {
        let Ok(mut bindings) = slot.try_borrow_mut() else {
            return;
        };
        let id = browser.identifier();
        let Some(binding) = bindings.get_mut(&id) else {
            return;
        };
        if !actual_matches(browser, binding) {
            return;
        }
        binding.schedule.notify(ready, Instant::now());
    });
}
#[derive(Clone, Copy)]
struct Stats {
    position: f64,
    duration: f64,
    playing: bool,
}
struct PendingRecord {
    sample: Stats,
    when: u64,
    attempts: u8,
    due: Instant,
}
impl PendingRecord {
    fn retry(&mut self, now: Instant) -> bool {
        self.attempts += 1;
        if self.attempts >= 3 {
            return false;
        }
        self.due = now + MANUAL_THROTTLE;
        true
    }
}
fn stats(value: &serde_json::Value) -> Option<Stats> {
    if value["configured"] != true || value["disposed"] != false {
        return None;
    }
    // The independent video clock is diagnostic; resume follows audible media.
    let position = value[if value["separate"] == true { "audio_position" } else { "position" }].as_f64()?;
    let duration = value["duration"].as_f64()?;
    let playing = value["playing"].as_bool()?;
    if !position.is_finite()
        || !duration.is_finite()
        || position < 0.0
        || duration <= 0.0
        || duration > MAX_SECONDS
        || position > duration
    {
        return None;
    }
    Some(Stats {
        position,
        duration,
        playing,
    })
}
/// Call from existing native maintenance; never creates a timer. A paused clean
/// player has NO scheduled probe. Dirty ticks coalesce behind the 500ms throttle.
pub fn maintenance(browsers: &HashMap<u64, Browser>) {
    if currently_on(ThreadId::UI) == 0 {
        return;
    }
    BINDINGS.with(|slot| {
        let Ok(mut bindings) = slot.try_borrow_mut() else {
            return;
        };
        let now = Instant::now();
        let mut remove = Vec::new();
        for browser in browsers.values() {
            let id = browser.identifier();
            let Some(binding) = bindings.get_mut(&id) else {
                continue;
            };
            if !actual_matches(browser, binding) {
                if binding.schedule.ready
                    || browser.is_valid() == 0
                    || !logical_matches(id, &binding.document, &binding.video)
                {
                    remove.push(id);
                }
                continue;
            }
            if binding
                .pending_record
                .as_ref()
                .is_some_and(|pending| pending.due <= now)
            {
                let mut pending = binding.pending_record.take().unwrap();
                if matches!(
                    service::record(
                        &binding.handle,
                        &binding.video,
                        pending.sample.position,
                        Some(pending.sample.duration),
                        pending.when
                    ),
                    Err(service::ServiceError::Busy)
                ) && pending.retry(now)
                {
                    binding.pending_record = Some(pending);
                }
            }
            if let Some(reply) = binding.probe.as_mut().and_then(Probe::poll) {
                binding.probe = None;
                let value = reply.ok();
                if value
                    .as_ref()
                    .is_some_and(|value| value["disposed"] == true)
                {
                    remove.push(id);
                    continue;
                }
                match value.and_then(|value| stats(&value)) {
                    Some(sample) => {
                        binding.schedule.observed(sample.playing, now);
                        let epoch = SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .ok()
                            .map(|d| d.as_secs());
                        // Queue retry uses only this bounded RAM sample, not
                        // extra CDP polling while paused. No saved receipt is
                        // asserted on enqueue; the IO service reports failures.
                        binding.pending_record = None;
                        if binding.schedule.ready {
                            if let Some(epoch) = epoch {
                                if matches!(
                                    service::record(
                                        &binding.handle,
                                        &binding.video,
                                        sample.position,
                                        Some(sample.duration),
                                        epoch
                                    ),
                                    Err(service::ServiceError::Busy)
                                ) {
                                    binding.pending_record = Some(PendingRecord {
                                        sample,
                                        when: epoch,
                                        attempts: 1,
                                        due: now + MANUAL_THROTTLE,
                                    });
                                }
                            }
                        }
                    }
                    None => binding.schedule.observed(binding.schedule.playing, now),
                }
            }
            if binding.schedule.startable(now, binding.probe.is_some()) {
                binding.schedule.started(now);
                match Probe::start(browser) {
                    Ok(probe) => binding.probe = Some(probe),
                    Err(_) => binding.schedule.observed(binding.schedule.playing, now),
                }
            }
        }
        for id in remove {
            bindings.remove(&id);
        }
    });
}
pub fn closed(browser: i32) {
    BINDINGS.with(|slot| {
        if let Ok(mut bindings) = slot.try_borrow_mut() {
            bindings.remove(&browser);
        }
    })
}
/// Drop observers before CEF shutdown/lock teardown. No flush or IO waits here.
pub fn clear() {
    BINDINGS.with(|slot| {
        if let Ok(mut bindings) = slot.try_borrow_mut() {
            bindings.clear();
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn no_probe_until_ready_and_manual_ticks_coalesce_at_500ms() {
        let now = Instant::now();
        let mut s = Schedule::default();
        s.notify(false, now);
        assert!(!s.startable(now, false));
        s.notify(true, now);
        assert!(s.startable(now, false));
        s.started(now);
        s.notify(false, now + Duration::from_millis(10));
        s.notify(false, now + Duration::from_millis(20));
        assert!(!s.startable(now + Duration::from_millis(499), false));
        assert!(s.startable(now + MANUAL_THROTTLE, false));
        assert!(!s.startable(now + MANUAL_THROTTLE, true));
    }
    #[test]
    fn paused_and_idle_players_have_no_regular_probes() {
        let now = Instant::now();
        let mut s = Schedule::default();
        s.notify(true, now);
        s.started(now);
        s.observed(false, now);
        assert!(s.due.is_none());
        assert!(!s.startable(now + Duration::from_secs(600), false));
    }
    #[test]
    fn playing_probes_every_five_seconds_and_pause_stops_periodic_work() {
        let now = Instant::now();
        let mut s = Schedule::default();
        s.notify(true, now);
        s.started(now);
        s.observed(true, now);
        assert!(!s.startable(now + Duration::from_secs(4), false));
        assert!(s.startable(now + PLAY_INTERVAL, false));
        s.started(now + PLAY_INTERVAL);
        s.observed(false, now + PLAY_INTERVAL);
        assert!(s.due.is_none());
    }
    #[test]
    fn tick_arriving_while_probe_pending_survives_observation() {
        let now = Instant::now();
        let mut s = Schedule::default();
        s.notify(true, now);
        s.started(now);
        s.notify(false, now + Duration::from_millis(100));
        s.observed(false, now + Duration::from_millis(150));
        assert!(s.dirty);
        assert!(s.startable(now + MANUAL_THROTTLE, false));
    }
    #[test]
    fn numeric_probe_data_must_be_ready_positive_and_not_disposed() {
        let valid = serde_json::json!({"configured":true,"disposed":false,"playing":false,"position":10.5,"duration":100});
        assert!(stats(&valid).is_some());
        for (key, value) in [
            ("configured", serde_json::json!(false)),
            ("disposed", serde_json::json!(true)),
            ("duration", serde_json::json!(0)),
            ("position", serde_json::json!(101)),
        ] {
            let mut bad = valid.clone();
            bad[key] = value;
            assert!(stats(&bad).is_none());
        }
    }
    #[test]
    fn split_tracks_resume_uses_audio_without_hiding_the_independent_video_clock() {
        let mut value = serde_json::json!({"configured":true,"disposed":false,"playing":false,"separate":true,"position":10.5,"audio_position":11.25,"duration":100});
        assert_eq!(stats(&value).unwrap().position, 11.25);
        value["audio_position"] = serde_json::Value::Null;
        assert!(stats(&value).is_none());
        value["separate"] = serde_json::json!(false);
        assert_eq!(stats(&value).unwrap().position, 10.5);
    }
    #[test]
    fn queued_record_retry_is_bounded_and_never_schedules_a_paused_probe() {
        let now = Instant::now();
        let mut pending = PendingRecord {
            sample: Stats {
                position: 10.0,
                duration: 100.0,
                playing: false,
            },
            when: 1,
            attempts: 1,
            due: now,
        };
        assert!(pending.retry(now));
        assert_eq!(pending.due, now + MANUAL_THROTTLE);
        assert!(!pending.retry(now + MANUAL_THROTTLE));
        let mut schedule = Schedule::default();
        schedule.ready = true;
        schedule.observed(false, now);
        assert!(schedule.due.is_none());
    }
}
