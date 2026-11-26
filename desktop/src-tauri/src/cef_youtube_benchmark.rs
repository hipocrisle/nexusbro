//! Bounded UI-thread actor for CI-only native benchmark observations.
use crate::{cef_vkladki::Sobytie, youtube_light::VideoId};
use cef::*;
use std::{cell::RefCell, collections::HashMap, sync::mpsc::Sender};

#[derive(Default)]
struct Reservations {
    next: u64,
    current: HashMap<i32, u64>,
}
impl Reservations {
    fn reserve(&mut self, browser: i32) -> Result<u64, &'static str> {
        if self.current.contains_key(&browser) {
            return Err("benchmark_browser_busy");
        }
        if self.current.len() >= 4 {
            return Err("benchmark_capacity");
        }
        let token = self
            .next
            .checked_add(1)
            .ok_or("benchmark_token_exhausted")?;
        self.next = token;
        self.current.insert(browser, token);
        Ok(token)
    }
    fn matches(&self, browser: i32, token: u64) -> bool {
        self.current.get(&browser) == Some(&token)
    }
    fn retire(&mut self, browser: i32, token: u64) -> bool {
        if !self.matches(browser, token) {
            return false;
        }
        self.current.remove(&browser);
        true
    }
}
enum Probe {
    Reference(crate::cef_youtube_reference_probe::Probe),
    Decode(crate::cef_youtube_decode_probe::Probe),
}
impl Probe {
    fn poll(&mut self) -> Option<Result<serde_json::Value, &'static str>> {
        match self {
            Self::Reference(p) => p.poll(),
            Self::Decode(p) => p.poll(),
        }
    }
}
struct Pending {
    browser: i32,
    token: u64,
    tab: u64,
    tx: Sender<Sobytie>,
    tip: &'static str,
    probe: Probe,
}
#[derive(Default)]
struct State {
    reservations: Reservations,
    pending: Vec<Pending>,
}
thread_local! {static STATE:RefCell<State>=RefCell::new(State::default());}
fn gate() -> Result<(), &'static str> {
    if !crate::youtube_benchmark_fixture::allowed()
    {
        return Err("benchmark_fixture_only");
    }
    if currently_on(ThreadId::UI) == 0 {
        return Err("probe_wrong_thread");
    }
    Ok(())
}
fn request(
    browser: &Browser,
    tab: u64,
    expected: &VideoId,
    tx: Sender<Sobytie>,
    decode: bool,
) -> Result<(), &'static str> {
    gate()?;
    if browser.is_valid() == 0 {
        return Err("probe_browser_invalid");
    }
    let identity = browser.identifier();
    let token = STATE.with(|s| s.borrow_mut().reservations.reserve(identity))?;
    // Probe::start can reenter CEF callbacks, including cancellation.
    let started = if decode {
        crate::cef_youtube_decode_probe::Probe::start(browser, expected).map(Probe::Decode)
    } else {
        crate::cef_youtube_reference_probe::Probe::start(browser, expected).map(Probe::Reference)
    };
    match started {
        Err(code) => {
            STATE.with(|s| {
                s.borrow_mut().reservations.retire(identity, token);
            });
            Err(code)
        }
        Ok(probe) => {
            let mut pending = Some(Pending {
                browser: identity,
                token,
                tab,
                tx,
                tip: if decode {
                    "youtube-decode-state"
                } else {
                    "youtube-reference-state"
                },
                probe,
            });
            let admitted = STATE.with(|s| {
                let mut s = s.borrow_mut();
                if s.reservations.matches(identity, token) {
                    s.pending.push(pending.take().unwrap());
                    true
                } else {
                    false
                }
            });
            // A canceled probe drops only after the RefCell borrow ended.
            drop(pending);
            if admitted {
                Ok(())
            } else {
                Err("benchmark_canceled")
            }
        }
    }
}
pub fn request_reference(
    browser: &Browser,
    tab: u64,
    expected: &VideoId,
    tx: Sender<Sobytie>,
) -> Result<(), &'static str> {
    request(browser, tab, expected, tx, false)
}
pub fn request_decode(
    browser: &Browser,
    tab: u64,
    expected: &VideoId,
    tx: Sender<Sobytie>,
) -> Result<(), &'static str> {
    request(browser, tab, expected, tx, true)
}
pub fn poll() {
    if gate().is_err() {
        return;
    }
    let pending = STATE.with(|s| std::mem::take(&mut s.borrow_mut().pending));
    for mut p in pending {
        if !STATE.with(|s| s.borrow().reservations.matches(p.browser, p.token)) {
            continue;
        }
        match p.probe.poll() {
            Some(reply) => {
                let current =
                    STATE.with(|s| s.borrow_mut().reservations.retire(p.browser, p.token));
                if current {
                    let value = match reply {
                        Ok(state) => serde_json::json!({"tip":p.tip,"id":p.tab,"state":state}),
                        Err(code) => {
                            serde_json::json!({"tip":p.tip,"id":p.tab,"error":"Нативное наблюдение недоступно","code":code})
                        }
                    };
                    let _ = p.tx.send(Sobytie::Sluzhebnoe(value));
                }
            }
            None => {
                let mut pending = Some(p);
                STATE.with(|s| {
                    let mut s = s.borrow_mut();
                    let p = pending.as_ref().unwrap();
                    if s.reservations.matches(p.browser, p.token) {
                        s.pending.push(pending.take().unwrap());
                    }
                });
                drop(pending);
            }
        }
    }
}
pub fn cancel(browser: i32) {
    let retired = STATE.with(|s| {
        let mut s = s.borrow_mut();
        s.reservations.current.remove(&browser);
        let all = std::mem::take(&mut s.pending);
        let (removed, kept) = all
            .into_iter()
            .partition::<Vec<_>, _>(|p| p.browser == browser);
        s.pending = kept;
        removed
    });
    drop(retired);
}
pub fn clear() {
    let retired = STATE.with(|s| {
        let mut s = s.borrow_mut();
        s.reservations.current.clear();
        std::mem::take(&mut s.pending)
    });
    drop(retired);
}
#[cfg(test)]
mod tests {
    use super::Reservations;
    #[test]
    fn cancellation_never_reuses_identity() {
        let mut r = Reservations::default();
        let a = r.reserve(7).unwrap();
        assert!(r.retire(7, a));
        let b = r.reserve(7).unwrap();
        assert_ne!(a, b);
        assert!(!r.retire(7, a));
        assert!(r.matches(7, b));
    }
    #[test]
    fn capacity_includes_starting_and_polled_out_probes() {
        let mut r = Reservations::default();
        for id in 1..=4 {
            r.reserve(id).unwrap();
        }
        assert_eq!(r.reserve(5), Err("benchmark_capacity"));
        assert_eq!(r.reserve(1), Err("benchmark_browser_busy"));
    }
    #[test]
    fn exhaustion_is_fail_closed() {
        let mut r = Reservations {
            next: u64::MAX,
            ..Default::default()
        };
        assert_eq!(r.reserve(1), Err("benchmark_token_exhausted"));
        assert!(r.current.is_empty());
    }
}
