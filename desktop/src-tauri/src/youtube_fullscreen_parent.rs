//! Native channel -> actual owner window fullscreen. No renderer-facing command.
use crate::Sostoyanie;
use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock},
};
use tauri::{Emitter, Manager};
const CAP: usize = 128;
#[derive(Clone, PartialEq, Eq)]
struct Source {
    super_id: String,
    engine: u64,
    tab: u64,
}
struct Entry {
    source: Source,
    changed_os: bool,
}
#[derive(Default)]
struct Registry {
    windows: HashMap<String, Entry>,
    seen: HashMap<String, (u64, u64)>,
}
impl Registry {
    fn accept(&mut self, id: &str, engine: u64, sequence: u64) -> bool {
        if sequence == 0 || (!self.seen.contains_key(id) && self.seen.len() >= CAP) {
            return false;
        }
        if self.seen.get(id).is_some_and(|(old_engine, old_sequence)| {
            engine < *old_engine || (engine == *old_engine && sequence <= *old_sequence)
        }) {
            return false;
        }
        self.seen.insert(id.to_owned(), (engine, sequence));
        true
    }
}
fn registry() -> &'static Mutex<Registry> {
    static R: OnceLock<Mutex<Registry>> = OnceLock::new();
    R.get_or_init(Default::default)
}
fn selected(app: &tauri::AppHandle, source: &Source, label: &str) -> bool {
    if crate::browser_lock::locked_global() {
        return false;
    }
    let s: tauri::State<Sostoyanie> = app.state();
    // Never acquire engine and additional-window mutexes simultaneously.
    let main = {
        let tabs = s.vkladki.lock().unwrap();
        let Some(v) = tabs
            .get(&source.super_id)
            .filter(|v| v.zapusk_id == source.engine && v.proc.is_some())
        else {
            return false;
        };
        v.obychnye.as_ref().is_some_and(|session| {
            session.aktivnaya == source.tab && session.vkladki.iter().any(|t| t.id == source.tab)
        })
    };
    if label == "main" {
        return main && s.aktivnaya.lock().unwrap().as_deref() == Some(source.super_id.as_str());
    }
    let valid = s.okna.0.lock().unwrap().get(label).is_some_and(|w| {
        !w.hidden
            && !w.closing
            && w.super_id == source.super_id
            && w.engine == source.engine
            && w.sessiya.as_ref().is_some_and(|session| {
                session.aktivnaya == source.tab
                    && session.vkladki.iter().any(|t| t.id == source.tab)
            })
    });
    valid
}
fn label_for(app: &tauri::AppHandle, source: &Source) -> Option<String> {
    if selected(app, source, "main") {
        return Some("main".into());
    }
    let s: tauri::State<Sostoyanie> = app.state();
    let labels: Vec<_> = s.okna.0.lock().unwrap().keys().cloned().collect();
    labels
        .into_iter()
        .find(|label| selected(app, source, label))
}
fn exit(app: &tauri::AppHandle, label: &str) {
    let entry = registry()
        .lock()
        .ok()
        .and_then(|mut r| r.windows.remove(label));
    let Some(entry) = entry else { return };
    let Some(window) = app.get_webview_window(label) else {
        return;
    };
    // Previously fullscreen windows remain fullscreen. Only our own transition
    // is reversed. Failed OS restoration retains ownership for reconciliation.
    if entry.changed_os && window.set_fullscreen(false).is_err() {
        if let Ok(mut r) = registry().lock() {
            r.windows.insert(label.into(), entry);
        }
        return;
    }
    let _ = window.emit(
        "youtube-fullscreen-changed",
        serde_json::json!({"active":false}),
    );
}
/// Call from the generation-bound native engine reader, never generic JS IPC.
pub fn receive(
    app: &tauri::AppHandle,
    super_id: &str,
    engine: u64,
    data: &serde_json::Value,
) -> bool {
    if data["tip"] != "youtube-fullscreen" {
        return false;
    }
    let (Some(tab), Some(sequence), Some(active)) = (
        data["id"].as_u64(),
        data["sequence"].as_u64(),
        data["fullscreen"].as_bool(),
    ) else {
        return true;
    };
    if sequence == 0 {
        return true;
    }
    let source = Source {
        super_id: super_id.into(),
        engine,
        tab,
    };
    let app_ui = app.clone();
    let _ = app.run_on_main_thread(move || {
        {
            let Ok(mut r) = registry().lock() else { return };
            if !r.accept(&source.super_id, source.engine, sequence) {
                return;
            }
        }
        if !active {
            let labels: Vec<_> = registry()
                .lock()
                .map(|r| {
                    r.windows
                        .iter()
                        .filter(|(_, e)| e.source == source)
                        .map(|(l, _)| l.clone())
                        .collect()
                })
                .unwrap_or_default();
            for label in labels {
                exit(&app_ui, &label);
            }
            return;
        }
        let Some(label) = label_for(&app_ui, &source) else {
            return;
        };
        let Some(window) = app_ui.get_webview_window(&label) else {
            return;
        };
        let Ok(prior) = window.is_fullscreen() else {
            return;
        };
        {
            let Ok(r) = registry().lock() else { return };
            if r.windows.contains_key(&label) || r.windows.len() >= CAP {
                return;
            }
        }
        if !selected(&app_ui, &source, &label) {
            return;
        }
        if !prior && window.set_fullscreen(true).is_err() {
            return;
        }
        if let Ok(mut r) = registry().lock() {
            r.windows.insert(
                label,
                Entry {
                    source,
                    changed_os: !prior,
                },
            );
        }
        let _ = window.emit(
            "youtube-fullscreen-changed",
            serde_json::json!({"active":true}),
        );
    });
    true
}
/// Call after selection, navigation, window attachment, or engine lifecycle changes.
pub fn reconcile(app: &tauri::AppHandle) {
    let app_ui = app.clone();
    let _ = app.run_on_main_thread(move || {
        let entries: Vec<_> = registry()
            .lock()
            .map(|r| {
                r.windows
                    .iter()
                    .map(|(l, e)| (l.clone(), e.source.clone()))
                    .collect()
            })
            .unwrap_or_default();
        for (label, source) in entries {
            if !selected(&app_ui, &source, &label) {
                exit(&app_ui, &label);
            }
        }
    });
}
pub fn release_super(app: &tauri::AppHandle, id: &str, engine: u64) {
    let app_ui = app.clone();
    let id = id.to_owned();
    let _ = app.run_on_main_thread(move || {
        let labels: Vec<_> = registry()
            .lock()
            .map(|r| {
                r.windows
                    .iter()
                    .filter(|(_, e)| e.source.super_id == id && e.source.engine == engine)
                    .map(|(l, _)| l.clone())
                    .collect()
            })
            .unwrap_or_default();
        for label in labels {
            exit(&app_ui, &label);
        }
    });
}
pub fn release_all(app: &tauri::AppHandle) {
    let app_ui = app.clone();
    let _ = app.run_on_main_thread(move || {
        let labels: Vec<_> = registry()
            .lock()
            .map(|r| r.windows.keys().cloned().collect())
            .unwrap_or_default();
        for label in labels {
            exit(&app_ui, &label);
        }
    });
}
/// Before an independent owner/user fullscreen operation, relinquish restoration.
/// The manual operation's own frontend event must restore its chrome policy.
pub fn relinquish(app: &tauri::AppHandle, label: &str) {
    let label = label.to_owned();
    let _ = app.run_on_main_thread(move || {
        if let Ok(mut r) = registry().lock() {
            r.windows.remove(&label);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn delayed_enter_cannot_override_exit_or_new_engine() {
        let mut registry = Registry::default();
        assert!(registry.accept("super", 5, 20));
        assert!(!registry.accept("super", 5, 19));
        assert!(!registry.accept("super", 5, 20));
        assert!(registry.accept("super", 6, 1));
        assert!(!registry.accept("super", 5, 999));
    }
    #[test]
    fn engine_restarts_do_not_exhaust_bounded_registry() {
        let mut registry = Registry::default();
        for engine in 1..1000 {
            assert!(registry.accept("super", engine, 1));
        }
        assert_eq!(registry.seen.len(), 1);
        for n in 1..CAP {
            assert!(registry.accept(&format!("super-{n}"), 1, 1));
        }
        assert!(!registry.accept("overflow", 1, 1));
        assert!(registry.accept("super", 1001, 1));
        assert!(!registry.accept("super", 1002, 0));
    }
}
