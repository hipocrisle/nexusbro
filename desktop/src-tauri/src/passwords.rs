//! Browser credentials live only in the encrypted browser vault. Site access is native and scoped.
use crate::secure_vault::VaultState;
use rand_core::{OsRng, RngCore};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, OnceLock,
    },
    time::{Duration, Instant},
};
use tauri::{Emitter, Manager};
use zeroize::{Zeroize, Zeroizing};
static ENABLED: AtomicBool = AtomicBool::new(false);
pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}
pub fn random_id() -> String {
    let mut bytes = [0u8; 16];
    OsRng.fill_bytes(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
pub fn secure_origin(input: &str) -> Result<String, String> {
    let url = reqwest::Url::parse(input).map_err(|_| "Укажите HTTPS-адрес сайта".to_string())?;
    let test_loopback = std::env::var_os("NEXUSBRO_TEST_DATA").is_some()
        && url.scheme() == "http"
        && matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"));
    if (url.scheme() != "https" && !test_loopback)
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(
            "Пароли доступны только для HTTPS-сайтов без встроенных учётных данных в URL".into(),
        );
    }
    Ok(url.origin().ascii_serialization())
}
#[derive(Serialize, Deserialize)]
pub struct Credential {
    pub id: String,
    pub origin: String,
    pub username: String,
    pub password: String,
    pub title: String,
    pub super_id: String,
    pub updated_at: u64,
}
impl Drop for Credential {
    fn drop(&mut self) {
        self.username.zeroize();
        self.password.zeroize();
    }
}
pub fn summary(c: &Credential) -> Value {
    json!({"id":c.id,"origin":c.origin,"username":c.username,"title":c.title,"super_id":c.super_id,"updated_at":c.updated_at})
}
pub fn read(v: &VaultState, id: &str) -> Result<Credential, String> {
    let value = v
        .get(&format!("password/{id}"))?
        .ok_or("Пароль не найден")?;
    serde_json::from_str(&value).map_err(|_| "Запись пароля повреждена".into())
}
pub fn store(v: &VaultState, c: &Credential) -> Result<(), String> {
    let value = Zeroizing::new(
        serde_json::to_string(c).map_err(|_| "Не удалось сохранить пароль".to_string())?,
    );
    v.put(&format!("password/{}", c.id), &value)
}
fn field(data: &Value, key: &str, max: usize) -> Result<String, String> {
    let value = data[key]
        .as_str()
        .ok_or_else(|| format!("Не указан параметр {key}"))?;
    if value.len() > max {
        return Err("Слишком длинное значение".into());
    }
    Ok(value.to_string())
}
struct Draft {
    created: Instant,
    credential: Credential,
}
static DRAFTS: OnceLock<Mutex<HashMap<String, Draft>>> = OnceLock::new();
fn draft(super_id: &str, origin: String, value: &Value) -> Result<Value, String> {
    let username = field(value, "username", 512)?;
    let password = field(value, "password", 4096)?;
    if password.is_empty() {
        return Err("Введите логин и пароль".into());
    }
    let id = random_id();
    let credential = Credential {
        id: random_id(),
        origin,
        username,
        password,
        title: String::new(),
        super_id: super_id.into(),
        updated_at: now(),
    };
    let result = json!({"draft_id":id,"origin":credential.origin,"username":credential.username});
    let mut items = DRAFTS.get_or_init(Default::default).lock().unwrap();
    items.retain(|_, d| d.created.elapsed() < Duration::from_secs(120));
    if items.len() >= 8 {
        items.clear();
    }
    items.insert(
        id,
        Draft {
            created: Instant::now(),
            credential,
        },
    );
    Ok(result)
}
type Pending = HashMap<(String, String), tokio::sync::oneshot::Sender<Result<Value, String>>>;
static PENDING: OnceLock<Mutex<Pending>> = OnceLock::new();
struct Wait((String, String));
impl Drop for Wait {
    fn drop(&mut self) {
        PENDING
            .get_or_init(Default::default)
            .lock()
            .unwrap()
            .remove(&self.0);
    }
}
pub fn receive(app: &tauri::AppHandle, super_id: &str, data: &Value) -> bool {
    match data["tip"].as_str() {
        Some("credential-result") => {
            let key = (super_id.into(), data["token"].as_str().unwrap_or("").into());
            if let Some(tx) = PENDING
                .get_or_init(Default::default)
                .lock()
                .unwrap()
                .remove(&key)
            {
                let result = if data.get("error").is_some() {
                    Err("Не удалось безопасно прочитать или заполнить форму".into())
                } else {
                    Ok(data.clone())
                };
                let _ = tx.send(result);
            }
            true
        }
        Some("credential-offer") => {
            let vault = app.state::<Arc<VaultState>>();
            if !vault.status().unlocked {
                return true;
            }
            let id = data["id"].as_u64().unwrap_or(0);
            let label = data["label"]
                .as_str()
                .filter(|s| !s.is_empty())
                .unwrap_or("main");
            let state = app.state::<crate::Sostoyanie>();
            let expected = target(&state, label, super_id, id);
            if data["value"]["username"]
                .as_str()
                .is_none_or(|s| s.trim().is_empty())
            {
                return true;
            }
            if let Ok(origin) = expected {
                if data["origin"].as_str() == Some(origin.as_str()) {
                    if let Ok(offer) = draft(super_id, origin, &data["value"]) {
                        let _=app.emit_to(label,"stranica-ui",json!({"super_id":super_id,"data":{"tip":"password-offer","id":id,"draft_id":offer["draft_id"],"origin":offer["origin"],"username":offer["username"]}}));
                    }
                }
            }
            true
        }
        _ => false,
    }
}
fn target(
    state: &crate::Sostoyanie,
    label: &str,
    super_id: &str,
    id: u64,
) -> Result<String, String> {
    if crate::privatnost::eto_id(super_id) {
        return Err("Пароли приватных супер-вкладок не сохраняются".into());
    }
    let session = if label == "main" {
        state
            .vkladki
            .lock()
            .unwrap()
            .get(super_id)
            .and_then(|v| v.obychnye.clone())
    } else {
        state
            .okna
            .0
            .lock()
            .unwrap()
            .get(label)
            .filter(|w| w.super_id == super_id && !w.privatnaya)
            .and_then(|w| w.sessiya.clone())
    }
    .ok_or("Страница ещё не открыта")?;
    let tab = session
        .vkladki
        .iter()
        .find(|v| v.id == id && session.aktivnaya == id && !v.privatnaya && !v.otlozhena)
        .ok_or("Выберите обычную активную вкладку")?;
    secure_origin(&tab.adres)
}
async fn site(
    window: &tauri::WebviewWindow,
    state: &crate::Sostoyanie,
    super_id: &str,
    id: u64,
    payload: Option<crate::password_cef::Secret>,
) -> Result<Value, String> {
    let origin = target(state, window.label(), super_id, id)?;
    let channel = crate::okna_app::kanal(state, window.label(), super_id)?;
    let token = random_id();
    let key = (super_id.into(), token.clone());
    let (tx, rx) = tokio::sync::oneshot::channel();
    PENDING
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .insert(key.clone(), tx);
    let _guard = Wait(key);
    let command = crate::sessiya::Komanda::PasswordForm {
        id,
        token,
        origin,
        payload,
    };
    let command = if window.label() == "main" {
        command
    } else {
        crate::sessiya::Komanda::OknoKomanda {
            label: window.label().into(),
            komanda: Box::new(command),
        }
    };
    channel
        .try_send(command)
        .map_err(|_| "Движок занят".to_string())?;
    tokio::time::timeout(Duration::from_secs(15), rx)
        .await
        .map_err(|_| "Нет подтверждения от страницы".to_string())?
        .map_err(|_| "Страница закрыта".to_string())?
}
fn broadcast(app: &tauri::AppHandle, value: bool) {
    if ENABLED.swap(value, Ordering::Relaxed) == value {
        return;
    }
    let state = app.state::<crate::Sostoyanie>();
    let channels: HashMap<_, _> = state
        .vkladki
        .lock()
        .unwrap()
        .iter()
        .filter_map(|(id, v)| v.kanal.clone().map(|q| (id.clone(), q)))
        .collect();
    for channel in channels.values() {
        let _ = channel.try_send(crate::sessiya::Komanda::PasswordsEnabled { enabled: value });
    }
    for (label, w) in state
        .okna
        .0
        .lock()
        .unwrap()
        .iter()
        .filter(|(_, w)| !w.closing)
    {
        if let Some(channel) = channels.get(&w.super_id) {
            let _ = channel.try_send(crate::sessiya::Komanda::OknoKomanda {
                label: label.clone(),
                komanda: Box::new(crate::sessiya::Komanda::PasswordsEnabled { enabled: value }),
            });
        }
    }
    if !value {
        DRAFTS.get_or_init(Default::default).lock().unwrap().clear();
    }
}
fn vault_root(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    if let Some(root) = std::env::var_os("NEXUSBRO_TEST_DATA") {
        Ok(PathBuf::from(root).join("security"))
    } else {
        app.path()
            .app_data_dir()
            .map_err(|_| "Не найден каталог приложения".to_string())
    }
}
pub fn setup(app: &tauri::AppHandle) -> Result<(), String> {
    let root = vault_root(app)?;
    let vault = app.state::<Arc<VaultState>>();
    vault.configure(&root)?;
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(10)).await;
            let value = app.state::<Arc<VaultState>>().status().unlocked;
            broadcast(&app, value);
            DRAFTS
                .get_or_init(Default::default)
                .lock()
                .unwrap()
                .retain(|_, d| d.created.elapsed() < Duration::from_secs(120));
        }
    });
    Ok(())
}
#[tauri::command]
pub async fn security_vault(
    app: tauri::AppHandle,
    vault: tauri::State<'_, Arc<VaultState>>,
    action: String,
    password: Option<String>,
) -> Result<crate::secure_vault::VaultStatus, String> {
    let root = vault_root(&app)?;
    let state = Arc::clone(&vault);
    let status = tauri::async_runtime::spawn_blocking(
        move || -> Result<crate::secure_vault::VaultStatus, String> {
            match action.as_str() {
                "status" => {}
                "lock" => state.lock(),
                "create" => state.create(
                    &root,
                    &Zeroizing::new(password.ok_or("Укажите мастер-пароль")?),
                )?,
                "unlock" => state.unlock(
                    &root,
                    &Zeroizing::new(password.ok_or("Укажите мастер-пароль")?),
                )?,
                _ => return Err("Неизвестное действие хранилища".into()),
            }
            Ok(state.status())
        },
    )
    .await
    .map_err(|_| "Ошибка хранилища".to_string())??;
    broadcast(&app, status.unlocked);
    Ok(status)
}
#[tauri::command]
pub async fn security_passwords(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, crate::Sostoyanie>,
    vault: tauri::State<'_, Arc<VaultState>>,
    action: String,
    data: Option<Value>,
) -> Result<Value, String> {
    let data = data.unwrap_or(Value::Null);
    if !vault.status().unlocked {
        return Err("Хранилище заблокировано".into());
    }
    match action.as_str() {
        "list" => {
            let mut out = vec![];
            for key in vault.keys("password/")? {
                if let Some(value) = vault.get(&key)? {
                    if let Ok(c) = serde_json::from_str::<Credential>(&value) {
                        out.push(summary(&c));
                    }
                }
            }
            Ok(json!(out))
        }
        "save" => {
            let super_id = field(&data, "super_id", 128)?;
            if crate::privatnost::eto_id(&super_id)
                || !state.vkladki.lock().unwrap().contains_key(&super_id)
            {
                return Err("Выберите обычную супер-вкладку".into());
            }
            let c = Credential {
                id: random_id(),
                origin: secure_origin(&field(&data, "origin", 2048)?)?,
                username: field(&data, "username", 512)?,
                password: field(&data, "password", 4096)?,
                title: data["title"]
                    .as_str()
                    .unwrap_or("")
                    .chars()
                    .take(200)
                    .collect(),
                super_id,
                updated_at: now(),
            };
            if c.username.is_empty() || c.password.is_empty() {
                return Err("Введите логин и пароль".into());
            }
            store(&vault, &c)?;
            Ok(summary(&c))
        }
        "delete" => {
            let id = field(&data, "id", 128)?;
            let _mutation = crate::browser_sync::mutation("password", &id);
            let old = read(&vault, &id)?;
            crate::browser_sync::record_password_delete(&vault, &old.id)?;
            vault.remove(&format!("password/{id}"))?;
            Ok(json!({}))
        }
        "reveal" => {
            let c = read(&vault, &field(&data, "id", 128)?)?;
            Ok(json!(c.password))
        }
        "capture" => {
            let super_id = field(&data, "super_id", 128)?;
            let id = data["tab_id"].as_u64().ok_or("Не выбрана вкладка")?;
            let result = site(&window, &state, &super_id, id, None).await?;
            let origin = secure_origin(result["origin"].as_str().ok_or("Неизвестный сайт")?)?;
            draft(&super_id, origin, &result["value"])
        }
        "save_draft" => {
            let id = field(&data, "draft_id", 128)?;
            let mut items = DRAFTS.get_or_init(Default::default).lock().unwrap();
            let draft = items
                .get_mut(&id)
                .filter(|d| d.created.elapsed() < Duration::from_secs(120))
                .ok_or("Срок сохранения формы истёк; введите данные заново")?;
            if let Some(username) = data["username"].as_str() {
                if username.len() > 512 {
                    return Err("Слишком длинный логин".into());
                }
                draft.credential.username = username.into();
            }
            if draft.credential.username.trim().is_empty() {
                return Err("Укажите логин перед сохранением".into());
            }
            draft.credential.title = data["title"]
                .as_str()
                .unwrap_or("")
                .chars()
                .take(200)
                .collect();
            store(&vault, &draft.credential)?;
            let result = summary(&draft.credential);
            items.remove(&id);
            Ok(result)
        }
        "discard_draft" => {
            DRAFTS
                .get_or_init(Default::default)
                .lock()
                .unwrap()
                .remove(&field(&data, "draft_id", 128)?);
            Ok(json!({}))
        }
        "fill" => {
            let c = read(&vault, &field(&data, "id", 128)?)?;
            let super_id = field(&data, "super_id", 128)?;
            let id = data["tab_id"].as_u64().ok_or("Не выбрана вкладка")?;
            if c.super_id != super_id || target(&state, window.label(), &super_id, id)? != c.origin
            {
                return Err("Пароль принадлежит другому сайту или супер-вкладке".into());
            }
            let value = site(
                &window,
                &state,
                &super_id,
                id,
                Some(crate::password_cef::Secret {
                    username: c.username.clone(),
                    password: c.password.clone(),
                }),
            )
            .await?;
            if value["value"] != true {
                return Err("Форма не подтвердила заполнение".into());
            }
            Ok(json!({}))
        }
        _ => Err("Неизвестное действие с паролем".into()),
    }
}
