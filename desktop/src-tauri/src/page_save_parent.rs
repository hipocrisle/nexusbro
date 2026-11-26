//! The native parent owns the page-save file dialog. The renderer never
//! supplies a path, HTML, or a URL; child requests are generation-pinned and
//! the picked path is re-checked against lock/ownership before it is sent
//! back. No paths, URLs, or page data are logged anywhere here.
use tauri::Manager;
use tauri_plugin_dialog::DialogExt;

fn valid_request(request: &str) -> bool {
    request.len() == 32 && request.bytes().all(|c| c.is_ascii_hexdigit())
}

/// Validation for the suggested default name only; the user-chosen path
/// from the native dialog is never restricted to this pattern.
fn valid_suggested_filename(name: &str) -> bool {
    name.len() <= 64
        && name.starts_with("page-")
        && name.ends_with(".mhtml")
        && name
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-'))
}

pub fn receive(
    app: &tauri::AppHandle,
    super_id: &str,
    engine: u64,
    data: &serde_json::Value,
) -> bool {
    if data["tip"] != "page-save" {
        return false;
    }
    if crate::browser_lock::locked_global() {
        return true;
    }
    let (Some(tab), Some(browser), Some(generation), Some(request), Some(filename)) = (
        data["id"].as_u64(),
        data["browser_id"].as_i64(),
        data["route_generation"].as_u64(),
        data["request"].as_str(),
        data["filename"].as_str(),
    ) else {
        return true;
    };
    if !valid_request(request) || !valid_suggested_filename(filename) {
        return true;
    }
    let label = data["label"]
        .as_str()
        .filter(|s| !s.is_empty())
        .unwrap_or("main")
        .to_owned();
    let state = app.state::<crate::Sostoyanie>();
    let matches = |session: &crate::sessiya::Sessiya| {
        session.aktivnaya == tab
            && session.vkladki.iter().any(|t| {
                t.id == tab
                    && t.browser_id as i64 == browser
                    && t.route_generation == generation
                    && !t.route_pending
            })
    };
    let current = {
        let all = state.vkladki.lock().unwrap();
        all.get(super_id)
            .is_some_and(|v| v.zapusk_id == engine && v.proc.is_some())
    };
    let admitted = if label == "main" {
        let active = state.aktivnaya.lock().unwrap().as_deref() == Some(super_id);
        active
            && state
                .vkladki
                .lock()
                .unwrap()
                .get(&super_id)
                .and_then(|v| v.obychnye.as_ref())
                .is_some_and(matches)
    } else {
        state.okna.0.lock().unwrap().get(&label).is_some_and(|w| {
            w.super_id == super_id
                && w.engine == engine
                && !w.closing
                && !w.hidden
                && w.sessiya.as_ref().is_some_and(matches)
        })
    };
    if !current || !admitted {
        return true;
    }
    let Some(owner) = app.get_webview_window(&label) else {
        return true;
    };
    let app = app.clone();
    let super_id = super_id.to_owned();
    let request = request.to_owned();
    let dialog = app
        .dialog()
        .file()
        .add_filter("MHTML", &["mhtml"])
        .set_file_name(filename)
        .set_parent(&owner);
    dialog.save_file(move |picked| {
        let path = picked
            .and_then(|p| p.into_path().ok())
            .filter(|p| p.is_absolute())
            .map(|p| p.to_string_lossy().into_owned());
        let state = app.state::<crate::Sostoyanie>();
        let channel = {
            let all = state.vkladki.lock().unwrap();
            all.get(&super_id)
                .filter(|v| v.zapusk_id == engine && v.proc.is_some())
                .and_then(|v| v.kanal.clone())
        };
        if let Some(channel) = channel {
            let matches = |session: &crate::sessiya::Sessiya| {
                session.aktivnaya == tab
                    && session.vkladki.iter().any(|t| {
                        t.id == tab
                            && t.browser_id as i64 == browser
                            && t.route_generation == generation
                            && !t.route_pending
                    })
            };
            let still_owned = if label == "main" {
                state.aktivnaya.lock().unwrap().as_deref() == Some(super_id.as_str())
                    && state
                        .vkladki
                        .lock()
                        .unwrap()
                        .get(&super_id)
                        .and_then(|v| v.obychnye.as_ref())
                        .is_some_and(matches)
            } else {
                state.okna.0.lock().unwrap().get(&label).is_some_and(|w| {
                    w.super_id == super_id
                        && w.engine == engine
                        && !w.closing
                        && w.sessiya.as_ref().is_some_and(matches)
                })
            };
            let path = if crate::browser_lock::locked_global() || !still_owned {
                None
            } else {
                path
            };
            let command = crate::sessiya::Komanda::PageSaveTarget {
                id: tab,
                request,
                path,
            };
            let command = if label == "main" {
                command
            } else {
                crate::sessiya::Komanda::OknoKomanda {
                    label,
                    komanda: Box::new(command),
                }
            };
            let _ = channel.try_send(command);
        }
    });
    true
}
