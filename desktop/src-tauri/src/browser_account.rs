//! Isolated NexusBro accounts and opaque per-item E2E sync. No NexuSSH service reuse.
//! Crypto adapted from NexuSSH (MIT); see browser_crypto.rs and sync-server/LICENSE.
use crate::{browser_crypto as ac, secure_vault::VaultState};
use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashSet};
use zeroize::{Zeroize, Zeroizing};
const CONFIG: &str = "account.browser.v1";
const RESPONSE_LIMIT: usize = 4 * 1024 * 1024;
const ITEM_LIMIT: usize = 256 * 1024 - 128;
#[derive(Default)]
pub struct AccountState {
    serial: tokio::sync::Mutex<()>,
}
#[derive(Default, Serialize, Deserialize)]
struct Config {
    #[serde(default)]
    server: String,
    #[serde(default)]
    username: String,
    #[serde(default)]
    device: String,
    session: Option<Session>,
    #[serde(default)]
    tracks: BTreeMap<String, Track>,
    #[serde(default)]
    watermark: u64,
}
#[derive(Serialize, Deserialize)]
struct Session {
    token: String,
    user_id: String,
    key: String,
    account_salt: String,
    kdf_params: String,
    wrapped_user_key: String,
    totp_enabled: bool,
}
impl Drop for Session {
    fn drop(&mut self) {
        self.token.zeroize();
        self.key.zeroize();
    }
}
#[derive(Default, Serialize, Deserialize)]
struct Track {
    rev: u64,
    digest: Option<String>,
    modified: u64,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct SyncItem {
    pub id: String,
    pub value: String,
    pub modified: u64,
    pub deleted: bool,
}
#[derive(Serialize, Debug)]
pub struct SyncExchange {
    pub remote: Vec<SyncItem>,
    pub pushed: usize,
    pub conflicts: Vec<String>,
}
#[derive(Serialize, Deserialize)]
struct Conflict {
    local: Option<SyncItem>,
    remote: SyncItem,
    revision: u64,
    account_id: String,
    server: String,
}
#[derive(Serialize, Deserialize)]
struct Resolution {
    choice: String,
    hash: String,
    account_id: String,
    server: String,
}
#[derive(Deserialize)]
struct WireItem {
    item_id: String,
    #[serde(rename = "type")]
    kind: String,
    ciphertext: String,
    rev: u64,
    updated_at: u64,
    deleted: bool,
}
fn load(vault: &VaultState) -> Result<Config, String> {
    match vault.get(CONFIG)? {
        Some(s) => {
            serde_json::from_str(&s).map_err(|_| "Повреждены локальные настройки аккаунта".into())
        }
        None => Ok(Config::default()),
    }
}
fn save(vault: &VaultState, cfg: &Config) -> Result<(), String> {
    let s = Zeroizing::new(serde_json::to_string(cfg).map_err(|_| "Ошибка настроек аккаунта")?);
    vault.put(CONFIG, &s)
}
fn status(c: &Config) -> Value {
    json!({"configured":!c.server.is_empty(),"logged_in":c.session.is_some(),"username":c.username,"server":c.server,"totp_enabled":c.session.as_ref().is_some_and(|s|s.totp_enabled),"user_id":c.session.as_ref().map(|s|s.user_id.as_str())})
}
fn text(v: &Value, key: &str, max: usize) -> Result<String, String> {
    let s = v
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("Не заполнено поле {key}"))?;
    if s.is_empty() || s.len() > max {
        return Err(format!("Недопустимое поле {key}"));
    }
    Ok(s.into())
}
fn secret(v: &mut Value, key: &str) -> Result<Zeroizing<String>, String> {
    let s = v
        .get_mut(key)
        .map(Value::take)
        .and_then(|v| {
            if let Value::String(s) = v {
                Some(s)
            } else {
                None
            }
        })
        .ok_or_else(|| format!("Не заполнено поле {key}"))?;
    if s.is_empty() || s.len() > 4096 {
        return Err(format!("Недопустимое поле {key}"));
    }
    Ok(Zeroizing::new(s))
}
fn strong(s: &str) -> Result<(), String> {
    if s.chars().count() < 12 {
        return Err("Пароль аккаунта должен содержать не менее 12 символов".into());
    }
    Ok(())
}
fn endpoint(server: &str) -> Result<String, String> {
    let u = reqwest::Url::parse(server).map_err(|_| "Неверный адрес сервера")?;
    let loopback = matches!(
        u.host_str(),
        Some("localhost" | "127.0.0.1" | "[::1]" | "::1")
    );
    let fixture = cfg!(test) || std::env::var_os("NEXUSBRO_TEST_DATA").is_some();
    if (u.scheme() != "https" && !(fixture && loopback && u.scheme() == "http"))
        || u.host_str().is_none()
        || !u.username().is_empty()
        || u.password().is_some()
        || u.query().is_some()
        || u.fragment().is_some()
        || u.path() != "/"
    {
        return Err(
            "Сервер должен иметь отдельный HTTPS-адрес без пути, пароля или параметров".into(),
        );
    }
    Ok(u.as_str().trim_end_matches('/').into())
}
fn client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(8))
        .timeout(std::time::Duration::from_secs(25))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| "Не удалось создать защищённое соединение".into())
}
async fn http(
    server: &str,
    method: reqwest::Method,
    path: &str,
    token: Option<&str>,
    body: Value,
) -> Result<Value, String> {
    let c = client()?;
    let mut last = "Сервер недоступен".to_string();
    for attempt in 0..2 {
        let mut req = c.request(method.clone(), format!("{server}{path}"));
        if let Some(t) = token {
            req = req.bearer_auth(t);
        }
        if method != reqwest::Method::GET {
            req = req.json(&body);
        }
        let response = match req.send().await {
            Ok(r) => r,
            Err(_) => {
                if method == reqwest::Method::GET && attempt == 0 {
                    continue;
                }
                return Err(
                    "Не удалось подключиться к серверу: проверьте адрес, TLS и сеть".into(),
                );
            }
        };
        let code = response.status();
        if code.is_server_error() && method == reqwest::Method::GET && attempt == 0 {
            last = "Сервер временно недоступен".into();
            continue;
        }
        if response
            .content_length()
            .is_some_and(|n| n > RESPONSE_LIMIT as u64)
        {
            return Err("Слишком большой ответ сервера".into());
        }
        let mut response = response;
        let mut raw = Zeroizing::new(Vec::new());
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| "Не удалось прочитать ответ сервера")?
        {
            if raw.len() + chunk.len() > RESPONSE_LIMIT {
                return Err("Слишком большой ответ сервера".into());
            }
            raw.extend_from_slice(&chunk);
        }
        let v: Value = serde_json::from_slice(&raw).map_err(|_| "Неверный ответ сервера")?;
        if code.is_success() {
            return Ok(v);
        }
        if v["totp_required"] == true {
            return Err("TOTP_REQUIRED: нужен код двухфакторной аутентификации".into());
        }
        let error = v["error"].as_str().unwrap_or("");
        return Err(match (code.as_u16(), error) {
            (401, "invalid token" | "session expired" | "missing bearer token") => {
                "ACCOUNT_SESSION_EXPIRED: войдите в аккаунт снова".into()
            }
            (401, _) => "Неверный пароль, код подтверждения или ключ восстановления".into(),
            (409, "username taken") => "Этот логин уже занят".into(),
            (409, _) => "Действие конфликтует с текущим состоянием аккаунта".into(),
            (429, _) => "Слишком много попыток. Подождите перед повтором".into(),
            _ => format!("Ошибка сервера (HTTP {})", code.as_u16()),
        });
    }
    Err(last)
}
async fn probe(server: &str) -> Result<(), String> {
    let v = http(
        server,
        reqwest::Method::GET,
        "/v1/health",
        None,
        Value::Null,
    )
    .await?;
    if v["service"] != "nexusbro-sync" || v["protocol"] != 1 {
        return Err("Это не отдельный сервер синхронизации NexusBro".into());
    }
    Ok(())
}
async fn derive(
    password: Zeroizing<String>,
    salt: String,
    params: String,
) -> Result<(Zeroizing<String>, Zeroizing<[u8; 32]>), String> {
    tokio::task::spawn_blocking(move || {
        let salt = B64.decode(salt).map_err(|_| "Неверная соль аккаунта")?;
        let params =
            ac::KdfParams::from_str(&params).map_err(|_| "Неверные параметры шифрования")?;
        params
            .validate()
            .map_err(|_| "Недопустимые параметры шифрования")?;
        let master = ac::derive_master_key(&password, &salt, &params)
            .map_err(|_| "Не удалось обработать пароль")?;
        let auth = ac::auth_hash(&master, &salt).map_err(|_| "Ошибка ключа аккаунта")?;
        Ok((Zeroizing::new(B64.encode(auth)), master))
    })
    .await
    .map_err(|_| "Ошибка обработки ключа аккаунта")?
}
fn key(session: &Session) -> Result<Zeroizing<[u8; 32]>, String> {
    let bytes = Zeroizing::new(
        B64.decode(&session.key)
            .map_err(|_| "Повреждён ключ аккаунта")?,
    );
    let mut out = Zeroizing::new([0; 32]);
    if bytes.len() != 32 {
        return Err("Повреждён ключ аккаунта".into());
    }
    out.copy_from_slice(&bytes);
    Ok(out)
}
fn reset_identity(c: &mut Config, username: &str, uid: &str) {
    if c.username != username || c.session.as_ref().is_some_and(|s| s.user_id != uid) {
        c.tracks.clear();
        c.watermark = 0;
    }
    c.username = username.into();
}
fn make_session(
    response: &Value,
    user_key: &[u8; 32],
    salt: &str,
    params: &str,
) -> Result<Session, String> {
    if response["account_salt"] != salt || response["kdf_params"] != params {
        return Err("Сервер изменил параметры аккаунта во время входа".into());
    }
    Ok(Session {
        token: text(response, "token", 512)?,
        user_id: text(response, "user_id", 128)?,
        key: B64.encode(user_key),
        account_salt: salt.into(),
        kdf_params: params.into(),
        wrapped_user_key: text(response, "wrapped_user_key", 512)?,
        totp_enabled: response["totp_enabled"] == true,
    })
}
impl AccountState {
    pub async fn dispatch(
        &self,
        vault: &VaultState,
        action: &str,
        mut data: Value,
    ) -> Result<Value, String> {
        let _guard = self.serial.lock().await;
        let mut cfg = load(vault)?;
        let result:Result<Value,String>=async {
        if action == "status" {
            return Ok(status(&cfg));
        }
        if action == "configure" {
            let server = endpoint(&text(&data, "server", 2048)?)?;
            probe(&server).await?;
            if cfg.server != server {
                cfg = Config {
                    server,
                    ..Default::default()
                };
            }
            save(vault, &cfg)?;
            return Ok(status(&cfg));
        }
        if cfg.server.is_empty() {
            return Err("Сервер аккаунтов NexusBro ещё не настроен".into());
        }
        // Never send a browser verifier to a legacy SSH service or a redirected origin.
        probe(&cfg.server).await?;
        if cfg.device.is_empty() {
            cfg.device = format!(
                "NexusBro-{}",
                ac::format_recovery_key(&ac::generate_recovery_key())[..16].to_string()
            );
        }
        match action {
            "register" => {
                let username = text(&data, "username", 128)?.trim().to_string();
                let password = secret(&mut data, "password")?;
                strong(&password)?;
                let reg = tokio::task::spawn_blocking(move || ac::prepare_registration(&password))
                    .await
                    .map_err(|_| "Ошибка регистрации")?
                    .map_err(|_| "Не удалось создать ключи аккаунта")?;
                let mut body =
                    serde_json::to_value(&reg.payload).map_err(|_| "Ошибка регистрации")?;
                body["username"] = json!(username);
                http(
                    &cfg.server,
                    reqwest::Method::POST,
                    "/v1/register",
                    None,
                    body,
                )
                .await?;
                let recovery = ac::format_recovery_key(&reg.recovery_key);
                let login=http(&cfg.server,reqwest::Method::POST,"/v1/login",None,json!({"username":username,"auth_hash":reg.payload.auth_hash,"device_name":cfg.device})).await;
                let mut warning = None;
                match login {
                    Ok(response) => {
                        let session = make_session(
                            &response,
                            &reg.user_key,
                            &reg.payload.account_salt,
                            &reg.payload.kdf_params,
                        )?;
                        if session.wrapped_user_key != reg.payload.wrapped_user_key {
                            return Err("Сервер изменил ключи после регистрации".into());
                        }
                        reset_identity(&mut cfg, &username, &session.user_id);
                        cfg.session = Some(session);
                    }
                    Err(e) => {
                        cfg.username = username;
                        cfg.session = None;
                        warning = Some(e);
                    }
                }
                save(vault, &cfg)?;
                let mut result = status(&cfg);
                result["recovery_key"] = json!(recovery);
                if let Some(w) = warning {
                    result["warning"] = json!(w);
                }
                Ok(result)
            }
            "login" => {
                let username = text(&data, "username", 128)?.trim().to_string();
                let password = secret(&mut data, "password")?;
                let pre = http(
                    &cfg.server,
                    reqwest::Method::POST,
                    "/v1/prelogin",
                    None,
                    json!({"username":username}),
                )
                .await?;
                let salt = text(&pre, "account_salt", 128)?;
                let params = text(&pre, "kdf_params", 512)?;
                let (auth, master) = derive(password, salt.clone(), params.clone()).await?;
                let mut body =
                    json!({"username":username,"auth_hash":auth.as_str(),"device_name":cfg.device});
                for name in ["totp", "recovery_code"] {
                    if let Some(v) = data
                        .get(name)
                        .filter(|v| v.as_str().is_some_and(|s| !s.is_empty() && s.len() <= 128))
                    {
                        body[name] = v.clone();
                    }
                }
                let response =
                    http(&cfg.server, reqwest::Method::POST, "/v1/login", None, body).await?;
                let wrapping = ac::wrap_key(&master).map_err(|_| "Ошибка ключа аккаунта")?;
                let wrapped = B64
                    .decode(text(&response, "wrapped_user_key", 512)?)
                    .map_err(|_| "Неверный ключ сервера")?;
                let uk = ac::unwrap_user_key(&wrapped, &wrapping)
                    .map_err(|_| "Не удалось расшифровать ключ аккаунта")?;
                let session = make_session(&response, &uk, &salt, &params)?;
                reset_identity(&mut cfg, &username, &session.user_id);
                cfg.session = Some(session);
                save(vault, &cfg)?;
                Ok(status(&cfg))
            }
            "logout" => {
                let revoked = if let Some(s) = cfg.session.as_ref() {
                    http(
                        &cfg.server,
                        reqwest::Method::POST,
                        "/v1/logout",
                        Some(&s.token),
                        json!({}),
                    )
                    .await
                    .is_ok()
                } else {
                    true
                };
                cfg.session = None;
                save(vault, &cfg)?;
                let mut out = status(&cfg);
                out["server_revoked"] = json!(revoked);
                Ok(out)
            }
            "totp_enroll" | "totp_verify" | "totp_disable" => {
                let s = cfg.session.as_ref().ok_or("Войдите в аккаунт")?;
                let path = match action {
                    "totp_enroll" => "/v1/totp/enroll",
                    "totp_verify" => "/v1/totp/verify",
                    _ => "/v1/totp/disable",
                };
                let body = if action == "totp_enroll" {
                    json!({})
                } else {
                    json!({"code":text(&data,"code",128)?})
                };
                let mut result = http(
                    &cfg.server,
                    reqwest::Method::POST,
                    path,
                    Some(&s.token),
                    body,
                )
                .await?;
                if action == "totp_enroll" {
                    result["otpauth_uri"] = result["otpauth_url"].take();
                    return Ok(result);
                }
                cfg.session.as_mut().unwrap().totp_enabled = action == "totp_verify";
                save(vault, &cfg)?;
                let mut out = status(&cfg);
                if let Some(codes) = result.get("recovery_codes") {
                    out["recovery_codes"] = codes.clone();
                }
                Ok(out)
            }
            "change_password" | "delete_account" => {
                let s = cfg.session.as_ref().ok_or("Войдите в аккаунт")?;
                let old = secret(
                    &mut data,
                    if action == "delete_account" {
                        "password"
                    } else {
                        "old_password"
                    },
                )?;
                let (auth, master) =
                    derive(old, s.account_salt.clone(), s.kdf_params.clone()).await?;
                let wrapping = ac::wrap_key(&master).map_err(|_| "Ошибка ключа")?;
                let stored = B64
                    .decode(&s.wrapped_user_key)
                    .map_err(|_| "Повреждён ключ")?;
                let old_key = ac::unwrap_user_key(&stored, &wrapping)
                    .map_err(|_| "Неверный текущий пароль")?;
                let uk = key(s)?;
                if old_key
                    .iter()
                    .zip(uk.iter())
                    .fold(0u8, |a, (b, c)| a | (b ^ c))
                    != 0
                {
                    return Err("Неверный текущий пароль".into());
                }
                let totp = data
                    .get("totp")
                    .and_then(Value::as_str)
                    .filter(|s| s.len() <= 128)
                    .map(str::to_owned);
                if action == "delete_account" {
                    http(
                        &cfg.server,
                        reqwest::Method::DELETE,
                        "/v1/account",
                        Some(&s.token),
                        json!({"auth_hash":auth.as_str(),"totp":totp}),
                    )
                    .await?;
                    cfg.session = None;
                    cfg.tracks.clear();
                    cfg.watermark = 0;
                    cfg.username.clear();
                    save(vault, &cfg)?;
                    return Ok(status(&cfg));
                }
                let new = secret(&mut data, "new_password")?;
                strong(&new)?;
                let salt = s.account_salt.clone();
                let params = s.kdf_params.clone();
                let rekey = tokio::task::spawn_blocking(move || {
                    ac::rekey_password(&uk, &new, &salt, &params)
                })
                .await
                .map_err(|_| "Ошибка смены пароля")?
                .map_err(|_| "Не удалось обработать новый пароль")?;
                http(&cfg.server,reqwest::Method::POST,"/v1/credentials",Some(&s.token),json!({"current_auth_hash":auth.as_str(),"totp":totp,"auth_hash":rekey.auth_hash,"wrapped_user_key":rekey.wrapped_user_key})).await?;
                cfg.session.as_mut().unwrap().wrapped_user_key = rekey.wrapped_user_key;
                save(vault, &cfg)?;
                Ok(status(&cfg))
            }
            "recover" => {
                let username = text(&data, "username", 128)?.trim().to_string();
                let recovery = secret(&mut data, "recovery_key")?;
                let new = secret(&mut data, "new_password")?;
                strong(&new)?;
                let pre = http(
                    &cfg.server,
                    reqwest::Method::POST,
                    "/v1/prelogin",
                    None,
                    json!({"username":username}),
                )
                .await?;
                let salt = text(&pre, "account_salt", 128)?;
                let params = text(&pre, "kdf_params", 512)?;
                ac::KdfParams::from_str(&params)
                    .and_then(|p| p.validate())
                    .map_err(|_| "Недопустимые параметры шифрования")?;
                let auth = ac::recovery_auth_hash_from_kit(&recovery, &salt)
                    .map_err(|_| "Неверный ключ восстановления")?;
                let response = http(
                    &cfg.server,
                    reqwest::Method::POST,
                    "/v1/recovery-login",
                    None,
                    json!({"username":username,"recovery_auth_hash":auth,"device_name":cfg.device}),
                )
                .await?;
                if response["account_salt"] != salt || response["kdf_params"] != params {
                    return Err("Сервер изменил параметры восстановления".into());
                }
                let uk = ac::recover_user_key(
                    &recovery,
                    &text(&response, "recovery_wrapped_user_key", 512)?,
                )
                .map_err(|_| "Неверный ключ восстановления")?;
                let rsalt = salt.clone();
                let rparams = params.clone();
                let rekey = tokio::task::spawn_blocking(move || {
                    ac::rekey_password(&uk, &new, &rsalt, &rparams).map(|r| (r, uk))
                })
                .await
                .map_err(|_| "Ошибка восстановления")?
                .map_err(|_| "Ошибка ключа восстановления")?;
                let token = text(&response, "token", 512)?;
                http(&cfg.server,reqwest::Method::POST,"/v1/credentials",Some(&token),json!({"auth_hash":rekey.0.auth_hash,"wrapped_user_key":rekey.0.wrapped_user_key})).await?;
                let uid = text(&response, "user_id", 128)?;
                reset_identity(&mut cfg, &username, &uid);
                cfg.session = Some(Session {
                    token,
                    user_id: uid,
                    key: B64.encode(&*rekey.1),
                    account_salt: salt,
                    kdf_params: params,
                    wrapped_user_key: rekey.0.wrapped_user_key,
                    totp_enabled: false,
                });
                save(vault, &cfg)?;
                Ok(status(&cfg))
            }
            _ => Err("Неизвестное действие аккаунта".into()),
        }
        }.await;
        if result
            .as_ref()
            .err()
            .is_some_and(|e| e.starts_with("ACCOUNT_SESSION_EXPIRED"))
        {
            cfg.session = None;
            let _ = save(vault, &cfg);
        }
        result
    }
    pub async fn prepare_resolution(
        &self,
        vault: &VaultState,
        id: &str,
        choice: &str,
    ) -> Result<SyncItem, String> {
        let _guard = self.serial.lock().await;
        if !matches!(choice, "local" | "remote") {
            return Err("Неверный выбор разрешения конфликта".into());
        }
        let cfg = load(vault)?;
        let session = cfg.session.as_ref().ok_or("Войдите в аккаунт")?;
        let raw = vault
            .get(&format!("account.conflict.{id}"))?
            .ok_or("Конфликт уже разрешён или отсутствует")?;
        let record: Conflict =
            serde_json::from_str(&raw).map_err(|_| "Повреждены данные конфликта")?;
        if record.account_id != session.user_id
            || record.server != cfg.server
            || record.remote.id != id
            || cfg.tracks.get(id).is_none_or(|t| t.rev != record.revision)
        {
            return Err("Конфликт относится к другому аккаунту или изменился".into());
        }
        validate_item(&record.remote)?;
        let mut selected = if choice == "remote" {
            record.remote.clone()
        } else {
            record.local.clone().ok_or("Локальная копия отсутствует")?
        };
        validate_item(&selected)?;
        if selected.id != id {
            return Err("Перепутаны данные конфликта".into());
        }
        if choice == "local" {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|_| "Неверное системное время")?
                .as_millis()
                .min(i64::MAX as u128 - 1) as u64;
            selected.modified = now
                .max(record.remote.modified.saturating_add(1))
                .max(selected.modified.saturating_add(1));
            validate_item(&selected)?;
        }
        let receipt = Resolution {
            choice: choice.into(),
            hash: B64.encode(Sha256::digest(raw.as_bytes())),
            account_id: session.user_id.clone(),
            server: cfg.server.clone(),
        };
        let data = Zeroizing::new(
            serde_json::to_string(&receipt)
                .map_err(|_| "Ошибка подготовки разрешения конфликта")?,
        );
        vault.put(&format!("account.resolution.{id}"), &data)?;
        Ok(selected)
    }
    pub async fn commit_resolution(
        &self,
        vault: &VaultState,
        id: &str,
        choice: &str,
    ) -> Result<(), String> {
        let _guard = self.serial.lock().await;
        let mut cfg = load(vault)?;
        let session = cfg.session.as_ref().ok_or("Войдите в аккаунт")?;
        let raw = vault
            .get(&format!("account.conflict.{id}"))?
            .ok_or("Конфликт уже разрешён или отсутствует")?;
        let receipt = vault
            .get(&format!("account.resolution.{id}"))?
            .ok_or("Сначала подготовьте разрешение конфликта")?;
        let receipt: Resolution =
            serde_json::from_str(&receipt).map_err(|_| "Повреждено подтверждение разрешения")?;
        let record: Conflict =
            serde_json::from_str(&raw).map_err(|_| "Повреждены данные конфликта")?;
        if receipt.choice != choice
            || receipt.hash != B64.encode(Sha256::digest(raw.as_bytes()))
            || receipt.account_id != session.user_id
            || receipt.server != cfg.server
            || record.account_id != session.user_id
            || record.server != cfg.server
            || record.remote.id != id
            || cfg.tracks.get(id).is_none_or(|t| t.rev != record.revision)
        {
            return Err("Конфликт изменился до подтверждения. Копии сохранены".into());
        }
        validate_item(&record.remote)?;
        cfg.tracks.insert(
            id.into(),
            Track {
                rev: record.revision,
                digest: Some(fingerprint(&record.remote)?),
                modified: record.remote.modified,
            },
        );
        // Root calls commit ONLY after applying the selected native record successfully.
        save(vault, &cfg)?;
        vault.remove(&format!("account.resolution.{id}"))?;
        vault.remove(&format!("account.conflict.{id}"))?;
        Ok(())
    }
    pub async fn exchange(
        &self,
        vault: &VaultState,
        items: Vec<SyncItem>,
    ) -> Result<SyncExchange, String> {
        let _guard = self.serial.lock().await;
        let mut cfg = load(vault)?;
        let result:Result<SyncExchange,String>=async {
        let session = cfg.session.as_ref().ok_or("Войдите в аккаунт")?;
        let uk = key(session)?;
        if items.len() > 20000 {
            return Err("Слишком много элементов синхронизации".into());
        }
        let mut local = BTreeMap::new();
        for item in items {
            validate_item(&item)?;
            if local.insert(item.id.clone(), item).is_some() {
                return Err("Повторяющийся идентификатор синхронизации".into());
            }
        }
        probe(&cfg.server).await?;
        let mut since = 0;
        let mut latest = 0;
        let mut remote = BTreeMap::new();
        let mut total = 0usize;
        for _ in 0..2501 {
            let page = http(
                &cfg.server,
                reqwest::Method::GET,
                &format!("/v1/items?since={since}"),
                Some(&session.token),
                Value::Null,
            )
            .await?;
            latest = page["latest_rev"]
                .as_u64()
                .ok_or("Неверная ревизия синхронизации")?;
            if latest < cfg.watermark {
                return Err("История сервера откатилась. Локальные данные сохранены; требуется проверка восстановления".into());
            }
            let rows = page["items"]
                .as_array()
                .ok_or("Неверный список синхронизации")?;
            if rows.is_empty() {
                if since < latest {
                    return Err("Сервер пропустил элементы синхронизации".into());
                }
                break;
            }
            for row in rows {
                let w: WireItem = serde_json::from_value(row.clone())
                    .map_err(|_| "Неверный элемент синхронизации")?;
                if w.rev <= since || w.rev > latest || w.kind != "browser-item" {
                    return Err("Неверная последовательность синхронизации".into());
                }
                since = w.rev;
                let item = open_item(&w, &uk, &session.user_id)?;
                total += item.value.len();
                if remote.len() >= 20000 || total > 64 * 1024 * 1024 {
                    return Err("Превышен безопасный объём синхронизации".into());
                }
                if cfg
                    .tracks
                    .get(&item.id)
                    .is_some_and(|t| t.rev > w.rev || t.modified > item.modified)
                {
                    return Err("Обнаружен откат элемента синхронизации; данные сохранены".into());
                }
                remote.insert(item.id.clone(), (w.rev, item));
            }
            if since >= latest {
                break;
            }
        }
        if since < latest {
            return Err("Слишком много страниц синхронизации".into());
        }
        if cfg.tracks.keys().any(|id| !remote.contains_key(id)) {
            return Err("На сервере исчез известный элемент. Локальные данные сохранены".into());
        }
        let mut uploads = Vec::new();
        let mut conflicts = HashSet::new();
        for (id, item) in &local {
            let digest = fingerprint(item)?;
            if let Some((rev, theirs)) = remote.get(id) {
                let rd = fingerprint(theirs)?;
                let track = cfg.tracks.entry(id.clone()).or_default();
                track.rev = *rev;
                track.modified = theirs.modified;
                if digest == rd {
                    track.digest = Some(digest);
                    continue;
                }
                if track.digest.as_ref() == Some(&digest) {
                    continue;
                } // unchanged locally: root may apply remote
                if track.digest.as_ref() != Some(&rd) {
                    conflicts.insert(id.clone());
                    continue;
                }
                if item.modified < theirs.modified {
                    conflicts.insert(id.clone());
                    continue;
                }
                uploads.push((item.clone(), *rev, digest));
            } else {
                uploads.push((item.clone(), 0, digest));
            }
        }
        let mut pushed = 0;
        for batch in uploads.chunks(8) {
            let mut changes = Vec::new();
            for (item, base, _) in batch {
                let clear =
                    Zeroizing::new(serde_json::to_vec(item).map_err(|_| "Ошибка элемента")?);
                let aad = format!("nexusbro-item-v1|{}|{}", session.user_id, item.id);
                let ct = ac::encrypt_item(&clear, &uk, aad.as_bytes())
                    .map_err(|_| "Ошибка шифрования элемента")?;
                changes.push(json!({"item_id":item.id,"type":"browser-item","ciphertext":B64.encode(ct),"updated_at":item.modified,"deleted":item.deleted,"base_rev":base}));
            }
            let result = http(
                &cfg.server,
                reqwest::Method::POST,
                "/v1/items",
                Some(&session.token),
                json!({"changes":changes}),
            )
            .await?;
            let rows = result["results"]
                .as_array()
                .ok_or("Неверный результат синхронизации")?;
            if rows.len() != batch.len() {
                return Err("Сервер пропустил результат записи".into());
            }
            for ((item, _, digest), row) in batch.iter().zip(rows) {
                if row["item_id"] != item.id {
                    return Err("Перепутаны результаты синхронизации".into());
                }
                if row["status"] == "ok" {
                    let rev = row["rev"].as_u64().ok_or("Неверная ревизия записи")?;
                    cfg.tracks.insert(
                        item.id.clone(),
                        Track {
                            rev,
                            digest: Some(digest.clone()),
                            modified: item.modified,
                        },
                    );
                    remote.insert(item.id.clone(), (rev, item.clone()));
                    pushed += 1;
                } else if row["status"] == "conflict" {
                    conflicts.insert(item.id.clone());
                    if !row["server"].is_null() {
                        let w: WireItem = serde_json::from_value(row["server"].clone())
                            .map_err(|_| "Неверный конфликт")?;
                        let clear = open_item(&w, &uk, &session.user_id)?;
                        remote.insert(clear.id.clone(), (w.rev, clear));
                    }
                } else {
                    return Err("Неизвестный результат синхронизации".into());
                }
            }
            latest = latest.max(
                result["latest_rev"]
                    .as_u64()
                    .ok_or("Неверная ревизия записи")?,
            );
        }
        let mut returned = Vec::new();
        for (id, (rev, item)) in remote {
            let track = cfg.tracks.entry(id.clone()).or_default();
            track.rev = rev;
            track.modified = item.modified;
            if conflicts.contains(&id) {
                let stored = Zeroizing::new(
                    serde_json::to_string(&Conflict{local:local.get(&id).cloned(),remote:item,revision:rev,account_id:session.user_id.clone(),server:cfg.server.clone()})
                        .map_err(|_| "Ошибка сохранения конфликта")?,
                );
                vault.put(&format!("account.conflict.{}", id), &stored)?;
            } else {
                returned.push(item);
            }
        }
        cfg.watermark = latest;
        save(vault, &cfg)?;
        let mut conflicts: Vec<_> = conflicts.into_iter().collect();
        conflicts.sort();
        Ok(SyncExchange {
            remote: returned,
            pushed,
            conflicts,
        })
        }.await;
        if result
            .as_ref()
            .err()
            .is_some_and(|e| e.starts_with("ACCOUNT_SESSION_EXPIRED"))
        {
            cfg.session = None;
            let _ = save(vault, &cfg);
        }
        result
    }
}
impl std::fmt::Debug for SyncItem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SyncItem")
            .field("id", &self.id)
            .field("value", &"[REDACTED]")
            .field("modified", &self.modified)
            .field("deleted", &self.deleted)
            .finish()
    }
}
fn validate_item(item: &SyncItem) -> Result<(), String> {
    let mut parts = item.id.split('/');
    if parts.next() != Some("nexusbro.v1")
        || !matches!(
            parts.next(),
            Some(
                "settings"
                    | "setting"
                    | "bookmarks"
                    | "bookmark"
                    | "supers"
                    | "super"
                    | "passwords"
                    | "password"
            )
        )
        || parts.next().is_none()
        || item.id.len() > 256
        || item.id.chars().any(char::is_control)
        || item.value.len() > ITEM_LIMIT
        || item.modified > i64::MAX as u64
        || item.modified == 0
        || item.deleted && !item.value.is_empty()
    {
        return Err("Недопустимый элемент синхронизации".into());
    }
    Ok(())
}
fn fingerprint(item: &SyncItem) -> Result<String, String> {
    let data = Zeroizing::new(serde_json::to_vec(item).map_err(|_| "Ошибка элемента")?);
    Ok(B64.encode(Sha256::digest(&*data)))
}
fn open_item(w: &WireItem, key: &[u8; 32], uid: &str) -> Result<SyncItem, String> {
    let ciphertext = B64
        .decode(&w.ciphertext)
        .map_err(|_| "Неверное шифрование элемента")?;
    if ciphertext.len() > 256 * 1024 {
        return Err("Слишком большой элемент".into());
    }
    let aad = format!("nexusbro-item-v1|{}|{}", uid, w.item_id);
    let plain = Zeroizing::new(
        ac::decrypt_item(&ciphertext, key, aad.as_bytes())
            .map_err(|_| "Проверка подлинности элемента не пройдена")?,
    );
    let item: SyncItem =
        serde_json::from_slice(&plain).map_err(|_| "Повреждён элемент синхронизации")?;
    validate_item(&item)?;
    if item.id != w.item_id || item.modified != w.updated_at || item.deleted != w.deleted {
        return Err("Подменены метаданные элемента синхронизации".into());
    }
    Ok(item)
}
