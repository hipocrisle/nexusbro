//! Preserve correlation IDs across interleaved native CDP clients.
//! ExecuteDevToolsMethod rewrites IDs below its high-water mark; raw protocol
//! messages preserve the IDs reserved before observers are installed.
use cef::*;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicI32, Ordering};
use zeroize::Zeroizing;
static NEXT: AtomicI32 = AtomicI32::new(1);
/// Exhaustion fails closed rather than wrapping into an outstanding request.
pub fn reserve(count: i32) -> Option<i32> {
    if count <= 0 {
        return None;
    }
    NEXT.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
        value.checked_add(count)
    })
    .ok()
}
pub fn send(browser: &Browser, id: i32, method: &str, params: Value) -> bool {
    if id <= 0 || method.is_empty() || !params.is_object() {
        return false;
    }
    let Some(host) = browser.host() else {
        return false;
    };
    let message = Zeroizing::new(json!({"id":id,"method":method,"params":params}).to_string());
    host.send_dev_tools_message(Some(message.as_bytes())) != 0
}
