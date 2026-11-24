//! Fixed CI-only download pipeline receipts. No control authority or raw data.
use crate::cef_vkladki::Sobytie;
use serde::Serialize;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    mpsc::Sender,
};
use tauri::Emitter;
static RECEIPTS: AtomicUsize = AtomicUsize::new(0);
const MAX_RECEIPTS: usize = 128;
#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    MarkerObserved,
    MarkerAdmitted,
    RequestReceived,
    RequestAdmitted,
    ParentReceived,
    ParentAdmitted,
    DialogDispatch,
    DialogReturned,
    TargetReceived,
    WorkerStarted,
}
#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Reason {
    None,
    MediaNotAdmitted,
    NotSelected,
    LockedOrClosing,
    RouteMissing,
    FormatUnsupported,
    JobLimit,
    InvalidRequest,
    CurrentEngineMismatch,
    SelectedSessionMismatch,
    OwnerMissing,
    Cancelled,
    Picked,
    ChannelMissing,
    ChannelSent,
    RandomUnavailable,
    StateUnavailable,
    WorkerSpawnFailed,
}
fn admitted() -> bool {
    if std::env::var("GITHUB_ACTIONS").as_deref() != Ok("true")
        || std::env::var_os("NEXUSBRO_TEST_DATA").is_none()
    {
        return false;
    }
    RECEIPTS
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| {
            (n < MAX_RECEIPTS).then_some(n + 1)
        })
        .is_ok()
}
fn data(tab: u64, stage: Stage, reason: Reason) -> serde_json::Value {
    serde_json::json!({"tip":"youtube-download-trace","id":tab,"stage":stage,"reason":reason})
}
pub fn child(sender: &Sender<Sobytie>, tab: u64, stage: Stage, reason: Reason) {
    if admitted() {
        let _ = sender.send(Sobytie::Sluzhebnoe(data(tab, stage, reason)));
    }
}
pub fn parent(app: &tauri::AppHandle, super_id: &str, tab: u64, stage: Stage, reason: Reason) {
    if admitted() {
        let _ = app.emit_to(
            "main",
            "stranica-ui",
            serde_json::json!({"super_id":super_id,"data":data(tab,stage,reason)}),
        );
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fixed_payload_contains_only_existing_tab_and_bounded_enums() {
        let payload = data(1, Stage::ParentReceived, Reason::SelectedSessionMismatch);
        let object = payload.as_object().unwrap();
        assert_eq!(object.len(), 4);
        assert_eq!(payload["id"], 1);
        assert_eq!(payload["stage"], "parent_received");
        assert_eq!(payload["reason"], "selected_session_mismatch");
        assert!(object
            .keys()
            .all(|k| matches!(k.as_str(), "tip" | "id" | "stage" | "reason")));
    }
}
