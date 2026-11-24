//! Bounded user-intent handoff. This module NEVER changes focus itself.
//! Caller must finish every dequeued request and revalidate the native current
//! browser, selected tab, route, lock/closing state and exact media document.
use crate::cef_vkladki::Sobytie;
use cef::*;
use std::sync::{mpsc::Sender, Mutex};
use std::time::{Duration, Instant};

const INTERVAL: Duration = Duration::from_millis(100);
static STATE: Mutex<State> = Mutex::new(State {
    pending: false,
    last: None,
});
struct State {
    pending: bool,
    last: Option<Instant>,
}
impl State {
    fn claim(&mut self, now: Instant) -> bool {
        if self.pending
            || self
                .last
                .is_some_and(|last| now.saturating_duration_since(last) < INTERVAL)
        {
            return false;
        }
        self.pending = true;
        self.last = Some(now);
        true
    }
}
fn source_matches(document: &str, source: &str) -> bool {
    // Compare the complete nonce-bearing URL. No prefix/suffix-only authority:
    // an old queued console message cannot acquire the new document's grant.
    !source.is_empty()
        && document
            .strip_suffix("player.html")
            .is_some_and(|base| source.strip_suffix("player.js") == Some(base))
}

/// Actual native visibility; never inferred from selected-tab state alone.
#[cfg(target_os = "linux")]
pub(crate) fn visible(browser: &Browser) -> bool {
    use x11rb::protocol::xproto::{ConnectionExt, MapState};
    let Some(host) = browser.host() else {
        return false;
    };
    let window = host.window_handle() as u32;
    if window <= 1 {
        return false;
    }
    let Ok((connection, _)) = x11rb::connect(None) else {
        return false;
    };
    let Ok(cookie) = connection.get_window_attributes(window) else {
        return false;
    };
    cookie
        .reply()
        .is_ok_and(|attributes| attributes.map_state == MapState::VIEWABLE)
}
#[cfg(target_os = "windows")]
pub(crate) fn visible(browser: &Browser) -> bool {
    use windows::Win32::{
        Foundation::HWND,
        UI::WindowsAndMessaging::{GetAncestor, IsIconic, IsWindow, IsWindowVisible, GA_ROOT},
    };
    let Some(host) = browser.host() else {
        return false;
    };
    let window = HWND(host.window_handle().0 as *mut core::ffi::c_void);
    if window.0.is_null() {
        return false;
    }
    unsafe {
        let root = GetAncestor(window, GA_ROOT);
        IsWindow(window).as_bool()
            && IsWindowVisible(window).as_bool()
            && !root.0.is_null()
            && !IsIconic(root).as_bool()
    }
}
#[cfg(not(any(target_os = "linux", target_os = "windows")))]
pub(crate) fn visible(_browser: &Browser) -> bool {
    false
}

/// Called on CEF UI only, for the exact fixed marker from the static player.
/// Source must be the current nonce-bearing player.js, not renderer arguments.
/// Returns whether a request was queued. Unknown/stale identity fails closed.
pub(crate) fn request(browser: &Browser, tab: u64, source: &str, tx: &Sender<Sobytie>) -> bool {
    if !visible(browser) {
        return false;
    }
    // CEF and grant lookup complete BEFORE acquiring our coalescing mutex.
    let Some((operation, document, _media)) = crate::cef_youtube::media(browser) else {
        return false;
    };
    if !source_matches(&document, source) {
        return false;
    }
    let identity = browser.identifier();
    let claimed = STATE
        .lock()
        .map(|mut state| state.claim(Instant::now()))
        .unwrap_or(false);
    if !claimed {
        return false;
    }
    if tx
        .send(Sobytie::YoutubeFocus {
            browser: identity,
            tab,
            operation,
            document,
        })
        .is_err()
    {
        finish();
        return false;
    }
    true
}

/// Consumer must call on EVERY dequeued YoutubeFocus, including rejected ones.
/// Keeps the monotonic rate limit after releasing the outstanding-request slot.
pub(crate) fn finish() {
    if let Ok(mut state) = STATE.lock() {
        state.pending = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn console_source_is_bound_to_current_document_nonce() {
        let document = "nb-light://player/012345/player.html";
        assert!(source_matches(
            document,
            "nb-light://player/012345/player.js"
        ));
        for source in [
            "",
            "unknown",
            "nb-light://player/stale/player.js",
            "https://remote/player.js",
            "nb-light://player/012345/player.js?x",
            "nb-light://player/012345/other-player.js",
            "nb-light://player/012345/player.html",
        ] {
            assert!(!source_matches(document, source));
        }
        assert!(!source_matches(
            "nb-light://player/012345/other.html",
            "nb-light://player/012345/player.js"
        ));
    }
    #[test]
    fn pending_slot_and_monotonic_rate_are_both_enforced() {
        let now = Instant::now();
        let mut state = State {
            pending: false,
            last: None,
        };
        assert!(state.claim(now));
        assert!(!state.claim(now + INTERVAL * 20)); // outstanding beats elapsed time
        state.pending = false; // rejected event also releases the slot
        assert!(!state.claim(now + Duration::from_millis(99)));
        assert!(state.claim(now + INTERVAL));
        state.pending = false;
        assert!(!state.claim(now)); // stale time cannot bypass the interval
        assert!(state.claim(now + INTERVAL * 2));
    }
}
