//! Native negative signals and renderer document epochs. Unknown means keep.
use std::{cell::RefCell, collections::HashMap};
#[derive(Default)]
struct Protection {
    edited: bool,
    non_get: bool,
    pending_get: Option<bool>,
    token: Option<String>,
    capture: bool,
}
thread_local! { static DOCUMENTS:RefCell<HashMap<i32,Protection>>=Default::default(); }
pub fn navigation(browser: i32, get: bool) {
    DOCUMENTS.with(|m| {
        m.borrow_mut().entry(browser).or_default().pending_get = Some(get);
    });
}
pub fn document(browser: i32, token: String) {
    DOCUMENTS.with(|m| {
        let mut m = m.borrow_mut();
        let p = m.entry(browser).or_default();
        if p.token.as_ref() != Some(&token) {
            p.edited = false;
            p.non_get = !p.pending_get.take().unwrap_or(false);
            p.token = Some(token);
        }
    });
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
}
