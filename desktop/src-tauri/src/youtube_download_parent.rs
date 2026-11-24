//! The native parent owns file selection. The renderer never supplies a save
//! path, an executable, or a network URL. Child requests are generation-pinned.
use tauri::Manager;
use crate::youtube_download_diagnostics::{self as trace, Stage, Reason};
use tauri_plugin_dialog::DialogExt;
pub fn receive(
    app: &tauri::AppHandle,
    super_id: &str,
    engine: u64,
    data: &serde_json::Value,
) -> bool {
    if data["tip"] != "youtube-save" {
        return false;
    }
    if let Some(tab)=data["id"].as_u64(){trace::parent(app,super_id,tab,Stage::ParentReceived,Reason::None);}
    if crate::browser_lock::locked_global() {
        if let Some(tab)=data["id"].as_u64(){trace::parent(app,super_id,tab,Stage::ParentReceived,Reason::LockedOrClosing);}
        return true;
    }
    let (Some(tab), Some(browser), Some(generation), Some(request), Some(filename)) = (
        data["id"].as_u64(),
        data["browser_id"].as_i64(),
        data["route_generation"].as_u64(),
        data["request"].as_str(),
        data["filename"].as_str(),
    ) else {
        if let Some(tab)=data["id"].as_u64(){trace::parent(app,super_id,tab,Stage::ParentReceived,Reason::InvalidRequest);}
        return true;
    };
    if request.len() != 32
        || !request.bytes().all(|c| c.is_ascii_hexdigit())
        || filename.len() > 200
        || !filename.starts_with("YouTube-")
        || !filename.ends_with(".webm")
        || !filename
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-'))
    {
        trace::parent(app,super_id,tab,Stage::ParentReceived,Reason::InvalidRequest);
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
                .get(super_id)
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
        trace::parent(app,super_id,tab,Stage::ParentReceived,if !current{Reason::CurrentEngineMismatch}else{Reason::SelectedSessionMismatch});
        return true;
    }
    let Some(owner) = app.get_webview_window(&label) else {
        trace::parent(app,super_id,tab,Stage::ParentReceived,Reason::OwnerMissing);
        return true;
    };
    trace::parent(app,super_id,tab,Stage::ParentAdmitted,Reason::None);
    let app = app.clone();
    let super_id = super_id.to_owned();
    let request = request.to_owned();
    let dialog = app
        .dialog()
        .file()
        .add_filter("WebM", &["webm"])
        .set_file_name(filename)
        .set_parent(&owner);
    trace::parent(&app,&super_id,tab,Stage::DialogDispatch,Reason::None);
    dialog.save_file(move |picked| {
        trace::parent(&app,&super_id,tab,Stage::DialogReturned,if picked.is_some(){Reason::Picked}else{Reason::Cancelled});
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
        if channel.is_none(){trace::parent(&app,&super_id,tab,Stage::DialogReturned,Reason::ChannelMissing);}
        if let Some(channel) = channel {
            let matches = |session: &crate::sessiya::Sessiya| {
                session.vkladki.iter().any(|t| {
                    t.id == tab
                        && t.browser_id as i64 == browser
                        && t.route_generation == generation
                        && !t.route_pending
                })
            };
            let still_owned = if label == "main" {
                state
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
            let command = crate::sessiya::Komanda::YoutubeDownloadTarget {
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
            let sent=channel.try_send(command);
            trace::parent(&app,&super_id,tab,Stage::DialogReturned,if sent.is_ok(){Reason::ChannelSent}else{Reason::ChannelMissing});
        }
    });
    true
}
