//! Normal application shutdown state. No CEF call is made under this RefCell.
use crate::app_shutdown_protocol::Request;
use std::{
    cell::RefCell,
    sync::{
        atomic::{AtomicU8, Ordering},
        Arc,
    },
};
#[derive(Default)]
struct State {
    request: Option<Request>,
    conflict: bool,
    drained: bool,
    flush: Option<Arc<AtomicU8>>,
}
impl State {
    fn accept(&mut self, request: &Request) -> bool {
        match &self.request {
            Some(old) if old == request => true,
            Some(_) => {
                self.conflict = true;
                false
            }
            None => {
                self.request = Some(request.clone());
                true
            }
        }
    }
}
thread_local! {static STATE:RefCell<State>=RefCell::new(State::default());}
fn generation() -> Option<u64> {
    let mut values = std::env::args()
        .filter_map(|s| s.strip_prefix("--nb-engine-generation=").map(str::to_owned));
    let value = values.next()?.parse().ok()?;
    if values.next().is_some() {
        return None;
    }
    Some(value)
}
pub fn begin(request: &Request) -> bool {
    if !request.valid() || generation() != Some(request.engine) {
        return false;
    }
    STATE.with(|slot| slot.borrow_mut().accept(request))
}
pub fn active() -> bool {
    STATE.with(|s| s.borrow().request.is_some())
}
pub fn ready() -> bool {
    STATE.with(|slot| {
        let s = slot.borrow();
        s.request.is_some()
            && s.drained
            && !s.conflict
            && s.flush
                .as_ref()
                .is_some_and(|f| f.load(Ordering::Acquire) == 1)
    })
}
/// Called only after actual browser/devtools callbacks and process-helper reap.
/// Disk barrier runs off CEF UI; timeout/failure never yields a clean ACK.
pub fn poll_drained(drained: bool) {
    let start = STATE.with(|slot| {
        let mut s = slot.borrow_mut();
        if !drained || s.request.is_none() || s.conflict {
            return None;
        }
        s.drained = true;
        if s.flush.is_some() {
            return None;
        }
        let status = Arc::new(AtomicU8::new(0));
        s.flush = Some(status.clone());
        Some(status)
    });
    if let Some(status) = start {
        let failed = status.clone();
        if std::thread::Builder::new()
            .name("app-shutdown-flush".into())
            .spawn(move || {
                let ok =
                    crate::youtube_light_resume_service::flush(std::time::Duration::from_secs(3))
                        .is_ok();
                status.store(if ok { 1 } else { 2 }, Ordering::Release);
            })
            .is_err()
        {
            failed.store(2, Ordering::Release);
        }
    }
}
/// Take only after Rabota/CEF registrations have been dropped. Caller still
/// must call CefShutdown before constructing/emitting the protocol ACK.
pub fn take_completed() -> Option<Request> {
    if !ready() {
        return None;
    }
    STATE.with(|s| s.borrow_mut().request.take())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn request(byte: &str) -> Request {
        Request {
            nonce: byte.repeat(16),
            engine: 4,
        }
    }
    #[test]
    fn duplicate_is_idempotent_but_conflicting_request_poisoned() {
        let mut state = State::default();
        let a = request("ab");
        assert!(state.accept(&a));
        assert!(state.accept(&a));
        assert!(!state.conflict);
        assert!(!state.accept(&request("cd")));
        assert!(state.conflict);
        assert_eq!(state.request.as_ref().unwrap().nonce, a.nonce);
    }
    #[test]
    fn no_clean_request_before_drain_and_successful_flush() {
        STATE.with(|s| *s.borrow_mut() = State::default());
        STATE.with(|s| {
            s.borrow_mut().accept(&request("ab"));
        });
        assert!(!ready());
        assert!(take_completed().is_none());
        STATE.with(|s| {
            let mut s = s.borrow_mut();
            s.drained = true;
            s.flush = Some(Arc::new(AtomicU8::new(2)));
        });
        assert!(!ready());
        assert!(take_completed().is_none());
        STATE.with(|s| {
            s.borrow()
                .flush
                .as_ref()
                .unwrap()
                .store(1, Ordering::Release)
        });
        assert!(ready());
        assert!(take_completed().is_some());
        assert!(take_completed().is_none());
        STATE.with(|s| *s.borrow_mut() = State::default());
    }
}
