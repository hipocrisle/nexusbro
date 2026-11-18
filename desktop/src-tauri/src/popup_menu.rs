//! Themed HTML popup in an owned window: CEF remains visible underneath.
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex,
    },
    time::Duration,
};
use tauri::{Manager, PhysicalPosition, PhysicalSize, WebviewWindow, WindowEvent};
use tokio::sync::oneshot;

#[derive(Clone, Deserialize, Serialize)]
pub struct Entry {
    text: String,
    #[serde(default = "yes")]
    enabled: bool,
}
fn yes() -> bool {
    true
}
#[derive(Clone, Deserialize, Serialize)]
pub struct Theme {
    colors: HashMap<String, [u8; 3]>,
    font_family: String,
    font_size: f64,
}
#[derive(Clone, Serialize)]
pub struct MenuData {
    items: Vec<Option<Entry>>,
    theme: Theme,
}
struct Pending {
    owner: String,
    data: MenuData,
    ready: bool,
    seen_focus: bool,
    sender: oneshot::Sender<Option<usize>>,
}
#[derive(Default)]
pub struct PopupMenuState {
    menus: Mutex<HashMap<String, Pending>>,
}
const COLOR_KEYS: [&str; 7] = [
    "--fon", "--fon2", "--fon3", "--tekst", "--tiho", "--kraj", "--akcent",
];

fn finish(app: &tauri::AppHandle, label: &str, chosen: Option<usize>, focus_owner: bool) {
    let pending = app
        .state::<PopupMenuState>()
        .menus
        .lock()
        .unwrap()
        .remove(label);
    if let Some(pending) = pending {
        if let Some(popup) = app.get_webview_window(label) {
            let _ = popup.destroy();
        }
        if focus_owner {
            if let Some(owner) = app.get_webview_window(&pending.owner) {
                let _ = owner.set_focus();
            }
        }
        let _ = pending.sender.send(chosen);
    }
}
fn valid_theme(theme: &Theme) -> bool {
    theme.font_size.is_finite()
        && (10.0..=24.0).contains(&theme.font_size)
        && !theme.font_family.is_empty()
        && theme.font_family.len() <= 240
        && theme
            .font_family
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || " ,-'\"_".contains(c))
        && theme.colors.len() == COLOR_KEYS.len()
        && COLOR_KEYS.iter().all(|k| theme.colors.contains_key(*k))
}
fn registered(app: &tauri::AppHandle, window: &WebviewWindow) -> bool {
    window.label() == "main"
        || app
            .state::<crate::Sostoyanie>()
            .okna
            .0
            .lock()
            .unwrap()
            .contains_key(window.label())
}

