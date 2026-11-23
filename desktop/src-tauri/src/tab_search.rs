//! Read-only snapshots; searching never launches an engine or visits a page.
use crate::{
    sessiya::{Komanda, Sessiya},
    Sostoyanie,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tauri::{Emitter, Manager, State};
#[derive(Clone, Serialize, Deserialize)]
pub struct Entry {
    pub super_id: String,
    pub super_name: String,
    pub window: String,
    pub tab_id: Option<u64>,
    pub title: String,
    pub address: String,
    pub sleeping: bool,
    #[serde(default)]
    pub route_label: String,
    pub index: usize,
    pub route: crate::tab_routes::RouteChoice,
    #[serde(default)]
    pub engine_generation: u64,
    #[serde(default)]
    pub view_generation: u64,
    #[serde(default)]
    pub route_generation: u64,
    #[serde(default)]
    pub cold: bool,
    #[serde(default)]
    pub profile_guard: String,
}
const CHANGED: &str = "Список вкладок изменился. Повторите поиск.";
pub(crate) fn profile_guard(n: &crate::Nastrojki) -> String {
    let raw = serde_json::json!([
        n.id,
        n.proxy,
        n.proxy_uzel,
        n.adres,
        n.pustaya,
        n.privatnaya
    ])
    .to_string();
    Sha256::digest(raw.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
/// Snapshot IDs are authoritative until a proved cold-view recreation. Identical
/// URLs never authorize selecting a neighbouring/reordered live tab.
pub(crate) fn validate(session: &Sessiya, entry: &Entry) -> Result<(), String> {
    let row = session.vkladki.get(entry.index).ok_or(CHANGED)?;
    if Some(row.id) != entry.tab_id
        || row.privatnaya
        || row.adres != entry.address
        || row.route != entry.route
        || (!entry.cold && row.route_generation != entry.route_generation)
    {
        return Err(CHANGED.into());
    }
    Ok(())
}
fn rows(
    out: &mut Vec<Entry>,
    id: &str,
    name: &str,
    label: &str,
    session: &Sessiya,
    cold: bool,
    engine: u64,
    view: u64,
    connections: &std::collections::HashMap<String, String>,
    profile: &str,
) {
    for (index, t) in session.vkladki.iter().enumerate() {
        if t.privatnaya {
            continue;
        }
        if out.len() >= 10_000 {
            break;
        }
        out.push(Entry {
            super_id: id.into(),
            super_name: name.into(),
            window: label.into(),
            tab_id: Some(t.id),
            title: t.zagolovok.clone(),
            address: t.adres.clone(),
            sleeping: cold || t.otlozhena,
            index,
            route_label: match &t.route {
                crate::tab_routes::RouteChoice::Inherit => "Путь супервкладки".into(),
                crate::tab_routes::RouteChoice::Direct => "Прямое соединение".into(),
                crate::tab_routes::RouteChoice::Connection { id } => connections
                    .get(id.as_str())
                    .cloned()
                    .unwrap_or_else(|| "Подключение недоступно".into()),
                _ => "Путь недоступен".into(),
            },
            route: t.route.clone(),
            engine_generation: engine,
            view_generation: view,
            route_generation: t.route_generation,
            cold,
            profile_guard: profile.into(),
        });
    }
}
fn unlocked() -> Result<(), String> {
    if crate::browser_lock::locked_global() {
        Err("Браузер заблокирован".into())
    } else {
        Ok(())
    }
}
#[tauri::command]
pub async fn tab_search_snapshot(s: State<'_, Sostoyanie>) -> Result<Vec<Entry>, String> {
    unlocked()?;
    let connections = crate::nastrojki::zagruzit()
        .vyhody
        .into_iter()
        .map(|v| (v.id, v.nazvanie))
        .collect();
    let definitions = {
        let all = s.vkladki.lock().map_err(|_| "Список занят")?;
        all.values()
            .filter(|v| !v.nastrojki.privatnaya && !crate::privatnost::eto_id(&v.nastrojki.id))
            .map(|v| {
                (
                    v.nastrojki.clone(),
                    v.obychnye.clone(),
                    v.proc.is_none() && v.zapas_pid.is_none(),
                    v.zapusk_id,
                )
            })
            .collect::<Vec<_>>()
    };
    let mut out = Vec::new();
    for (n, live, cold, engine) in definitions {
        let session = if !cold && live.is_some() {
            live.unwrap()
        } else if !n.pustaya {
            let id = n.id.clone();
            tauri::async_runtime::spawn_blocking(move || {
                Sessiya::zagruzit(&crate::brauzer::papka_profilya(&id))
            })
            .await
            .map_err(|_| "Не удалось прочитать сессию")?
        } else {
            Sessiya::novaya(&n.adres)
        };
        unlocked()?;
        rows(
            &mut out,
            &n.id,
            &n.nazvanie,
            "main",
            &session,
            cold,
            engine,
            engine,
            &connections,
            &profile_guard(&n),
        );
    }
    let names = s
        .vkladki
        .lock()
        .map_err(|_| "Список занят")?
        .iter()
        .map(|(id, v)| {
            (
                id.clone(),
                (
                    v.nastrojki.nazvanie.clone(),
                    v.nastrojki.privatnaya,
                    v.zapusk_id,
                    profile_guard(&v.nastrojki),
                ),
            )
        })
        .collect::<std::collections::HashMap<_, _>>();
    {
        let windows = s.okna.0.lock().map_err(|_| "Список окон занят")?;
        for (label, w) in windows.iter() {
            if w.privatnaya || w.closing {
                continue;
            }
            let views = std::iter::once((
                &w.super_id,
                w.sessiya.as_ref(),
                w.engine,
                w.hwnd.is_some_and(crate::okna::sistema::okno_zhivo),
                w.recovery_lazy,
            ))
            .chain(w.others.iter().map(|(id, v)| {
                (
                    id,
                    v.sessiya.as_ref(),
                    v.engine,
                    v.hwnd.is_some_and(crate::okna::sistema::okno_zhivo),
                    false,
                )
            }));
            for (id, session, view, attached, lazy) in views {
                if crate::privatnost::eto_id(id) {
                    continue;
                }
                if let (Some((name, false, engine, profile)), Some(session)) =
                    (names.get(id), session)
                {
                    rows(
                        &mut out,
                        id,
                        name,
                        label,
                        session,
                        lazy || !attached || view != *engine,
                        *engine,
                        view,
                        &connections,
                        profile,
                    );
                }
            }
        }
    }
    for n in crate::zagruzit_sohranennye() {
        if out.len() >= 10_000 {
            break;
        }
        if !n.privatnaya && !crate::privatnost::eto_id(&n.id) && !names.contains_key(&n.id) {
            out.push(Entry {
                super_id: n.id.clone(),
                super_name: n.nazvanie.clone(),
                window: "main".into(),
                tab_id: None,
                title: n.nazvanie.clone(),
                address: String::new(),
                sleeping: true,
                index: 0,
                route_label: String::new(),
                route: crate::tab_routes::RouteChoice::Inherit,
                engine_generation: 0,
                view_generation: 0,
                route_generation: 0,
                cold: true,
                profile_guard: profile_guard(&n),
            });
        }
    }
    out.sort_by(|a, b| {
        (&a.super_name, &a.window, a.index).cmp(&(&b.super_name, &b.window, b.index))
    });
    unlocked()?;
    Ok(out)
}
fn live_session(s: &Sostoyanie, entry: &Entry, engine: u64) -> Result<Option<Sessiya>, String> {
    {
        let map = s.vkladki.lock().map_err(|_| "Список занят")?;
        if !map
            .get(&entry.super_id)
            .is_some_and(|v| v.zapusk_id == engine && !v.nastrojki.privatnaya)
        {
            return Err(CHANGED.into());
        }
    }
    if entry.window == "main" {
        return Ok(s
            .vkladki
            .lock()
            .map_err(|_| "Список занят")?
            .get(&entry.super_id)
            .and_then(|v| v.obychnye.clone()));
    }
    let windows = s.okna.0.lock().map_err(|_| "Список окон занят")?;
    let w = windows
        .get(&entry.window)
        .filter(|w| !w.closing && !w.privatnaya && w.super_id == entry.super_id)
        .ok_or(CHANGED)?;
    Ok(if w.engine == engine && w.hwnd.is_some() {
        w.sessiya.clone()
    } else {
        None
    })
}
#[tauri::command]
pub async fn tab_search_activate(
    app: tauri::AppHandle,
    s: State<'_, Sostoyanie>,
    entry: Entry,
) -> Result<(), String> {
    static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    let _serial = SERIAL.lock().await;
    unlocked()?;
    if crate::privatnost::eto_id(&entry.super_id) {
        return Err("Приватная сессия недоступна в общем поиске".into());
    }
    let target = app
        .get_webview_window(&entry.window)
        .ok_or("Окно уже закрыто")?;
    if entry.tab_id.is_none() {
        if entry.window != "main" {
            return Err(CHANGED.into());
        }
        let saved = crate::zagruzit_sohranennye()
            .into_iter()
            .find(|n| n.id == entry.super_id)
            .ok_or("Супер-вкладка уже удалена")?;
        if profile_guard(&saved) != entry.profile_guard {
            return Err(CHANGED.into());
        }
        if saved.privatnaya {
            return Err("Приватная сессия недоступна в общем поиске".into());
        }
        let result = crate::vosstanovit(
            app.clone(),
            target.clone(),
            s.clone(),
            entry.super_id.clone(),
        )
        .await?;
        if !result.ok {
            return Err(result.soobshchenie);
        }
    } else {
        let (n, cold, current) = {
            let map = s.vkladki.lock().map_err(|_| "Список занят")?;
            let v = map
                .get(&entry.super_id)
                .filter(|v| {
                    !v.nastrojki.privatnaya
                        && v.zapusk_id == entry.engine_generation
                        && profile_guard(&v.nastrojki) == entry.profile_guard
                })
                .ok_or(CHANGED)?;
            (
                v.nastrojki.clone(),
                v.proc.is_none() && v.zapas_pid.is_none(),
                v.obychnye.clone(),
            )
        };
        let (tab, engine) = if entry.window == "main" {
            if cold != entry.cold {
                return Err(CHANGED.into());
            }
            let before = if cold {
                let id = n.id.clone();
                let address = n.adres.clone();
                tauri::async_runtime::spawn_blocking(move || {
                    if n.pustaya {
                        Sessiya::novaya(&address)
                    } else {
                        Sessiya::zagruzit(&crate::brauzer::papka_profilya(&id))
                    }
                })
                .await
                .map_err(|_| "Не удалось прочитать сессию")?
            } else {
                current.ok_or(CHANGED)?
            };
            validate(&before, &entry)?;
            if cold {
                let mut map = s.vkladki.lock().map_err(|_| "Список занят")?;
                let v = map
                    .get_mut(&entry.super_id)
                    .filter(|v| {
                        v.zapusk_id == entry.engine_generation
                            && v.proc.is_none()
                            && v.zapas_pid.is_none()
                            && !v.zapusk_idet
                            && profile_guard(&v.nastrojki) == entry.profile_guard
                    })
                    .ok_or(CHANGED)?;
                // Discard an old engine's cached snapshot; wait for this launch's publication.
                v.obychnye = None;
            }
            unlocked()?;
            let result = crate::pereklyuchit(
                app.clone(),
                target.clone(),
                s.clone(),
                entry.super_id.clone(),
            )
            .await?;
            let _ = app.emit_to(&entry.window, "vkladki-izmenilis", &entry.super_id);
            if !result.ok {
                return Err(result.soobshchenie);
            }
            let engine = s
                .vkladki
                .lock()
                .map_err(|_| "Список занят")?
                .get(&entry.super_id)
                .ok_or(CHANGED)?
                .zapusk_id;
            if !cold && engine != entry.engine_generation {
                return Err(CHANGED.into());
            }
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(25);
            loop {
                unlocked()?;
                if let Some(session) = live_session(&s, &entry, engine)? {
                    validate(&session, &entry)?;
                    break (entry.tab_id.ok_or(CHANGED)?, engine);
                }
                if std::time::Instant::now() >= deadline {
                    return Err("Движок не ответил. Повторите поиск.".into());
                }
                tokio::time::sleep(std::time::Duration::from_millis(40)).await;
            }
        } else {
            crate::okna_app::prepare_search(&app, &s, &target, &entry).await?
        };
        let command = Komanda::Vybrat { id: tab };
        let command = if entry.window == "main" {
            command
        } else {
            Komanda::OknoKomanda {
                label: entry.window.clone(),
                komanda: Box::new(command),
            }
        };
        crate::okna_app::kanal(&s, &entry.window, &entry.super_id)?
            .try_send(command)
            .map_err(|_| "Движок занят. Повторите поиск.")?;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(25);
        loop {
            unlocked()?;
            if let Some(session) = live_session(&s, &entry, engine)? {
                let row = session
                    .vkladki
                    .iter()
                    .find(|t| t.id == tab && !t.privatnaya && t.route == entry.route)
                    .ok_or(CHANGED)?;
                if session.aktivnaya == tab && !row.otlozhena {
                    break;
                }
            }
            if std::time::Instant::now() >= deadline {
                return Err("Движок не подтвердил выбор вкладки. Повторите поиск.".into());
            }
            tokio::time::sleep(std::time::Duration::from_millis(40)).await;
        }
    }
    unlocked()?;
    // Warm Super switching does not otherwise refresh App's selected owner.
    app.emit_to(&entry.window, "vkladki-izmenilis", &entry.super_id)
        .map_err(|_| "Окно уже закрыто")?;
    target.show().map_err(|e| e.to_string())?;
    target.set_focus().map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn sample() -> (Sessiya, Entry) {
        let mut session = Sessiya::novaya("https://duplicate.test/");
        session.dobavit("https://duplicate.test/");
        let row = &session.vkladki[0];
        let entry = Entry {
            super_id: "super".into(),
            super_name: "Super".into(),
            window: "main".into(),
            tab_id: Some(row.id),
            title: String::new(),
            address: row.adres.clone(),
            sleeping: false,
            route_label: String::new(),
            index: 0,
            route: row.route.clone(),
            engine_generation: 4,
            view_generation: 4,
            route_generation: row.route_generation,
            cold: false,
            profile_guard: String::new(),
        };
        (session, entry)
    }
    #[test]
    fn duplicate_urls_do_not_authorize_another_live_id_or_reordered_index() {
        let (mut session, entry) = sample();
        assert!(validate(&session, &entry).is_ok());
        session.vkladki.swap(0, 1);
        assert!(validate(&session, &entry).is_err());
        session.vkladki.remove(1);
        assert!(validate(&session, &entry).is_err());
    }
    #[test]
    fn private_and_changed_route_generation_are_rejected_before_selection() {
        let (mut session, entry) = sample();
        session.vkladki[0].route_generation += 1;
        assert!(validate(&session, &entry).is_err());
        session.vkladki[0].route_generation = entry.route_generation;
        session.vkladki[0].privatnaya = true;
        assert!(validate(&session, &entry).is_err());
    }
    #[test]
    fn profile_guard_changes_when_cold_inherited_route_changes() {
        let mut n:crate::Nastrojki=serde_json::from_value(serde_json::json!({"id":"super","nazvanie":"Super",
            "proxy":"socks5://localhost:1234","adres":"about:blank","rezat_reklamu":true,"pustaya":false})).unwrap();
        let before = profile_guard(&n);
        assert_eq!(before.len(), 64);
        n.proxy = "socks5://localhost:5678".into();
        assert_ne!(profile_guard(&n), before);
        n.proxy = "socks5://localhost:1234".into();
        n.nazvanie = "Renamed".into();
        assert_eq!(profile_guard(&n), before);
    }
}
