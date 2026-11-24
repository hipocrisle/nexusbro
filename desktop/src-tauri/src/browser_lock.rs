//! Master-password gate for the complete browser, separate from account login.
use crate::{secure_vault::VaultState, Sostoyanie};
use serde::Serialize;
use std::sync::{atomic::Ordering, Arc};
use tauri::{Emitter, Manager};

// Fail closed until setup has configured the existing encrypted vault.
// Generation and lock bit share one atomic snapshot: a new engine can never
// receive an old lock bit paired with a newer generation.
static LOCK_GATE: crate::browser_lock_intent::IntentGate =
    crate::browser_lock_intent::IntentGate::new();
fn transition(locked: bool) {
    LOCK_GATE.transition(locked);
}
#[cfg(test)]
pub(crate) fn transition_for_test(locked: bool) {
    transition(locked);
}
pub fn engine_state() -> (bool, u64) {
    LOCK_GATE.engine_state()
}
pub fn generation() -> u64 {
    engine_state().1
}
#[derive(Default)]
pub struct BrowserLockState {
    serial: tokio::sync::Mutex<()>,
    #[cfg(any(target_os = "linux", target_os = "windows"))]
    shortcuts: std::sync::Mutex<std::collections::HashSet<String>>,
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
/// Handle the security shortcut before a focused GTK/WebKit widget consumes it.
/// This is installed only by trusted Rust window constructors, never by page IPC.
#[cfg(target_os = "linux")]
pub fn install_shortcut(window: &tauri::WebviewWindow) {
    let shell = window.clone();
    let app = window.app_handle().clone();
    if window
        .run_on_main_thread(move || {
            use gtk::{gdk::ModifierType, glib::Propagation, prelude::*};
            let Ok(native) = shell.gtk_window() else {
                crate::zapisat_v_zhurnal("NEXUSBRO_BROWSER_LOCK shortcut=gtk-unavailable".into());
                return;
            };
            let label = shell.label().to_owned();
            if !app
                .state::<BrowserLockState>()
                .shortcuts
                .lock()
                .unwrap()
                .insert(label.clone())
            {
                return;
            }
            let target = app.clone();
            let held = std::rc::Rc::new(std::cell::Cell::new(false));
            let pressed = held.clone();
            let released = held.clone();
            let unfocused = held.clone();
            native.connect_key_release_event(move |_, event| {
                if event.hardware_keycode() == 46
                    || event
                        .keyval()
                        .to_unicode()
                        .is_some_and(|key| matches!(key, 'l' | 'L' | 'д' | 'Д'))
                {
                    released.set(false);
                }
                Propagation::Proceed
            });
            native.connect_focus_out_event(move |_, _| {
                unfocused.set(false);
                Propagation::Proceed
            });
            native.connect_key_press_event(move |_, event| {
                let modifiers = event.state();
                let required = ModifierType::CONTROL_MASK | ModifierType::SHIFT_MASK;
                let excluded =
                    ModifierType::MOD1_MASK | ModifierType::META_MASK | ModifierType::SUPER_MASK;
                let key = event.hardware_keycode() == 46
                    || event
                        .keyval()
                        .to_unicode()
                        .is_some_and(|key| matches!(key, 'l' | 'L' | 'д' | 'Д'));
                if !modifiers.contains(required) || modifiers.intersects(excluded) || !key {
                    return Propagation::Proceed;
                }
                crate::zapisat_v_zhurnal("NEXUSBRO_BROWSER_LOCK shortcut=gtk-key".into());
                if !pressed.replace(true) {
                    request_lock(target.clone());
                }
                Propagation::Stop
            });
            native.connect_destroy(move |_| {
                if let Some(state) = app.try_state::<BrowserLockState>() {
                    state.shortcuts.lock().unwrap().remove(&label);
                }
            });
            crate::zapisat_v_zhurnal("NEXUSBRO_BROWSER_LOCK shortcut=gtk-installed".into());
        })
        .is_err()
    {
        crate::zapisat_v_zhurnal("NEXUSBRO_BROWSER_LOCK shortcut=gtk-schedule-failed".into());
    }
}
#[cfg(target_os = "windows")]
thread_local! {
    // COM registration and removal stay on the WebView2/UI thread.
    static WINDOWS_SHORTCUTS:std::cell::RefCell<std::collections::HashMap<String,(webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2Controller,i64)>>=Default::default();
}
#[cfg(target_os = "windows")]
pub fn install_shortcut(window: &tauri::WebviewWindow) {
    let app = window.app_handle().clone();
    let label = window.label().to_owned();
    let cleanup_app = app.clone();
    let cleanup_label = label.clone();
    if window
        .with_webview(move |platform| {
            use webview2_com::{
                AcceleratorKeyPressedEventHandler, Microsoft::Web::WebView2::Win32::*,
            };
            use windows::Win32::UI::Input::KeyboardAndMouse::{
                GetKeyState, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT,
            };
            if !app
                .state::<BrowserLockState>()
                .shortcuts
                .lock()
                .unwrap()
                .insert(label.clone())
            {
                return;
            }
            let target = app.clone();
            let handler = AcceleratorKeyPressedEventHandler::create(Box::new(move |_, args| {
                let Some(args) = args else { return Ok(()) };
                unsafe {
                    let mut kind = COREWEBVIEW2_KEY_EVENT_KIND::default();
                    args.KeyEventKind(&mut kind)?;
                    if kind != COREWEBVIEW2_KEY_EVENT_KIND_KEY_DOWN
                        && kind != COREWEBVIEW2_KEY_EVENT_KIND_SYSTEM_KEY_DOWN
                    {
                        return Ok(());
                    }
                    let mut key = 0u32;
                    let mut physical = COREWEBVIEW2_PHYSICAL_KEY_STATUS::default();
                    args.VirtualKey(&mut key)?;
                    args.PhysicalKeyStatus(&mut physical)?;
                    let down = |code: u16| GetKeyState(i32::from(code)) < 0;
                    if (physical.ScanCode != 0x26 && key != 0x4c)
                        || !down(VK_CONTROL.0)
                        || !down(VK_SHIFT.0)
                        || down(VK_MENU.0)
                        || down(VK_LWIN.0)
                        || down(VK_RWIN.0)
                    {
                        return Ok(());
                    }
                    // WebView2 reserves Ctrl+Shift+L for Paste and Go. Mark it
                    // handled before doing anything that could call back into UI.
                    args.SetHandled(true)?;
                    crate::zapisat_v_zhurnal("NEXUSBRO_BROWSER_LOCK shortcut=webview2-key".into());
                    if !physical.WasKeyDown.as_bool() {
                        request_lock(target.clone());
                    }
                }
                Ok(())
            }));
            let controller = platform.controller();
            let mut token = 0i64;
            if unsafe { controller.add_AcceleratorKeyPressed(&handler, &mut token) }.is_ok() {
                WINDOWS_SHORTCUTS.with(|registrations| {
                    registrations
                        .borrow_mut()
                        .insert(label, (controller, token));
                });
                crate::zapisat_v_zhurnal(
                    "NEXUSBRO_BROWSER_LOCK shortcut=webview2-installed".into(),
                );
            } else {
                app.state::<BrowserLockState>()
                    .shortcuts
                    .lock()
                    .unwrap()
                    .remove(&label);
                crate::zapisat_v_zhurnal(
                    "NEXUSBRO_BROWSER_LOCK shortcut=webview2-install-failed".into(),
                );
            }
        })
        .is_err()
    {
        crate::zapisat_v_zhurnal("NEXUSBRO_BROWSER_LOCK shortcut=webview2-schedule-failed".into());
    }
    window.on_window_event(move |event| {
        if !matches!(event, tauri::WindowEvent::Destroyed) {
            return;
        }
        let app = cleanup_app.clone();
        let label = cleanup_label.clone();
        let scheduler = app.clone();
        let _ = scheduler.run_on_main_thread(move || {
            let registration = WINDOWS_SHORTCUTS
                .try_with(|registrations| registrations.borrow_mut().remove(&label))
                .ok()
                .flatten();
            if let Some((controller, token)) = registration {
                unsafe {
                    let _ = controller.remove_AcceleratorKeyPressed(token);
                }
            }
            if let Some(state) = app.try_state::<BrowserLockState>() {
                state.shortcuts.lock().unwrap().remove(&label);
            }
        });
    });
}
#[cfg(not(any(target_os = "linux", target_os = "windows")))]
pub fn install_shortcut(_window: &tauri::WebviewWindow) {}

pub fn setup(app: &tauri::AppHandle) {
    LOCK_GATE.configure(app.state::<Arc<VaultState>>().status().exists);
    for (label, window) in app.webview_windows() {
        if trusted(app, &label) {
            install_shortcut(&window);
        }
    }
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
    // Never pair an old requested bit with a newly registered lock generation.
    let (current, generation) = engine_state();
    if current != locked {
        return;
    }
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
#[cfg(target_os = "linux")]
fn outermost_x11_handles(handles: Vec<u32>) -> Vec<u32> {
    use x11rb::protocol::xproto::ConnectionExt;
    let Ok((connection, _)) = x11rb::connect(None) else {
        return handles;
    };
    let selected = handles
        .iter()
        .copied()
        .collect::<std::collections::HashSet<_>>();
    handles
        .into_iter()
        .filter(|handle| {
            let mut current = *handle;
            for _ in 0..128 {
                let Ok(cookie) = connection.query_tree(current) else {
                    return true;
                };
                let Ok(tree) = cookie.reply() else {
                    return true;
                };
                if selected.contains(&tree.parent) && tree.parent != current {
                    // The selected ancestor hides the whole subtree. Preserve the
                    // renderer's own mapped state so restoring its host paints it.
                    return false;
                }
                if tree.parent == 0 || tree.parent == tree.root || tree.parent == current {
                    return true;
                }
                current = tree.parent;
            }
            // Unknown ancestry stays hidden rather than risking exposed content.
            true
        })
        .collect()
}
fn hide_all(app: &tauri::AppHandle) {
    crate::youtube_fullscreen_parent::release_all(app);
    let state = app.state::<Sostoyanie>();
    let _windows = state.izmenenie_okon.lock().unwrap();
    state.dvizhki_skryty.store(true, Ordering::SeqCst);
    let mut handles = Vec::new();
    let mut pids = Vec::new();
    {
        let tabs = state.vkladki.lock().unwrap();
        for tab in tabs.values() {
            handles.extend(tab.okno);
            pids.extend(tab.proc.as_ref().and_then(|owner| owner.pid()));
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
    #[cfg(target_os = "linux")]
    let handles = outermost_x11_handles(handles);
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
    LOCK_GATE.enable();
    // A registered intent can already have set the global bit. Still perform
    // native hiding and vault locking; that bit is not a cleanup receipt.
    // Refuse every new browsing command and cover native content before waiting
    // for vault work. Even a failed vault operation cannot expose the browser.
    transition(true);
    engine_lock(app, true);
    hide_all(app);
    publish(app);
    crate::passwords::broadcast(app, false);
    crate::passwords::security_vault(app.clone(), app.state(), "lock".into(), None).await?;
    Ok(publish(app))
}
/// Called only from the native CEF keyboard event path, never a page console.
pub fn request_lock(app: tauri::AppHandle) {
    LOCK_GATE.register_lock();
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
    LOCK_GATE.register_lock();
    lock_inner(&app).await
}
#[tauri::command]
pub async fn browser_unlock(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
    password: String,
) -> Result<LockStatus, String> {
    let intent = LOCK_GATE.ticket();
    let password = zeroize::Zeroizing::new(password);
    validate(&app, &window)?;
    let serial = app.state::<BrowserLockState>();
    let _guard = serial.serial.lock().await;
    if !LOCK_GATE.is_current(intent) {
        return Ok(retain_locked(&app));
    }
    if !locked_global() {
        return Ok(status(&app));
    }
    if !app.state::<Arc<VaultState>>().status().exists {
        return Err("Защищённое хранилище недоступно; браузер остаётся заблокирован".into());
    }
    // Existing vault code does bounded Argon2id/AEAD verification, zeroizes the
    // submitted password and leaves main/backup unchanged for a wrong password.
    let vault = crate::passwords::security_vault(
        app.clone(),
        app.state(),
        "unlock".into(),
        Some(password.as_str().to_owned()),
    )
    .await;
    if !LOCK_GATE.is_current(intent) {
        return Ok(retain_locked(&app));
    }
    let vault = vault?;
    if !vault.unlocked {
        return Err("Браузер остаётся заблокирован".into());
    }
    LOCK_GATE.enable();
    Ok(finish_unlock(&app, intent))
}
fn retain_locked(app: &tauri::AppHandle) -> LockStatus {
    transition(true);
    app.state::<Arc<VaultState>>().lock();
    crate::passwords::broadcast(app, false);
    engine_lock(app, true);
    hide_all(app);
    publish(app)
}
fn visible_intent(intent: crate::browser_lock_intent::Ticket) -> bool {
    LOCK_GATE.is_current(intent) && !locked_global()
}
fn finish_unlock(app: &tauri::AppHandle, intent: crate::browser_lock_intent::Ticket) -> LockStatus {
    // Linearization only: no UI/window/CEF calls under the intent mutex.
    if !LOCK_GATE.try_unlock(intent) {
        return retain_locked(app);
    }
    engine_lock(&app, false);
    if !visible_intent(intent) {
        return retain_locked(app);
    }
    restore_all(&app);
    if !visible_intent(intent) {
        return retain_locked(app);
    }
    crate::okna_app::restore_windows(&app);
    if !visible_intent(intent) {
        return retain_locked(app);
    }
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
                if visible_intent(intent) {
                    let state = app.state::<Sostoyanie>();
                    let _ = crate::zapustit_vnutri(&app, &state, &id).await;
                    let _ = app.emit("vkladki-izmenilis", id);
                }
            });
        }
    }
    result
}

