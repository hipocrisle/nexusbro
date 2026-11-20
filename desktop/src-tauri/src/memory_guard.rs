//! Native negative signals and renderer document epochs. Unknown means keep.
use std::{cell::RefCell, collections::HashMap};
struct NativeError {
    epoch: u64,
    failed_url: String,
    confirmed_token: Option<String>,
}
#[derive(Default)]
struct Protection {
    edited: bool,
    non_get: bool,
    pending_get: Option<bool>,
    token: Option<String>,
    capture: bool,
    navigation_epoch: u64,
    navigation_url: String,
    navigation_get: bool,
    error_eligible: bool,
    error: Option<NativeError>,
    pending_error_document: Option<(u64, String, String)>,
}
thread_local! { static DOCUMENTS:RefCell<HashMap<i32,Protection>>=Default::default(); }
pub fn navigation(browser: i32, get: bool) {
    navigation_to(browser, get, "");
}
pub fn navigation_to(browser: i32, get: bool, url: &str) {
    DOCUMENTS.with(|m| {
        let mut m = m.borrow_mut();
        let p = m.entry(browser).or_default();
        p.pending_get = Some(get);
        p.navigation_epoch = p.navigation_epoch.saturating_add(1);
        p.navigation_url = url.into();
        p.navigation_get = get;
        p.error_eligible = true;
        p.error = None;
        p.pending_error_document = None;
    });
}
fn set_document(p: &mut Protection, token: String) {
    if p.token.as_ref() != Some(&token) {
        p.edited = false;
        p.non_get = !p.pending_get.take().unwrap_or(false);
        p.token = Some(token);
    }
}
pub fn document(browser: i32, token: String) {
    DOCUMENTS.with(|m| {
        let mut m = m.borrow_mut();
        let p = m.entry(browser).or_default();
        set_document(p, token);
        p.error = None;
        p.error_eligible = false;
        p.pending_error_document = None;
    });
}
/// Only the native main-frame load-error callback may create this candidate.
/// Renderer messages alone cannot convert an unknown document into an error page.
pub fn native_load_error(browser: i32, failed_url: &str) -> bool {
    DOCUMENTS.with(|m| {
        let mut m = m.borrow_mut();
        let Some(p) = m.get_mut(&browser) else {
            return false;
        };
        if !p.error_eligible
            || p.navigation_url != failed_url
            || !(failed_url.starts_with("http://") || failed_url.starts_with("https://"))
        {
            return false;
        }
        let pending = p
            .pending_error_document
            .take()
            .filter(|(epoch, url, _)| *epoch == p.navigation_epoch && url == failed_url);
        let confirmed_token = pending.map(|(_, _, token)| {
            set_document(p, token.clone());
            token
        });
        p.error = Some(NativeError {
            epoch: p.navigation_epoch,
            failed_url: failed_url.into(),
            confirmed_token,
        });
        true
    })
}
pub fn load_succeeded(browser: i32) {
    DOCUMENTS.with(|m| {
        if let Some(p) = m.borrow_mut().get_mut(&browser) {
            p.error = None;
            p.error_eligible = false;
            p.pending_error_document = None;
        }
    });
}
/// Both URLs come from authenticated native CEF frame metadata, not page JS.
pub fn error_document(browser: i32, current_url: &str, renderer_url: &str, token: String) -> bool {
    DOCUMENTS.with(|m| {
        let mut m = m.borrow_mut();
        let Some(p) = m.get_mut(&browser) else {
            return false;
        };
        if !matches!(
            renderer_url,
            "chrome-error://chromewebdata/" | "chrome-error://chromewebdata"
        ) {
            return false;
        }
        let Some(error) = p.error.as_ref().filter(|e| {
            e.epoch == p.navigation_epoch
                && e.failed_url == current_url
                && e.failed_url == p.navigation_url
        }) else {
            // Native load and renderer IPC callbacks can arrive in either order.
            // Store a candidate only; it grants no permission without LoadError.
            if p.error_eligible && p.navigation_url == current_url {
                p.pending_error_document = Some((p.navigation_epoch, current_url.into(), token));
            }
            return false;
        };
        let epoch = error.epoch;
        set_document(p, token.clone());
        p.error = Some(NativeError {
            epoch,
            failed_url: current_url.into(),
            confirmed_token: Some(token),
        });
        true
    })
}
pub fn confirmed_error(browser: i32, current_url: &str) -> bool {
    DOCUMENTS.with(|m| {
        m.borrow().get(&browser).is_some_and(|p| {
            p.error.as_ref().is_some_and(|e| {
                e.epoch == p.navigation_epoch
                    && e.failed_url == current_url
                    && e.failed_url == p.navigation_url
                    && e.confirmed_token.is_some()
                    && e.confirmed_token == p.token
            })
        })
    })
}
/// A confirmed Chromium-owned error document has no owner web application state.
/// Keep native editing, capture and POST protection even for this narrow case.
pub fn error_reason(browser: i32, current_url: &str) -> Option<&'static str> {
    if !confirmed_error(browser, current_url) {
        return Some("Состояние документа ещё неизвестно");
    }
    DOCUMENTS.with(|m| {
        let m = m.borrow();
        let p = m.get(&browser).unwrap();
        if p.capture {
            Some("Вкладка использует камеру, микрофон или захват экрана")
        } else if p.edited {
            Some("Во вкладке был ввод; перезагрузите страницу после сохранения данных")
        } else if p.non_get || !p.navigation_get {
            Some("Страница получена отправкой формы и не выгружается")
        } else {
            None
        }
    })
}
pub fn token(browser: i32) -> Option<String> {
    DOCUMENTS.with(|m| m.borrow().get(&browser).and_then(|p| p.token.clone()))
}
pub fn capture(browser: i32, active: bool) {
    DOCUMENTS.with(|m| {
        m.borrow_mut().entry(browser).or_default().capture = active;
    });
}
pub fn edited(browser: i32) {
    DOCUMENTS.with(|m| {
        m.borrow_mut().entry(browser).or_default().edited = true;
    });
}
pub fn reason(browser: i32) -> Option<&'static str> {
    DOCUMENTS.with(|m| {
        let m = m.borrow();
        let Some(p) = m.get(&browser).filter(|p| p.token.is_some()) else {
            return Some("Состояние документа ещё неизвестно");
        };
        if p.capture {
            Some("Вкладка использует камеру, микрофон или захват экрана")
        } else if p.edited {
            Some("Во вкладке был ввод; перезагрузите страницу после сохранения данных")
        } else if p.non_get {
            Some("Страница получена отправкой формы и не выгружается")
        } else {
            None
        }
    })
}
pub fn closed(browser: i32) {
    DOCUMENTS.with(|m| {
        m.borrow_mut().remove(&browser);
    });
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn canceled_navigation_never_clears_dirty_document() {
        navigation(100, true);
        document(100, "first".into());
        assert!(reason(100).is_none());
        edited(100);
        navigation(100, true);
        assert!(reason(100).is_some());
        document(100, "first".into());
        assert!(reason(100).is_some());
        document(100, "second".into());
        assert!(reason(100).is_none());
        navigation(100, false);
        document(100, "post".into());
        assert!(reason(100).is_some());
        capture(100, true);
        navigation(100, true);
        document(100, "third".into());
        assert!(reason(100).is_some());
        capture(100, false);
        assert!(reason(100).is_none());
        closed(100);
        assert!(reason(100).is_some());
    }
    #[test]
    fn renderer_cannot_forge_error_or_reuse_a_stale_navigation() {
        let b = 901;
        navigation_to(b, true, "https://one.test/");
        assert!(!error_document(
            b,
            "https://one.test/",
            "chrome-error://chromewebdata/",
            "forged".into()
        ));
        assert!(!native_load_error(b, "https://other.test/"));
        assert!(native_load_error(b, "https://one.test/"));
        navigation_to(b, true, "https://two.test/");
        assert!(!native_load_error(b, "https://one.test/"));
        assert!(!error_document(
            b,
            "https://one.test/",
            "chrome-error://chromewebdata/",
            "old".into()
        ));
        assert!(!confirmed_error(b, "https://one.test/"));
        closed(b);
    }
    #[test]
    fn native_error_requires_matching_internal_document_and_get() {
        let b = 902;
        let url = "https://unreachable.test/";
        navigation_to(b, true, url);
        assert!(native_load_error(b, url));
        assert!(!error_document(b, url, url, "normal".into()));
        assert!(!error_document(
            b,
            "https://other.test/",
            "chrome-error://chromewebdata/",
            "wrong".into()
        ));
        assert!(error_document(
            b,
            url,
            "chrome-error://chromewebdata/",
            "error".into()
        ));
        assert!(confirmed_error(b, url));
        assert!(error_reason(b, url).is_none());
        edited(b);
        assert!(error_reason(b, url).is_some());
        navigation_to(b, false, url);
        assert!(native_load_error(b, url));
        assert!(error_document(
            b,
            url,
            "chrome-error://chromewebdata/",
            "post".into()
        ));
        assert!(error_reason(b, url).is_some());
        closed(b);
    }
    #[test]
    fn error_capture_success_and_normal_document_never_allow_stale_bypass() {
        let b = 903;
        let url = "https://unreachable.test/";
        navigation_to(b, true, url);
        assert!(native_load_error(b, url));
        assert!(error_document(
            b,
            url,
            "chrome-error://chromewebdata/",
            "error".into()
        ));
        capture(b, true);
        assert!(error_reason(b, url).is_some());
        capture(b, false);
        assert!(error_reason(b, url).is_none());
        load_succeeded(b);
        assert!(!confirmed_error(b, url));
        assert!(!native_load_error(b, url));
        navigation_to(b, true, url);
        assert!(native_load_error(b, url));
        document(b, "normal".into());
        assert!(!confirmed_error(b, url));
        assert!(!native_load_error(b, url));
        closed(b);
        assert!(!confirmed_error(b, url));
    }
    #[test]
    fn logical_blank_and_unknown_normal_documents_get_no_error_exception() {
        let b = 904;
        navigation_to(b, true, "about:blank");
        assert!(!native_load_error(b, "about:blank"));
        assert!(!error_document(
            b,
            "about:blank",
            "chrome-error://chromewebdata/",
            "blank".into()
        ));
        assert!(reason(b).is_some());
        document(b, "observed-blank".into());
        assert!(reason(b).is_none());
        edited(b);
        assert!(reason(b).is_some());
        closed(b);
    }

    #[test]
    fn reverse_callback_order_requires_native_error_before_permission() {
        let b = 905;
        let url = "https://unreachable.test/";
        navigation_to(b, true, url);
        assert!(!error_document(
            b,
            url,
            "chrome-error://chromewebdata/",
            "early".into()
        ));
        assert!(!confirmed_error(b, url));
        assert!(native_load_error(b, url));
        assert!(confirmed_error(b, url));
        assert!(error_reason(b, url).is_none());
        navigation_to(b, true, "https://different.test/");
        assert!(!native_load_error(b, url));
        assert!(!confirmed_error(b, url));
        closed(b);
    }
}
