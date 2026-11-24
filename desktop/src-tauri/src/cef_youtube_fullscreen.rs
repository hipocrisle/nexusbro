//! CEF-native fullscreen receipts. Never callable from page console messages.
use crate::cef_vkladki::Sobytie;
use cef::*;
use std::{
    cell::RefCell,
    collections::HashMap,
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc::Sender,
    },
};
static SEQUENCE: AtomicU64 = AtomicU64::new(1);
thread_local! { static ENTERED: RefCell<HashMap<i32,u64>> = RefCell::new(HashMap::new()); }
fn emit(browser: i32, tab: u64, fullscreen: bool, tx: &Sender<Sobytie>) {
    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    if sequence == u64::MAX {
        return;
    }
    let _ = tx.send(Sobytie::Sluzhebnoe(serde_json::json!({
        "tip":"youtube-fullscreen", "id":tab, "browser_id":browser,
        "fullscreen":fullscreen, "sequence":sequence
    })));
}
/// Invoke exclusively from DisplayHandler::on_fullscreen_mode_change on CEF UI.
/// `selected` comes from the native view's selected ordinary-tab state.
pub fn changed(
    browser: &Browser,
    tab: u64,
    selected: bool,
    fullscreen: bool,
    tx: &Sender<Sobytie>,
) {
    if !fullscreen {
        leaving(browser.identifier(), tab, tx);
        return;
    }
    if currently_on(ThreadId::UI) == 0 || !selected || browser.is_valid() == 0 {
        return;
    }
    let Some(frame) = browser.main_frame() else {
        return;
    };
    let document = CefString::from(&frame.url()).to_string();
    if crate::cef_youtube::logical_url(browser.identifier(), &document).is_none() {
        return;
    }
    let accepted = ENTERED.with(|set| {
        let mut set = set.borrow_mut();
        if set.len() >= 128 || set.contains_key(&browser.identifier()) {
            return false;
        }
        set.insert(browser.identifier(), tab);
        true
    });
    if accepted {
        emit(browser.identifier(), tab, true, tx);
    }
}
/// Call BEFORE navigation revokes admission, and on close/lock. Exit uses only
/// a prior native admission, so an unrelated document cannot enter fullscreen.
pub fn leaving(browser: i32, tab: u64, tx: &Sender<Sobytie>) {
    if currently_on(ThreadId::UI) == 0 {
        return;
    }
    if ENTERED.with(|set| set.borrow_mut().remove(&browser).is_some()) {
        emit(browser, tab, false, tx);
    }
}
/// Root must call leaving with each native tab id before destroying the view.
/// This final shutdown fallback has no page-controlled identifiers.
pub fn clear(tx: &Sender<Sobytie>) {
    if currently_on(ThreadId::UI) == 0 {
        return;
    }
    let entered = ENTERED.with(|set| std::mem::take(&mut *set.borrow_mut()));
    for (browser, tab) in entered {
        emit(browser, tab, false, tx);
    }
}

/// Fixed native action, admitted current player only. No renderer script input.
pub fn leave_browser(browser: &Browser, tab: u64, tx: &Sender<Sobytie>) {
    if currently_on(ThreadId::UI) == 0 {
        return;
    }
    let identity = browser.identifier();
    let entered = ENTERED.with(|set| set.borrow().contains_key(&identity));
    if !entered {
        return;
    }
    if let Some(frame) = browser.main_frame() {
        let document = CefString::from(&frame.url()).to_string();
        if crate::cef_youtube::logical_url(identity, &document).is_some() {
            frame.execute_java_script(
                Some(&CefString::from(
                    "if(document.fullscreenElement){document.exitFullscreen().catch(()=>{});}",
                )),
                None,
                0,
            );
        }
    }
    leaving(identity, tab, tx);
}