#[tauri::command]
pub async fn popup_menu_open(
    app: tauri::AppHandle,
    window: WebviewWindow,
    items: Vec<Option<Entry>>,
    theme: Theme,
    x: f64,
    y: f64,
) -> Result<Option<usize>, String> {
    if !registered(&app, &window) {
        return Err("Окно не может открыть меню".into());
    }
    if items.is_empty()
        || items.len() > 80
        || !items.iter().any(|e| e.is_some())
        || items.iter().flatten().any(|e| {
            e.text.is_empty()
                || e.text.chars().count() > 300
                || e.text.chars().any(|c| c.is_control() && c != '\t')
        })
        || !valid_theme(&theme)
        || !x.is_finite()
        || !y.is_finite()
    {
        return Err("Неверные параметры меню".into());
    }
    let owner = window.label().to_string();
    let previous = app
        .state::<PopupMenuState>()
        .menus
        .lock()
        .unwrap()
        .iter()
        .filter(|(_, p)| p.owner == owner)
        .map(|(k, _)| k.clone())
        .collect::<Vec<_>>();
    for label in previous {
        finish(&app, &label, None, false);
    }
    let scale = window
        .scale_factor()
        .map_err(|_| "Не удалось определить масштаб окна")?;
    let origin = window
        .inner_position()
        .map_err(|_| "Не удалось определить положение окна")?;
    let size = window
        .inner_size()
        .map_err(|_| "Не удалось определить размер окна")?;
    let anchor_x = origin.x as f64 + (x * scale).clamp(0., size.width as f64);
    let anchor_y = origin.y as f64 + (y * scale).clamp(0., size.height as f64);
    let monitor = window
        .monitor_from_point(anchor_x, anchor_y)
        .ok()
        .flatten()
        .or_else(|| window.current_monitor().ok().flatten())
        .ok_or("Не найден экран для меню")?;
    let work_area = monitor.work_area();
    let has_work_area = work_area.size.width > 0 && work_area.size.height > 0;
    let area_position = if has_work_area {
        work_area.position
    } else {
        *monitor.position()
    };
    let area_size = if has_work_area {
        work_area.size
    } else {
        *monitor.size()
    };
    let margin = 6.0_f64.min(area_size.width.min(area_size.height) as f64 / 4.);
    let monitor_scale = monitor.scale_factor().max(0.5);
    let max_w = (area_size.width as f64 - 2. * margin).max(1.);
    let max_h = (area_size.height as f64 - 2. * margin).max(1.);
    let longest = items
        .iter()
        .flatten()
        .map(|e| e.text.chars().count())
        .max()
        .unwrap_or(16) as f64;
    let width =
        ((longest * theme.font_size * 0.57 + 48.).clamp(230., 460.) * monitor_scale).min(max_w);
    let height = ((items
        .iter()
        .map(|e| {
            if e.is_some() {
                theme.font_size + 20.
            } else {
                9.
            }
        })
        .sum::<f64>()
        + 14.)
        * monitor_scale)
        .min(max_h);
    let left = anchor_x.clamp(
        area_position.x as f64 + margin,
        area_position.x as f64 + margin + (area_size.width as f64 - 2. * margin - width).max(0.),
    );
    let top = anchor_y.clamp(
        area_position.y as f64 + margin,
        area_position.y as f64 + margin + (area_size.height as f64 - 2. * margin - height).max(0.),
    );
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let label = format!("menu-{}", NEXT.fetch_add(1, Ordering::Relaxed));
    let background = theme.colors["--fon2"];
    let (sender, mut receiver) = oneshot::channel();
    app.state::<PopupMenuState>().menus.lock().unwrap().insert(
        label.clone(),
        Pending {
            owner: owner.clone(),
            data: MenuData { items, theme },
            ready: false,
            seen_focus: false,
            sender,
        },
    );
    let builder = tauri::WebviewWindowBuilder::new(
        &app,
        &label,
        tauri::WebviewUrl::App("index.html?popup=menu".into()),
    )
    .title("NexusBro — меню")
    // GTK realizes a non-resizable window using its initial size request.
    // A post-build resize before mapping can leave the default 800x600 child.
    .inner_size(width / monitor_scale, height / monitor_scale)
    .position(left / monitor_scale, top / monitor_scale)
    .decorations(false)
    .resizable(false)
    .minimizable(false)
    .maximizable(false)
    .skip_taskbar(true)
    .shadow(true)
    .visible(false)
    .focused(false)
    .disable_drag_drop_handler()
    .background_color(tauri::window::Color(
        background[0],
        background[1],
        background[2],
        255,
    ));
    let popup = match builder.parent(&window).and_then(|b| b.build()) {
        Ok(popup) => popup,
        Err(_) => {
            finish(&app, &label, None, false);
            return Err("Не удалось открыть меню NexusBro".into());
        }
    };
    if popup
        .set_position(PhysicalPosition::new(
            left.round() as i32,
            top.round() as i32,
        ))
        .is_err()
        || popup
            .set_size(PhysicalSize::new(
                width.round() as u32,
                height.round() as u32,
            ))
            .is_err()
    {
        finish(&app, &label, None, false);
        return Err("Не удалось разместить меню".into());
    }
    eprintln!(
        "NEXUSBRO_POPUP stage=created requested_width={} requested_height={}",
        width.round() as u32,
        height.round() as u32
    );
    let events_app = app.clone();
    let events_label = label.clone();
    popup.on_window_event(move |event| match event {
        WindowEvent::Focused(true) => {
            eprintln!("NEXUSBRO_POPUP stage=focus focused=true");
            if let Some(pending) = events_app
                .state::<PopupMenuState>()
                .menus
                .lock()
                .unwrap()
                .get_mut(&events_label)
            {
                pending.seen_focus = true;
            }
        }
        WindowEvent::Focused(false) => {
            eprintln!("NEXUSBRO_POPUP stage=focus focused=false");
            let ready = events_app
                .state::<PopupMenuState>()
                .menus
                .lock()
                .unwrap()
                .get(&events_label)
                .is_some_and(|p| p.ready && p.seen_focus);
            if ready {
                finish(&events_app, &events_label, None, false);
            }
        }
        WindowEvent::CloseRequested { .. } | WindowEvent::Destroyed => {
            finish(&events_app, &events_label, None, false)
        }
        _ => {}
    });
    let result = tokio::time::timeout(Duration::from_secs(120), async {
        loop {
            tokio::select! {
                result = &mut receiver => break result.unwrap_or(None),
                _ = tokio::time::sleep(Duration::from_millis(250)) => {
                    if app.get_webview_window(&owner).is_none() { break None; }
                }
            }
        }
    })
    .await
    .unwrap_or(None);
    finish(&app, &label, None, false);
    Ok(result)
}

#[tauri::command]
pub fn popup_menu_data(app: tauri::AppHandle, window: WebviewWindow) -> Result<MenuData, String> {
    let state = app.state::<PopupMenuState>();
    let result = state
        .menus
        .lock()
        .unwrap()
        .get(window.label())
        .map(|p| p.data.clone())
        .ok_or_else(|| "Меню уже закрыто".into());
    result
}