/// Recovery is available only to the trusted shell while sites remain hidden.
#[tauri::command]
pub async fn browser_vault_backups(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
) -> Result<Vec<crate::secure_vault::VaultBackup>, String> {
    validate(&app, &window)?;
    let serial = app.state::<BrowserLockState>();
    let _guard = serial.serial.lock().await;
    if !locked_global() {
        return Err("Сначала заблокируйте браузер".into());
    }
    let vault = Arc::clone(&app.state::<Arc<VaultState>>());
    tauri::async_runtime::spawn_blocking(move || vault.recovery_backups())
        .await
        .map_err(|_| "Не удалось прочитать список копий".to_string())?
}
async fn recover_vault(
    app: &tauri::AppHandle,
    window: &tauri::WebviewWindow,
    password: String,
    backup: Option<String>,
) -> Result<LockStatus, String> {
    let intent = LOCK_GATE.ticket();
    let password = zeroize::Zeroizing::new(password);
    validate(app, window)?;
    let serial = app.state::<BrowserLockState>();
    let _guard = serial.serial.lock().await;
    if !LOCK_GATE.is_current(intent) {
        retain_locked(app);
        return Err("Операция отменена новой блокировкой; хранилище не изменено".into());
    }
    if !locked_global() {
        return Err("Сначала заблокируйте браузер".into());
    }
    // Drain any pre-lock exchange before changing the vault. An old request
    // must never persist its account/session into the replacement vault.
    let sync = app.state::<crate::browser_sync::SyncState>();
    let _sync_guard = sync.serial.lock().await;
    if !LOCK_GATE.is_current(intent) {
        retain_locked(app);
        return Err("Операция отменена новой блокировкой; хранилище не изменено".into());
    }
    let vault = Arc::clone(&app.state::<Arc<VaultState>>());
    let operation = tauri::async_runtime::spawn_blocking(move || {
        let _history = crate::browser_sync::HISTORY
            .lock()
            .map_err(|_| "История восстановления занята".to_string())?;
        if !LOCK_GATE.is_current(intent) {
            return Err("Операция отменена новой блокировкой; хранилище не изменено".into());
        }
        match backup {
            Some(id) => vault.restore_recovery_backup(&id, &password),
            None => vault.reset_with_password(&password),
        }
    })
    .await
    .unwrap_or_else(|_| Err("Не удалось изменить хранилище".to_string()));
    if let Err(error) = operation {
        app.state::<Arc<VaultState>>().lock();
        crate::passwords::broadcast(app, false);
        return Err(error);
    }
    // SUCCESS means reset/restore is already committed. A newer lock only
    // revokes visibility. Return locked status, not an ambiguous failure that
    // could encourage retrying a successfully committed vault replacement.
    if !LOCK_GATE.is_current(intent) {
        return Ok(retain_locked(app));
    }
    if !app.state::<Arc<VaultState>>().status().unlocked {
        return Err("Хранилище не открыто; браузер остаётся заблокирован".into());
    }
    crate::passwords::broadcast(app, true);
    LOCK_GATE.enable();
    Ok(finish_unlock(app, intent))
}
#[tauri::command]
pub async fn browser_vault_reset(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
    password: String,
    confirmation: String,
) -> Result<LockStatus, String> {
    if confirmation != "СБРОСИТЬ" {
        return Err("Введите СБРОСИТЬ для подтверждения".into());
    }
    recover_vault(&app, &window, password, None).await
}
#[tauri::command]
pub async fn browser_vault_restore(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
    id: String,
    password: String,
) -> Result<LockStatus, String> {
    recover_vault(&app, &window, password, Some(id)).await
}
