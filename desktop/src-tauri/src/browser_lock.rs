//! Master-password gate for the complete browser, separate from account login.
use crate::{secure_vault::VaultState, Sostoyanie};
use serde::Serialize;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};
use tauri::{Emitter, Manager};

// Fail closed until setup has configured the existing encrypted vault.
// Generation and lock bit share one atomic snapshot: a new engine can never
// receive an old lock bit paired with a newer generation.
static LOCK_STATE: AtomicU64 = AtomicU64::new(1);
fn transition(locked: bool) {
    let _ = LOCK_STATE.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |old| {
        Some(((old & !1).wrapping_add(2)) | u64::from(locked))
    });
}
pub fn engine_state() -> (bool, u64) {
    let state = LOCK_STATE.load(Ordering::SeqCst);
    (state & 1 != 0, state >> 1)
}
pub fn generation() -> u64 {
    engine_state().1
}
#[derive(Default)]
pub struct BrowserLockState {
    serial: tokio::sync::Mutex<()>,
}
#[derive(Clone, Serialize)]
pub struct LockStatus {
    pub locked: bool,
    pub exists: bool,
    pub revision: u64,
}
pub fn locked_global() -> bool {
    engine_state().0
}
pub fn is_locked(_app: &tauri::AppHandle) -> bool {
    locked_global()
}
fn trusted(app: &tauri::AppHandle, label: &str) -> bool {
    label == "main"
        || app
            .state::<Sostoyanie>()
            .okna
            .0
            .lock()
            .unwrap()
            .contains_key(label)
}
fn validate(app: &tauri::AppHandle, window: &tauri::WebviewWindow) -> Result<(), String> {
    if trusted(app, window.label()) {
        Ok(())
    } else {
        Err("Окно не управляет блокировкой браузера".into())
    }
}
fn status(app: &tauri::AppHandle) -> LockStatus {
    let (locked, revision) = engine_state();
    LockStatus {
        locked,
        exists: app.state::<Arc<VaultState>>().status().exists,
        revision,
    }
}
fn publish(app: &tauri::AppHandle) -> LockStatus {
    let result = status(app);
    for (label, _) in app.webview_windows() {
        if trusted(app, &label) {
            let _ = app.emit_to(label, "browser-lock-changed", &result);
        }
    }
    result
}
pub fn setup(app: &tauri::AppHandle) {
    transition(app.state::<Arc<VaultState>>().status().exists);
    if locked_global() {
        hide_all(app);
    }
}
fn deliver(
    channel: &std::sync::mpsc::SyncSender<crate::sessiya::Komanda>,
    command: crate::sessiya::Komanda,
) {
    if let Err(std::sync::mpsc::TrySendError::Full(command)) = channel.try_send(command) {
        let channel = channel.clone();
        // Never block the owner/UI thread and never silently discard the lock
        // because an earlier burst filled the bounded CEF command queue.
        std::thread::spawn(move || {
            let _ = channel.send(command);
        });
    }
}
/// Synchronize a newly registered CEF pipe with the current controller state.
pub fn synchronize_engine(channel: &std::sync::mpsc::SyncSender<crate::sessiya::Komanda>) {
    let (locked, generation) = engine_state();
    deliver(
        channel,
        crate::sessiya::Komanda::BrowserLocked { locked, generation },
    );
}
fn engine_lock(app: &tauri::AppHandle, locked: bool) {
    let generation = generation();
    let state = app.state::<Sostoyanie>();
    let channels = state
        .vkladki
        .lock()
        .unwrap()
        .iter()
        .filter_map(|(id, tab)| tab.kanal.clone().map(|channel| (id.clone(), channel)))
        .collect::<std::collections::HashMap<_, _>>();
    for channel in channels.values() {
        deliver(
            channel,
            crate::sessiya::Komanda::BrowserLocked { locked, generation },
        );
    }
    for (label, extra) in state
        .okna
        .0
        .lock()
        .unwrap()
        .iter()
        .filter(|(_, extra)| !extra.closing)
    {
        if let Some(channel) = channels.get(&extra.super_id) {
            deliver(
                channel,
                crate::sessiya::Komanda::OknoKomanda {
                    label: label.clone(),
                    komanda: Box::new(crate::sessiya::Komanda::BrowserLocked {
                        locked,
                        generation,
                    }),
                },
            );
        }
    }
}
fn hide_all(app: &tauri::AppHandle) {
    let state = app.state::<Sostoyanie>();
    let _windows = state.izmenenie_okon.lock().unwrap();
    state.dvizhki_skryty.store(true, Ordering::SeqCst);
    let mut handles = Vec::new();
    let mut pids = Vec::new();
    {
        let tabs = state.vkladki.lock().unwrap();
        for tab in tabs.values() {
            handles.extend(tab.okno);
            pids.extend(tab.proc.as_ref().map(std::process::Child::id));
            pids.extend(tab.zapas_pid);
        }
    }
    {
        let mut extras = state.okna.0.lock().unwrap();
        for extra in extras.values_mut() {
            extra.hidden = true;
            handles.extend(extra.hwnd);
            handles.extend(extra.others.values().filter_map(|view| view.hwnd));
        }
    }
    handles.extend(crate::okna::sistema::vse_okna(&pids));
    handles.sort_unstable();
    handles.dedup();
    for handle in handles {
        let _ = crate::okna::sistema::pokazat(handle, false);
    }
    crate::okna::sistema::spryatat_chuzhie(&pids);
    crate::primenit_vidimost(&state);
    let labels = state
        .okna
        .0
        .lock()
        .unwrap()
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    for label in labels {
        crate::okna_app::obolochka(app, &state, &label);
    }
    // An existing popup must never remain above the master-password screen.
    for (label, popup) in app.webview_windows() {
        if label.starts_with("menu-") {
            let _ = popup.destroy();
        }
    }
}
fn restore_all(app: &tauri::AppHandle) {
    let state = app.state::<Sostoyanie>();
    let _windows = state.izmenenie_okon.lock().unwrap();
    state.dvizhki_skryty.store(false, Ordering::SeqCst);
    let labels = {
        let mut extras = state.okna.0.lock().unwrap();
        extras
            .iter_mut()
            .filter_map(|(label, extra)| {
                if extra.closing {
                    None
                } else {
                    extra.hidden = false;
                    Some(label.clone())
                }
            })
            .collect::<Vec<_>>()
    };
    crate::primenit_vidimost(&state);
    for label in labels {
        crate::okna_app::obolochka(app, &state, &label);
    }
}
async fn lock_inner(app: &tauri::AppHandle) -> Result<LockStatus, String> {
    let serial = app.state::<BrowserLockState>();
    let _guard = serial.serial.lock().await;
    if !app.state::<Arc<VaultState>>().status().exists {
        return Err(
            "Сначала включите сохранение паролей и создайте мастер-пароль в настройках защиты"
                .into(),
        );
    }
    if locked_global() {
        return Ok(status(app));
    }
    // Refuse every new browsing command and cover native content before waiting
    // for vault work. Even a failed vault operation cannot expose the browser.
    transition(true);
    engine_lock(app, true);
    hide_all(app);
    publish(app);
    crate::passwords::security_vault(app.clone(), app.state(), "lock".into(), None).await?;
    Ok(publish(app))
}
/// Called only from the native CEF keyboard event path, never a page console.
pub fn request_lock(app: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        if let Err(message) = lock_inner(&app).await {
            for (label, _) in app.webview_windows() {
                if trusted(&app, &label) {
                    let _ = app.emit_to(label, "browser-lock-unavailable", &message);
                }
            }
        }
    });
}
#[tauri::command]
pub fn browser_lock_status(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
) -> Result<LockStatus, String> {
    validate(&app, &window)?;
    Ok(status(&app))
}
#[tauri::command]
pub async fn browser_lock(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
) -> Result<LockStatus, String> {
    validate(&app, &window)?;
    lock_inner(&app).await
}
#[tauri::command]
pub async fn browser_unlock(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
    password: String,
) -> Result<LockStatus, String> {
    validate(&app, &window)?;
    let serial = app.state::<BrowserLockState>();
    let _guard = serial.serial.lock().await;
    if !locked_global() {
        return Ok(status(&app));
    }
    if !app.state::<Arc<VaultState>>().status().exists {
        return Err("Защищённое хранилище недоступно; браузер остаётся заблокирован".into());
    }
    // Existing vault code does bounded Argon2id/AEAD verification, zeroizes the
    // submitted password and leaves main/backup unchanged for a wrong password.
    let vault =
        crate::passwords::security_vault(app.clone(), app.state(), "unlock".into(), Some(password))
            .await?;
    if !vault.unlocked {
        return Err("Браузер остаётся заблокирован".into());
    }
    transition(false);
    engine_lock(&app, false);
    restore_all(&app);
    let result = publish(&app);
    // At startup the native RunEvent autostart is deliberately gated. Resume
    // only the last active Super after a successful master-password check.
    let state = app.state::<Sostoyanie>();
    if let Some(id) = crate::zagruzit_aktivnuyu() {
        let sleeping = state
            .vkladki
            .lock()
            .unwrap()
            .get(&id)
            .is_some_and(|tab| tab.proc.is_none() && !tab.zapusk_idet);
        if sleeping {
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                if !locked_global() {
                    let state = app.state::<Sostoyanie>();
                    let _ = crate::zapustit_vnutri(&app, &state, &id).await;
                    let _ = app.emit("vkladki-izmenilis", id);
                }
            });
        }
    }
    Ok(result)
}
