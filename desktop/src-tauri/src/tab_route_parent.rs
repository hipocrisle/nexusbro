//! Trusted-shell route transactions. Network preparation never commits tab state.
use crate::tab_routes::{NativeRoute, RouteChoice};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use tauri::{Manager, State};

type Attempt = (String, u64, String, u64);
struct Pending {
    token: Arc<()>,
    generation: u64,
    browser_id: i32,
    sender: tokio::sync::oneshot::Sender<Result<(), String>>,
}
static PENDING: OnceLock<Mutex<HashMap<Attempt, Pending>>> = OnceLock::new();
static LAST_SENT: OnceLock<Mutex<HashMap<Attempt, u64>>> = OnceLock::new();
fn last_sent() -> &'static Mutex<HashMap<Attempt, u64>> {
    LAST_SENT.get_or_init(Default::default)
}
fn pending() -> &'static Mutex<HashMap<Attempt, Pending>> {
    PENDING.get_or_init(Default::default)
}
struct WaitGuard {
    key: Attempt,
    token: Arc<()>,
}
impl Drop for WaitGuard {
    fn drop(&mut self) {
        if let Ok(mut map) = pending().lock() {
            if map
                .get(&self.key)
                .is_some_and(|p| Arc::ptr_eq(&p.token, &self.token))
            {
                map.remove(&self.key);
            }
        }
    }
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Choices {
    inherit_label: String,
    connections: Vec<Connection>,
}
#[derive(serde::Serialize)]
struct Connection {
    id: String,
    name: String,
}

#[tauri::command]
pub async fn tab_route_choices(
    window: tauri::WebviewWindow,
    s: State<'_, crate::Sostoyanie>,
    id: String,
) -> Result<Choices, String> {
    if crate::browser_lock::locked_global() {
        return Err("Браузер заблокирован".into());
    }
    crate::okna_app::kanal(&s, window.label(), &id)?;
    let name = s
        .vkladki
        .lock()
        .map_err(|_| "Состояние занято")?
        .get(&id)
        .map(|v| v.nastrojki.nazvanie.clone())
        .ok_or("Супер-вкладка закрыта")?;
    // Return names/opaque IDs only, never provider URLs or credentials.
    let settings = tauri::async_runtime::spawn_blocking(crate::nastrojki::zagruzit)
        .await
        .map_err(|_| "Не удалось прочитать подключения")?;
    let mut counts = HashMap::new();
    for c in &settings.vyhody {
        *counts.entry(c.id.as_str()).or_insert(0) += 1;
    }
    let connections = settings
        .vyhody
        .iter()
        .filter(|c| {
            counts.get(c.id.as_str()) == Some(&1)
                && crate::tab_routes::ConnectionId::parse(&c.id).is_ok()
        })
        .map(|c| Connection {
            id: c.id.clone(),
            name: c.nazvanie.clone(),
        })
        .collect();
    Ok(Choices {
        inherit_label: format!("«{name}»"),
        connections,
    })
}

fn session_tab(
    s: &crate::Sostoyanie,
    label: &str,
    super_id: &str,
    tab_id: u64,
) -> Result<(u64, crate::sessiya::Obychnaya), String> {
    if label == "main" {
        let map = s.vkladki.lock().map_err(|_| "Состояние занято")?;
        let v = map
            .get(super_id)
            .filter(|v| v.proc.is_some())
            .ok_or("Движок остановлен")?;
        let tab = v
            .obychnye
            .as_ref()
            .and_then(|s| s.vkladki.iter().find(|t| t.id == tab_id))
            .cloned()
            .ok_or("Вкладка закрыта")?;
        return Ok((v.zapusk_id, tab));
    }
    let map = s.okna.0.lock().map_err(|_| "Состояние занято")?;
    let w = map
        .get(label)
        .filter(|w| w.super_id == super_id && !w.closing)
        .ok_or("Окно закрыто или переключено")?;
    let tab = w
        .sessiya
        .as_ref()
        .and_then(|s| s.vkladki.iter().find(|t| t.id == tab_id))
        .cloned()
        .ok_or("Вкладка закрыта")?;
    Ok((w.engine, tab))
}

fn for_window(label: &str, command: crate::sessiya::Komanda) -> crate::sessiya::Komanda {
    if label == "main" {
        command
    } else {
        crate::sessiya::Komanda::OknoKomanda {
            label: label.into(),
            komanda: Box::new(command),
        }
    }
}

#[tauri::command]
pub async fn tab_route_set(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
    s: State<'_, crate::Sostoyanie>,
    id: String,
    tab_id: u64,
    route: RouteChoice,
    expected_generation: u64,
) -> Result<(), String> {
    if crate::browser_lock::locked_global() {
        return Err("Браузер заблокирован".into());
    }
    if matches!(route, RouteChoice::Blocked { .. }) {
        return Err("Недопустимый путь".into());
    }
    let label = window.label().to_owned();
    let (engine, tab) = session_tab(&s, &label, &id, tab_id)?;
    if tab.route_pending || tab.route_generation != expected_generation {
        return Err("Путь вкладки уже изменяется. Повторите после завершения.".into());
    }
    if tab.route == route && tab.route_error.is_none() {
        return Ok(());
    }
    let generation = expected_generation
        .checked_add(1)
        .filter(|g| *g < (1_u64 << 53))
        .ok_or("Счётчик изменений исчерпан. Откройте супервкладку заново.")?;
    let key = (id.clone(), engine, label.clone(), tab_id);
    if last_sent()
        .lock()
        .map_err(|_| "Состояние занято")?
        .get(&key)
        .is_some_and(|g| *g > expected_generation)
    {
        return Err("Подтверждение пути ещё обновляется. Повторите действие.".into());
    }
    let token = Arc::new(());
    let (sender, receiver) = tokio::sync::oneshot::channel();
    {
        let mut map = pending().lock().map_err(|_| "Состояние занято")?;
        if map.contains_key(&key) {
            return Err("Путь вкладки уже изменяется".into());
        }
        map.insert(
            key.clone(),
            Pending {
                token: token.clone(),
                generation,
                browser_id: tab.browser_id,
                sender,
            },
        );
    }
    let _wait = WaitGuard {
        key: key.clone(),
        token,
    };
    crate::zapisat_v_zhurnal(format!("route-prepare engine={engine} tab={tab_id} attempt={generation} begin"));
    let descriptor = prepare(&app, &id, engine, &route).await.map_err(|error| {
        crate::zapisat_v_zhurnal(format!("route-prepare engine={engine} tab={tab_id} attempt={generation} denied"));
        error
    })?;
    let (current_engine, current) = session_tab(&s, &label, &id, tab_id)?;
    if current_engine != engine
        || current.browser_id != tab.browser_id
        || current.route_generation != expected_generation
        || crate::browser_lock::locked_global()
    {
        return Err("Вкладка изменилась во время подготовки пути. Повторите действие.".into());
    }
    let channel = crate::okna_app::kanal(&s, &label, &id)?;
    // Consume the attempt before sending: a fast native ACK can precede its
    // session snapshot. A stale UI must never reuse that attempt's identity.
    let previous = last_sent()
        .lock()
        .map_err(|_| "Состояние занято")?
        .insert(key.clone(), generation);
    if channel
        .try_send(for_window(
            &label,
            crate::sessiya::Komanda::RouteApply {
                id: tab_id,
                route,
                generation,
                prepared: descriptor,
                expected_browser: tab.browser_id,
            },
        ))
        .is_err()
    {
        let mut sent = last_sent().lock().map_err(|_| "Состояние занято")?;
        if sent.get(&key) == Some(&generation) {
            if let Some(old) = previous {
                sent.insert(key, old);
            } else {
                sent.remove(&key);
            }
        }
        return Err("Движок занят или остановлен".into());
    }
    match tokio::time::timeout(std::time::Duration::from_secs(30), receiver).await {
        Ok(Ok(result)) => result,
        _ => Err("Движок не подтвердил изменение пути. Проверьте текущий путь вкладки.".into()),
    }
}

async fn prepare(
    app: &tauri::AppHandle,
    id: &str,
    engine: u64,
    route: &RouteChoice,
) -> Result<NativeRoute, String> {
    let s: State<crate::Sostoyanie> = app.state();
    let (settings, inherit) = {
        let map = s.vkladki.lock().map_err(|_| "Состояние занято")?;
        let v = map
            .get(id)
            .filter(|v| v.zapusk_id == engine && v.proc.is_some())
            .ok_or("Движок остановлен")?;
        let inherit = v
            .inherit_route
            .clone()
            .ok_or("Путь супервкладки недоступен")?;
        let settings = v
            .route_settings
            .clone()
            .ok_or("Путь супервкладки недоступен")?;
        (settings, inherit)
    };
    s.routes
        .prepare(id, engine, &settings, route, &inherit)
        .await
}

pub fn translation_port(
    s: &crate::Sostoyanie,
    super_id: &str,
    engine: u64,
    data: &serde_json::Value,
) -> Option<u16> {
    let label = data["label"]
        .as_str()
        .filter(|s| !s.is_empty())
        .unwrap_or("main");
    let id = data["id"].as_u64()?;
    let (current_engine, tab) = session_tab(s, label, super_id, id).ok()?;
    if current_engine != engine
        || tab.route_pending
        || data["route_generation"].as_u64()? != tab.route_generation
        || data["browser_id"].as_i64()? != i64::from(tab.browser_id)
    {
        return None;
    }
    let key = data["route_key"].as_str()?;
    let map = s.vkladki.lock().ok()?;
    let v = map
        .get(super_id)
        .filter(|v| v.zapusk_id == engine && v.proc.is_some())?;
    let inherit = v.inherit_route.as_ref()?;
    if key == inherit.key {
        return v.nash_proxy.as_ref().map(|p| p.port);
    }
    drop(map);
    s.routes.port_for_key(super_id, engine, key)
}

/// Native stdout already belongs to a checked engine generation.
pub fn receive(
    app: &tauri::AppHandle,
    super_id: &str,
    engine: u64,
    data: &serde_json::Value,
) -> bool {
    let kind = data["tip"].as_str().unwrap_or("");
    if kind != "route-result" && kind != "route-needed" {
        return false;
    }
    let label = data["label"]
        .as_str()
        .filter(|s| !s.is_empty())
        .unwrap_or("main")
        .to_owned();
    let (Some(id), Some(generation), Some(browser_id)) = (
        data["id"].as_u64(),
        data["generation"].as_u64(),
        data["browser_id"]
            .as_i64()
            .and_then(|v| i32::try_from(v).ok()),
    ) else {
        return true;
    };
    if kind == "route-result" {
        if data["ok"] == true {
            crate::perevod::otmenit(super_id, id);
        }
        let key = (super_id.into(), engine, label, id);
        if let Ok(mut sent) = last_sent().lock() {
            sent.entry(key.clone())
                .and_modify(|g| *g = (*g).max(generation))
                .or_insert(generation);
        }
        if let Ok(mut map) = pending().lock() {
            if map
                .get(&key)
                .is_some_and(|p| p.generation == generation && p.browser_id == browser_id)
            {
                if let Some(p) = map.remove(&key) {
                    let result = if data["ok"] == true {
                        Ok(())
                    } else {
                        Err(crate::tab_routes::change_error_message(data["error_code"].as_str()).into())
                    };
                    let _ = p.sender.send(result);
                }
            }
        }
        crate::zapisat_v_zhurnal(format!(
            "route-result engine={engine} tab={id} attempt={generation} ok={} reason={}",
            data["ok"] == true,
            if data["ok"] == true { "none" } else { crate::tab_routes::change_error_code(data["error_code"].as_str()) }
        ));
        return true;
    }
    let Ok(route) = serde_json::from_value::<RouteChoice>(data["route"].clone()) else {
        return true;
    };
    let app = app.clone();
    let super_id = super_id.to_owned();
    tauri::async_runtime::spawn(async move {
        let result = prepare(&app, &super_id, engine, &route).await;
        let s: State<crate::Sostoyanie> = app.state();
        if !s.vkladki.lock().ok().is_some_and(|v| {
            v.get(&super_id)
                .is_some_and(|v| v.zapusk_id == engine && v.proc.is_some())
        }) {
            return;
        }
        let command = match result {
            Ok(prepared) => crate::sessiya::Komanda::RouteApply {
                id,
                route,
                generation,
                prepared,
                expected_browser: browser_id,
            },
            Err(_) => crate::sessiya::Komanda::RouteFailed {
                id,
                generation,
                expected_browser: browser_id,
                error: "Выбранный путь недоступен. Соединение заблокировано.".into(),
            },
        };
        if let Ok(channel) = crate::okna_app::kanal(&s, &label, &super_id) {
            let _ = channel.try_send(for_window(&label, command));
        }
    });
    true
}

pub fn release_engine(super_id: &str, engine: u64) {
    if let Ok(mut map) = pending().lock() {
        map.retain(|(s, g, _, _), _| s != super_id || *g != engine);
    }
    if let Ok(mut map) = last_sent().lock() {
        map.retain(|(s, g, _, _), _| s != super_id || *g != engine);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_waiter_drop_cannot_remove_a_new_waiter_with_repeated_native_identity() {
        let key = ("waiter-race-fixture".into(), 1, "main".into(), 1);
        let old_token = Arc::new(());
        let guard = WaitGuard {
            key: key.clone(),
            token: old_token,
        };
        let new_token = Arc::new(());
        let (sender, _receiver) = tokio::sync::oneshot::channel();
        pending().lock().unwrap().insert(
            key.clone(),
            Pending {
                token: new_token.clone(),
                generation: 1,
                browser_id: 7,
                sender,
            },
        );
        drop(guard);
        let mut map = pending().lock().unwrap();
        assert!(map
            .get(&key)
            .is_some_and(|p| Arc::ptr_eq(&p.token, &new_token)));
        map.remove(&key);
    }
}