#[tauri::command]
pub async fn popup_menu_ready(app: tauri::AppHandle, window: WebviewWindow) -> Result<(), String> {
    {
        let state = app.state::<PopupMenuState>();
        let mut menus = state.menus.lock().unwrap();
        let pending = menus.get_mut(window.label()).ok_or("Меню уже закрыто")?;
        if app.get_webview_window(&pending.owner).is_none() {
            return Err("Родительское окно закрыто".into());
        }
        pending.ready = true;
    }
    let (sender, receiver) = oneshot::channel();
    let ui = window.clone();
    let ui_app = app.clone();
    let generation = crate::browser_lock::generation();
    window
        .run_on_main_thread(move || {
            if crate::browser_lock::locked_global()
                || crate::browser_lock::generation() != generation
                || !ui_app
                    .state::<PopupMenuState>()
                    .menus
                    .lock()
                    .unwrap()
                    .contains_key(ui.label())
            {
                let _ = sender.send(Err("Меню уже закрыто".to_string()));
                return;
            }
            let shown = (|| -> tauri::Result<()> {
                ui.show()?;
                #[cfg(target_os = "linux")]
                {
                    use gtk::prelude::*;
                    // Tao's show/focus are separate GLib requests, and focus ignores
                    // a still-hidden GTK window. Map children now before requesting
                    // focus, rather than relying on timing or a hidden-frame RAF.
                    let gtk = ui.gtk_window()?;
                    gtk.show_all();
                    gtk.check_resize();
                    gtk.present();
                    gtk.queue_resize();
                    gtk.queue_draw();
                    gtk.display().sync();
                    if let Some(handle) = crate::nash_descriptor(&ui) {
                        if crate::okna::sistema::fokus_obolochke(handle).is_err() {
                            eprintln!("NEXUSBRO_POPUP stage=x11-focus accepted=false");
                        }
                    }
                    ui.with_webview(|platform| {
                        let view = platform.inner();
                        view.show_all();
                        view.queue_resize();
                        view.queue_draw();
                        view.grab_focus();
                        let bounds = view.allocation();
                        eprintln!(
                            "NEXUSBRO_POPUP stage=gtk-content width={} height={}",
                            bounds.width(),
                            bounds.height()
                        );
                    })?;
                }
                ui.set_focus()?;
                let webview: &tauri::Webview = ui.as_ref();
                webview.set_focus()?;
                if let Ok(size) = ui.inner_size() {
                    eprintln!(
                        "NEXUSBRO_POPUP stage=ready width={} height={}",
                        size.width, size.height
                    );
                }
                Ok(())
            })();
            let _ = sender.send(shown.map_err(|_| "Не удалось показать меню".to_string()));
        })
        .map_err(|_| "Не удалось показать меню")?;
    tokio::time::timeout(Duration::from_secs(5), receiver)
        .await
        .map_err(|_| "Окно меню не ответило")?
        .map_err(|_| "Окно меню закрыто")?
}

#[tauri::command]
pub fn popup_menu_choose(
    app: tauri::AppHandle,
    window: WebviewWindow,
    index: usize,
) -> Result<(), String> {
    {
        let state = app.state::<PopupMenuState>();
        let menus = state.menus.lock().unwrap();
        let pending = menus.get(window.label()).ok_or("Меню уже закрыто")?;
        if !pending
            .data
            .items
            .get(index)
            .and_then(|e| e.as_ref())
            .is_some_and(|e| e.enabled)
        {
            return Err("Пункт меню недоступен".into());
        }
    }
    finish(&app, window.label(), Some(index), true);
    Ok(())
}
#[tauri::command]
pub fn popup_menu_dismiss(
    app: tauri::AppHandle,
    window: WebviewWindow,
    restore_focus: Option<bool>,
) -> Result<(), String> {
    if !app
        .state::<PopupMenuState>()
        .menus
        .lock()
        .unwrap()
        .contains_key(window.label())
    {
        return Err("Меню уже закрыто".into());
    }
    finish(&app, window.label(), None, restore_focus.unwrap_or(false));
    Ok(())
}

/// Close every menu before locking or otherwise hiding the browser UI.
pub fn dismiss_all(app: &tauri::AppHandle) {
    let labels = app
        .state::<PopupMenuState>()
        .menus
        .lock()
        .unwrap()
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    for label in labels {
        finish(app, &label, None, false);
    }
}

/// The popup may request the same global lock, but has no general browser IPC.
#[tauri::command]
pub fn popup_menu_lock(app: tauri::AppHandle, window: WebviewWindow) -> Result<(), String> {
    if !app
        .state::<PopupMenuState>()
        .menus
        .lock()
        .unwrap()
        .contains_key(window.label())
    {
        return Err("Меню уже закрыто".into());
    }
    crate::browser_lock::request_lock(app);
    Ok(())
}
