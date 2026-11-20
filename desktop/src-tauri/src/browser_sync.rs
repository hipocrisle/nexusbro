//! Selected portable records only. No profile directories, cookies, private data or proxy secrets.
use crate::{
    browser_account::{AccountState, SyncItem},
    secure_vault::VaultState,
    sync_receipts::{classify_record, invalidate_from_remote, Receipt, RecordState},
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    io::Write,
    sync::{Arc, Mutex},
};
use tauri::{Emitter, Manager};
use zeroize::Zeroizing;
#[derive(Default)]
pub struct SyncState {
    serial: tokio::sync::Mutex<()>,
}
#[derive(Default, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Selection {
    settings: bool,
    bookmarks: bool,
    supers: Vec<String>,
    passwords: Vec<String>,
}
#[derive(Default, Serialize, Deserialize)]
struct Meta {
    hash: String,
    modified: u64,
}
#[derive(Default, Serialize, Deserialize)]
struct Report {
    pushed: usize,
    pulled: usize,
    conflicts: Vec<String>,
    errors: Vec<String>,
}
#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct Local {
    selection: Selection,
    meta: BTreeMap<String, Meta>,
    tombstones: BTreeMap<String, u64>,
    last_sync: Option<u64>,
    intent_seen: BTreeMap<String, u64>,
    report: Report,
    catalog: BTreeMap<String, String>,
    // Separate from `meta`: snapshot metadata is written before a network ACK.
    super_receipts: BTreeMap<String, Receipt>,
    last_exchange_failed: bool,
    exchange_incomplete: bool,
}
const SETTINGS: &[&str] = &[
    "tema",
    "shrift",
    "razmer_shrifta",
    "plotnost",
    "masshtab_stranic",
    "panel_zakladok",
    "novaya_stranica",
    "poisk",
    "yazyk_perevoda",
];
fn hash(value: &str) -> String {
    Sha256::digest(value.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
fn slot(v: &VaultState) -> Result<String, String> {
    let raw = v.get("account.browser.v1")?;
    let cfg: Value = raw
        .as_ref()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or(Value::Null);
    let who = json!([cfg["server"], cfg["session"]["user_id"]]);
    Ok(format!("sync.state/{}", hash(&who.to_string())))
}
fn load(v: &VaultState, key: &str) -> Result<Local, String> {
    match v.get(key)? {
        None => Ok(Local::default()),
        Some(raw) => {
            serde_json::from_str(&raw).map_err(|_| "Повреждены настройки синхронизации".into())
        }
    }
}
fn save(v: &VaultState, key: &str, data: &Local) -> Result<(), String> {
    v.put(
        key,
        &Zeroizing::new(
            serde_json::to_string(data)
                .map_err(|_| "Ошибка состояния синхронизации".to_string())?,
        ),
    )
}
fn identity(id: &str) -> Result<(&str, &str), String> {
    let parts: Vec<_> = id.split('/').collect();
    if parts.len() != 3
        || parts[0] != "nexusbro.v1"
        || parts[2].is_empty()
        || parts[2].len() > 128
        || !parts[2]
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err("Неверный идентификатор синхронизации".into());
    }
    Ok((parts[1], parts[2]))
}
fn selected(s: &Selection, id: &str) -> bool {
    let Ok((kind, id)) = identity(id) else {
        return false;
    };
    match kind {
        "settings" => s.settings,
        "bookmark" => s.bookmarks,
        "super" => !crate::privatnost::eto_id(id) && (s.supers.iter().any(|n| n == "*" || n == id)),
        "password" => s.passwords.iter().any(|n| n == "*" || n == id),
        _ => false,
    }
}
fn id(kind: &str, key: &str) -> String {
    format!("nexusbro.v1/{kind}/{key}")
}
fn portable_super(n: &crate::Nastrojki) -> Value {
    json!({"id":n.id,"nazvanie":n.nazvanie,"cvet":n.cvet,"adres":n.adres,"rezat_reklamu":n.rezat_reklamu,"pustaya":n.pustaya,"poryadok":n.poryadok})
}
fn all_supers(s: &crate::Sostoyanie) -> Result<BTreeMap<String, crate::Nastrojki>, String> {
    let saved = match std::fs::read(crate::fajl_sohranennyh()) {
        Ok(raw) => serde_json::from_slice::<Vec<crate::Nastrojki>>(&raw)
            .map_err(|_| "Повреждён список сохранённых супер-вкладок; синхронизация остановлена")?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => vec![],
        Err(_) => return Err("Не удалось прочитать сохранённые супер-вкладки".into()),
    };
    let mut out: BTreeMap<_, _> = saved
        .into_iter()
        .filter(|n| !n.privatnaya && !crate::privatnost::eto_id(&n.id))
        .map(|n| (n.id.clone(), n))
        .collect();
    for n in s
        .vkladki
        .lock()
        .unwrap()
        .values()
        .map(|v| &v.nastrojki)
        .filter(|n| !n.privatnaya && !crate::privatnost::eto_id(&n.id))
    {
        out.insert(n.id.clone(), n.clone());
    }
    Ok(out)
}
static MUTATING: Mutex<Vec<String>> = Mutex::new(Vec::new());
pub struct Mutation(String);
impl Drop for Mutation {
    fn drop(&mut self) {
        if let Ok(mut ids) = MUTATING.lock() {
            if let Some(index) = ids.iter().position(|id| id == &self.0) {
                ids.remove(index);
            }
        }
    }
}
pub fn mutation(kind: &str, key: &str) -> Mutation {
    let id = id(kind, key);
    MUTATING.lock().unwrap().push(id.clone());
    Mutation(id)
}
fn gather(
    v: &VaultState,
    s: &crate::Sostoyanie,
    selection: &Selection,
) -> Result<BTreeMap<String, String>, String> {
    if MUTATING
        .lock()
        .map_err(|_| "Данные заняты")?
        .iter()
        .any(|id| selected(selection, id))
    {
        return Err(
            "Супер-вкладка сейчас изменяется; повторите синхронизацию после завершения действия"
                .into(),
        );
    }
    let mut out = BTreeMap::new();
    if selection.settings {
        if let Some(settings) = crate::nastrojki::zagruzit_sync()? {
            let all = serde_json::to_value(settings).map_err(|_| "Ошибка настроек".to_string())?;
            let data: serde_json::Map<_, _> = SETTINGS
                .iter()
                .filter_map(|k| all.get(*k).map(|value| ((*k).into(), value.clone())))
                .collect();
            out.insert(id("settings", "global"), Value::Object(data).to_string());
        }
    }
    if selection.bookmarks {
        for bookmark in crate::biblioteka::sync_bookmarks()? {
            let key = id("bookmark", &bookmark.id);
            identity(&key)?;
            out.insert(
                key,
                serde_json::to_string(&bookmark).map_err(|_| "Ошибка закладки".to_string())?,
            );
        }
    }
    for (key, n) in all_supers(s)? {
        let key = id("super", &key);
        if selected(selection, &key) {
            identity(&key)?;
            out.insert(key, portable_super(&n).to_string());
        }
    }
    for key in v.keys("password/")? {
        let Some(raw) = v.get(&key)? else {
            continue;
        };
        let c: crate::passwords::Credential =
            serde_json::from_str(&raw).map_err(|_| "Повреждена запись пароля".to_string())?;
        let key = id("password", &c.id);
        if selected(selection, &key) && !crate::privatnost::eto_id(&c.super_id) {
            identity(&key)?;
            out.insert(key, raw.to_string());
        }
    }
    Ok(out)
}
static INTENTS: Mutex<()> = Mutex::new(());
fn intents_path() -> std::path::PathBuf {
    crate::brauzer::papka_profilej()
        .parent()
        .unwrap()
        .join("sync-delete-intents.json")
}
fn read_intents() -> Result<BTreeMap<String, u64>, String> {
    match std::fs::read(intents_path()) {
        Ok(raw) => serde_json::from_slice(&raw)
            .map_err(|_| "Повреждён журнал удалений; синхронизация остановлена".into()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
        Err(_) => Err("Не удалось прочитать журнал удалений".into()),
    }
}
fn delete_intents() -> Result<BTreeMap<String, u64>, String> {
    let _guard = INTENTS.lock().map_err(|_| "Журнал удалений занят")?;
    read_intents()
}
pub fn record_delete(kind: &str, record: &str) -> Result<(), String> {
    let id = id(kind, record);
    identity(&id)?;
    let _guard = INTENTS.lock().map_err(|_| "Журнал удалений занят")?;
    let mut items = read_intents()?;
    if items.len() > 20000 {
        return Err("Журнал удалений переполнен".into());
    }
    let generation =
        crate::passwords::now().max(items.get(&id).copied().unwrap_or(0).saturating_add(1));
    items.insert(id, generation);
    atomic(
        &intents_path(),
        &serde_json::to_vec(&items).map_err(|_| "Ошибка журнала удалений")?,
    )
}

pub fn record_password_delete(_v: &VaultState, password_id: &str) -> Result<(), String> {
    record_delete("password", password_id)
}
fn snapshot(
    local: &mut Local,
    values: &BTreeMap<String, String>,
    intents: &BTreeMap<String, u64>,
) -> Result<Vec<SyncItem>, String> {
    let mut items = vec![];
    for (id, value) in values {
        let digest = hash(value);
        let meta = local.meta.entry(id.clone()).or_default();
        if meta.hash != digest {
            meta.hash = digest;
            meta.modified = crate::passwords::now().max(meta.modified.saturating_add(1));
        }
        local.tombstones.remove(id);
        if let Some(generation) = intents.get(id) {
            local.intent_seen.insert(id.clone(), *generation);
        }
        items.push(SyncItem {
            id: id.clone(),
            value: value.clone(),
            modified: meta.modified,
            deleted: false,
        });
    }
    for (id, meta) in &local.meta {
        if selected(&local.selection, id) && !values.contains_key(id) {
            if let Some(time) = intents
                .get(id)
                .filter(|time| **time > local.intent_seen.get(id).copied().unwrap_or(0))
            {
                local
                    .tombstones
                    .insert(id.clone(), (*time).max(meta.modified.saturating_add(1)));
                local.intent_seen.insert(id.clone(), *time);
            }
        }
    }
    for (id, time) in &local.tombstones {
        if selected(&local.selection, id) {
            items.push(SyncItem {
                id: id.clone(),
                value: String::new(),
                modified: *time,
                deleted: true,
            });
        }
    }
    Ok(items)
}
fn atomic(path: &std::path::Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path.parent().ok_or("Нет каталога данных")?;
    std::fs::create_dir_all(parent).map_err(|_| "Не удалось записать данные")?;
    let tmp = path.with_extension(format!("sync-{}.tmp", crate::passwords::random_id()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let result: Result<(), String> = (|| {
        let mut file = options
            .open(&tmp)
            .map_err(|_| "Не удалось записать данные")?;
        file.write_all(bytes)
            .and_then(|_| file.sync_all())
            .map_err(|_| "Не удалось записать данные")?;
        std::fs::rename(&tmp, path)
            .map_err(|_| "Не удалось сохранить синхронизированные данные".to_string())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(tmp);
    }
    result
}
async fn apply(
    app: &tauri::AppHandle,
    s: &crate::Sostoyanie,
    v: &VaultState,
    item: &SyncItem,
    expected: Option<&str>,
) -> Result<(), String> {
    let (kind, key) = identity(&item.id)?;
    let value = if item.deleted {
        Value::Null
    } else {
        serde_json::from_str::<Value>(&item.value).map_err(|_| "Некорректные данные из облака")?
    };
    match kind {
        "settings" => {
            if item.deleted {
                return Err("Удаление настроек из облака не применяется автоматически".into());
            }
            let expected = expected.map(str::to_owned);
            let n = tauri::async_runtime::spawn_blocking(move || {
                crate::nastrojki::patch_portable(value, expected)
            })
            .await
            .map_err(|_| "Ошибка применения настроек")??;
            for channel in s
                .vkladki
                .lock()
                .unwrap()
                .values()
                .filter_map(|v| v.kanal.as_ref())
            {
                let _ = channel.try_send(crate::sessiya::Komanda::Masshtab {
                    procent: n.masshtab_stranic,
                });
            }
            let _ = app.emit("nastrojki-izmenilis", &n);
        }
        "bookmark" => {
            let bookmark = if item.deleted {
                None
            } else {
                let n: crate::biblioteka::Zapis =
                    serde_json::from_value(value).map_err(|_| "Некорректная закладка")?;
                if n.id != key
                    || reqwest::Url::parse(&n.adres)
                        .ok()
                        .is_none_or(|u| !matches!(u.scheme(), "https" | "http"))
                {
                    return Err("Небезопасная закладка".into());
                }
                Some(n)
            };
            crate::biblioteka::sync_apply_bookmark(app, key, bookmark, expected)?;
        }
        "password" => {
            if item.deleted {
                v.compare_and_set(&format!("password/{key}"), expected, None)?;
            } else {
                let n: crate::passwords::Credential =
                    serde_json::from_value(value).map_err(|_| "Некорректная запись пароля")?;
                if n.id != key
                    || crate::passwords::secure_origin(&n.origin)? != n.origin
                    || n.username.len() > 512
                    || n.password.len() > 4096
                    || crate::privatnost::eto_id(&n.super_id)
                {
                    return Err("Небезопасная запись пароля".into());
                }
                let raw =
                    Zeroizing::new(serde_json::to_string(&n).map_err(|_| "Ошибка записи пароля")?);
                v.compare_and_set(&format!("password/{key}"), expected, Some(&raw))?;
            }
        }
        "super" => {
            if crate::privatnost::eto_id(key) {
                return Err("Приватные супер-вкладки не синхронизируются".into());
            }
            if item.deleted {
                let map = s.vkladki.lock().unwrap();
                if map.contains_key(key) {
                    return Err(
                        "Закройте супер-вкладку перед применением удаления из облака".into(),
                    );
                }
                let _guard = crate::supervkladki::SOHRANENNYE
                    .lock()
                    .map_err(|_| "Сохранённые супер-вкладки заняты")?;
                let mut saved = match std::fs::read(crate::fajl_sohranennyh()) {
                    Ok(raw) => serde_json::from_slice::<Vec<crate::Nastrojki>>(&raw)
                        .map_err(|_| "Повреждён список супер-вкладок")?,
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => vec![],
                    Err(_) => return Err("Не удалось прочитать супер-вкладки".into()),
                };
                if saved
                    .iter()
                    .find(|n| n.id == key)
                    .map(|n| portable_super(n).to_string())
                    .as_deref()
                    != expected
                {
                    return Err("Супер-вкладка изменилась во время синхронизации".into());
                }
                saved.retain(|n| n.id != key);
                crate::sohranit_sohranennye(&saved)?;
            } else {
                let name = value["nazvanie"]
                    .as_str()
                    .filter(|n| !n.trim().is_empty() && n.chars().count() <= 80)
                    .ok_or("Неверное имя супер-вкладки")?;
                let color = value["cvet"].as_str().unwrap_or("");
                if !crate::supervkladki::cvet_dopustim(color) || value["id"] != key {
                    return Err("Некорректная супер-вкладка".into());
                }
                let address = value["adres"].as_str().unwrap_or("about:blank");
                if address != "about:blank"
                    && reqwest::Url::parse(address)
                        .ok()
                        .is_none_or(|u| !matches!(u.scheme(), "http" | "https"))
                {
                    return Err("Недопустимый стартовый адрес".into());
                }
                let mut map = s.vkladki.lock().unwrap();
                let _saved_guard = crate::supervkladki::SOHRANENNYE
                    .lock()
                    .map_err(|_| "Сохранённые супер-вкладки заняты")?;
                let mut saved = match std::fs::read(crate::fajl_sohranennyh()) {
                    Ok(raw) => serde_json::from_slice::<Vec<crate::Nastrojki>>(&raw)
                        .map_err(|_| "Повреждён список сохранённых супер-вкладок")?,
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => vec![],
                    Err(_) => return Err("Не удалось прочитать супер-вкладки".into()),
                };
                let existing = map
                    .get(key)
                    .map(|v| v.nastrojki.clone())
                    .or_else(|| saved.iter().find(|n| n.id == key).cloned());
                if existing
                    .as_ref()
                    .map(|n| portable_super(n).to_string())
                    .as_deref()
                    != expected
                {
                    return Err("Супер-вкладка изменилась во время синхронизации".into());
                }
                let mut n = existing.unwrap_or(crate::Nastrojki {
                    id: key.into(),
                    nazvanie: name.into(),
                    cvet: String::new(),
                    proxy: String::new(),
                    proxy_uzel: String::new(),
                    adres: "about:blank".into(),
                    rezat_reklamu: true,
                    pustaya: false,
                    privatnaya: false,
                    poryadok: 0,
                });
                n.nazvanie = name.into();
                n.cvet = color.into();
                n.adres = address.into();
                n.rezat_reklamu = value["rezat_reklamu"].as_bool().unwrap_or(true);
                n.pustaya = value["pustaya"].as_bool().unwrap_or(false);
                n.poryadok = value["poryadok"].as_i64().unwrap_or(0).clamp(0, 100000) as i64;
                let saved_index = saved.iter().position(|old| old.id == key);
                if let Some(index) = saved_index {
                    saved[index] = n.clone();
                    crate::sohranit_sohranennye(&saved)?;
                    if !map.contains_key(key) {
                        drop(map);
                        let _ = app.emit("vkladki-izmenilis", key);
                        return Ok(());
                    }
                }
                let mut all: Vec<_> = map
                    .values()
                    .filter(|x| x.nastrojki.id != key)
                    .map(|x| x.nastrojki.clone())
                    .collect();
                all.push(n.clone());
                atomic(
                    &crate::fajl_spiska(),
                    &serde_json::to_vec(&all).map_err(|_| "Ошибка записи")?,
                )?;
                if let Some(old) = map.get_mut(key) {
                    old.nastrojki = n;
                } else {
                    map.insert(
                        key.into(),
                        crate::Vkladka {
                            nastrojki: n,
                            proc: None,
                            okno: None,
                            zapas_pid: None,
                            nash_proxy: None,
                            inherit_route: None,
                            route_settings: None,
                            tunnel: None,
                            zapusk_idet: false,
                            zapusk_id: 0,
                            kanal: None,
                            obychnye: None,
                        },
                    );
                }
            }
            let _ = app.emit("vkladki-izmenilis", key);
        }
        _ => return Err("Неизвестный тип данных NexusBro".into()),
    }
    Ok(())
}
// Called only for an authenticated remote echo after matching/applying its value.
fn remember_super_ack(data: &mut Local, item: &SyncItem) {
    let Ok((kind, key)) = identity(&item.id) else {
        return;
    };
    if kind != "super" || crate::privatnost::eto_id(key) {
        return;
    }
    if item.deleted {
        data.super_receipts.remove(&item.id);
    } else {
        data.super_receipts.insert(
            item.id.clone(),
            Receipt {
                hash: hash(&item.value),
                confirmed_at: crate::passwords::now(),
            },
        );
    }
}
fn status(s: &crate::Sostoyanie, data: &Local) -> Result<Value, String> {
    let normal: BTreeMap<_, _> = all_supers(s)?
        .into_iter()
        .filter(|(key, n)| !crate::privatnost::eto_id(key) && !n.privatnaya)
        .collect();
    let mut supers = data.catalog.clone();
    supers.retain(|key, _| !crate::privatnost::eto_id(key));
    for (key, n) in &normal {
        supers.insert(key.clone(), n.nazvanie.clone());
    }
    let exchange_bad =
        data.last_exchange_failed || data.exchange_incomplete || !data.report.errors.is_empty();
    let mut states = BTreeMap::new();
    let mut active = Vec::new();
    for key in supers.keys() {
        let record = id("super", key);
        let digest = normal
            .get(key)
            .map(|n| hash(&portable_super(n).to_string()));
        let receipt = data.super_receipts.get(&record);
        let kind = classify_record(
            selected(&data.selection, &record),
            digest.as_deref(),
            receipt,
            data.report.conflicts.contains(&record),
            exchange_bad,
        );
        let mut row = json!({"state":kind});
        if let Some(ack) = receipt {
            row["last_success"] = json!(ack.confirmed_at);
        }
        states.insert(key.clone(), row);
        if kind != RecordState::Off {
            active.push(kind);
        }
    }
    let overall = if active.is_empty() {
        "off"
    } else if exchange_bad
        || !data.report.conflicts.is_empty()
        || active
            .iter()
            .any(|s| matches!(s, RecordState::Error | RecordState::Conflict))
    {
        "error"
    } else if active.iter().all(|s| *s == RecordState::Ok) {
        "ok"
    } else {
        "pending"
    };
    let mut sync_state = json!({"state":overall});
    if let Some(time) = data.last_sync {
        sync_state["last_success"] = json!(time);
    }
    Ok(
        json!({"selection":data.selection,"last_sync":data.last_sync,
        "available_supers":supers.into_iter().map(|(id,name)|json!({"id":id,"name":name})).collect::<Vec<_>>(),
        "report":data.report,"super_states":states,"sync_state":sync_state}),
    )
}
#[tauri::command]
pub async fn security_account(
    vault: tauri::State<'_, Arc<VaultState>>,
    account: tauri::State<'_, AccountState>,
    sync: tauri::State<'_, SyncState>,
    action: String,
    data: Option<Value>,
) -> Result<Value, String> {
    if action == "default_server" {
        // Explicit isolated test mode only; normal installations always use the
        // separate browser service, never a server inferred from another product.
        if std::env::var_os("NEXUSBRO_TEST_DATA").is_some() {
            if let Ok(candidate) = std::env::var("NEXUSBRO_TEST_SYNC_URL") {
                let url = reqwest::Url::parse(&candidate).map_err(|_| "Неверный тестовый адрес")?;
                if url.scheme() != "http"
                    || !matches!(
                        url.host_str(),
                        Some("127.0.0.1" | "localhost" | "[::1]" | "::1")
                    )
                    || !url.username().is_empty()
                    || url.password().is_some()
                    || url.path() != "/"
                    || url.query().is_some()
                    || url.fragment().is_some()
                {
                    return Err("Тестовый сервер должен быть локальным".into());
                }
                return Ok(json!({"server":candidate.trim_end_matches('/')}));
            }
        }
        return Ok(json!({"server":"https://nexusbro-sync.hipogas.org"}));
    }
    let _guard = sync.serial.lock().await;
    account
        .dispatch(&vault, &action, data.unwrap_or_else(|| json!({})))
        .await
}
#[tauri::command]
pub async fn security_sync(
    app: tauri::AppHandle,
    state: tauri::State<'_, crate::Sostoyanie>,
    vault: tauri::State<'_, Arc<VaultState>>,
    account: tauri::State<'_, AccountState>,
    sync: tauri::State<'_, SyncState>,
    action: String,
    data: Option<Value>,
) -> Result<Value, String> {
    let _guard = sync.serial.lock().await;
    let key = slot(&vault)?;
    let mut local = load(&vault, &key)?;
    let data = data.unwrap_or(Value::Null);
    match action.as_str() {
        "status" => status(&state, &local),
        "configure" => {
            let selection: Selection = serde_json::from_value(data["selection"].clone())
                .map_err(|_| "Неверный выбор данных")?;
            if selection.supers.len() > 5000 || selection.passwords.len() > 5000 {
                return Err("Выбрано слишком много записей".into());
            }
            for id in &selection.supers {
                if id != "*" {
                    identity(&crate::browser_sync::id("super", id))?;
                    if crate::privatnost::eto_id(id) {
                        return Err("Приватные записи не синхронизируются".into());
                    }
                }
            }
            for id in &selection.passwords {
                if id != "*" {
                    identity(&crate::browser_sync::id("password", id))?;
                }
            }
            local.selection = selection;
            save(&vault, &key, &local)?;
            status(&state, &local)
        }
        "resolve" => {
            let id = data["id"].as_str().ok_or("Не выбран конфликт")?;
            if !selected(&local.selection, id) {
                return Err("Сначала включите синхронизацию этой записи".into());
            }
            let choice = data["choice"].as_str().ok_or("Не выбран вариант")?;
            let intent_snapshot = delete_intents()?;
            let current = gather(&vault, &state, &local.selection)?;
            let raw = vault
                .get(&format!("account.conflict.{id}"))?
                .ok_or("Конфликт отсутствует")?;
            let record: Value =
                serde_json::from_str(&raw).map_err(|_| "Повреждены данные конфликта")?;
            let previous = record["local"].as_object().and_then(|o| {
                if o.get("deleted") == Some(&Value::Bool(true)) {
                    None
                } else {
                    o.get("value").and_then(Value::as_str)
                }
            });
            if current.get(id).map(String::as_str) != previous {
                return Err("Локальная запись изменилась после появления конфликта. Повторите синхронизацию перед выбором.".into());
            }
            let item = account.prepare_resolution(&vault, id, choice).await?;
            if gather(&vault, &state, &local.selection)?.get(id) != current.get(id) {
                return Err("Локальная запись изменилась; повторите синхронизацию".into());
            }
            apply(
                &app,
                &state,
                &vault,
                &item,
                current.get(&item.id).map(String::as_str),
            )
            .await?;
            account.commit_resolution(&vault, id, choice).await?;
            local.meta.insert(
                item.id.clone(),
                Meta {
                    hash: hash(&item.value),
                    modified: item.modified,
                },
            );
            if item.deleted {
                local.tombstones.insert(item.id.clone(), item.modified);
            } else {
                local.tombstones.remove(&item.id);
                if let Some(generation) = intent_snapshot.get(&item.id) {
                    local.intent_seen.insert(item.id.clone(), *generation);
                }
            }
            // A cached conflict choice is not a fresh server acknowledgement.
            local.super_receipts.remove(&item.id);
            local.report.conflicts.retain(|n| n != id);
            save(&vault, &key, &local)?;
            status(&state, &local)
        }
        "run" => {
            // Durable before any fallible work: restart/lock interruption must
            // not restore a stale green status from an earlier account date.
            local.exchange_incomplete = true;
            save(&vault, &key, &local)?;
            let intent_snapshot = delete_intents()?;
            let initial = gather(&vault, &state, &local.selection)?;
            let items = snapshot(&mut local, &initial, &intent_snapshot)?;
            save(&vault, &key, &local)?;
            let exchange = match account.exchange(&vault, items).await {
                Ok(exchange) => {
                    local.last_exchange_failed = false;
                    exchange
                }
                Err(error) => {
                    local.last_exchange_failed = true;
                    local.exchange_incomplete = false;
                    save(&vault, &key, &local)?;
                    return Err(error);
                }
            };
            for conflict in &exchange.conflicts {
                local.super_receipts.remove(conflict);
            }
            let mut report = Report {
                pushed: exchange.pushed,
                pulled: 0,
                conflicts: exchange.conflicts,
                errors: vec![],
            };
            for item in exchange.remote {
                if let Ok((kind, id)) = identity(&item.id) {
                    if kind == "super" && !crate::privatnost::eto_id(id) {
                        // Even while opted out, a verified remote change makes
                        // an older receipt obsolete. Never mutate local data here.
                        let remote_hash = (!item.deleted).then(|| hash(&item.value));
                        invalidate_from_remote(
                            &mut local.super_receipts,
                            &item.id,
                            remote_hash.as_deref(),
                        );
                        if item.deleted {
                            local.catalog.remove(id);
                        }
                    }
                    if kind == "super" && !item.deleted && !crate::privatnost::eto_id(id) {
                        if let Ok(v) = serde_json::from_str::<Value>(&item.value) {
                            if let Some(name) = v["nazvanie"].as_str() {
                                local
                                    .catalog
                                    .insert(id.into(), name.chars().take(80).collect());
                            }
                        }
                    }
                }
                if !selected(&local.selection, &item.id) {
                    continue;
                }
                if item.deleted {
                    local.super_receipts.remove(&item.id);
                }
                if delete_intents()?.get(&item.id) != intent_snapshot.get(&item.id) {
                    report
                        .errors
                        .push("Запись удалена во время передачи; повторите синхронизацию".into());
                    continue;
                }
                let current = gather(&vault, &state, &local.selection)?;
                if current.get(&item.id) != initial.get(&item.id) {
                    report.errors.push(
                        "Локальные данные изменились во время передачи; повторите синхронизацию"
                            .into(),
                    );
                    continue;
                }
                if current.get(&item.id) == Some(&item.value) && !item.deleted {
                    if !report.conflicts.contains(&item.id) {
                        remember_super_ack(&mut local, &item);
                    }
                    local.meta.insert(
                        item.id.clone(),
                        Meta {
                            hash: hash(&item.value),
                            modified: item.modified,
                        },
                    );
                    continue;
                }
                match apply(
                    &app,
                    &state,
                    &vault,
                    &item,
                    current.get(&item.id).map(String::as_str),
                )
                .await
                {
                    Ok(()) => {
                        if !report.conflicts.contains(&item.id) {
                            remember_super_ack(&mut local, &item);
                        }
                        local.meta.insert(
                            item.id.clone(),
                            Meta {
                                hash: hash(&item.value),
                                modified: item.modified,
                            },
                        );
                        if item.deleted {
                            local.tombstones.insert(item.id.clone(), item.modified);
                        } else {
                            local.tombstones.remove(&item.id);
                            if let Some(generation) = intent_snapshot.get(&item.id) {
                                local.intent_seen.insert(item.id.clone(), *generation);
                            }
                        }
                        report.pulled += 1;
                    }
                    Err(error) => {
                        local.super_receipts.remove(&item.id);
                        report.errors.push(error);
                    }
                }
            }
            report.conflicts.sort();
            report.conflicts.dedup();
            if report.conflicts.is_empty() && report.errors.is_empty() {
                local.last_sync = Some(crate::passwords::now());
            }
            local.report = report;
            local.exchange_incomplete = false;
            save(&vault, &key, &local)?;
            Ok(serde_json::to_value(&local.report).map_err(|_| "Ошибка отчёта")?)
        }
        _ => Err("Неизвестное действие синхронизации".into()),
    }
}
