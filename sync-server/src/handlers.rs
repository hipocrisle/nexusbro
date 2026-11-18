// Adapted from NexuSSH, Copyright (c) 2026 hipocrisle, MIT; see ../LICENSE.
//! HTTP handlers + auth middleware for the Phase-0 account/auth/2FA/session API.

use axum::{
    extract::{ConnectInfo, State},
    http::{HeaderMap, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::net::SocketAddr;
use std::time::{SystemTime, UNIX_EPOCH};
use totp_rs::{Algorithm, Secret, TOTP};

use crate::crypto;
use crate::AppState;

const SESSION_TTL_SECS: i64 = 30 * 24 * 60 * 60; // 30 days
const RECOVERY_CODE_COUNT: usize = 10;
const TOTP_ISSUER: &str = "NexusBro";

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

/// The real client IP for rate-limiting. The service runs behind nginx, so the
/// TCP peer (`addr`) is always the loopback proxy — keying the limiter on it
/// collapses every client to one bucket (no real per-IP throttle on credential
/// guessing). Prefer the forwarded real IP: `X-Real-IP`, else the LAST hop of
/// `X-Forwarded-For` (nginx appends the true client there; earlier hops are
/// client-spoofable), else fall back to the peer.
/// NOTE: requires nginx to set `proxy_set_header X-Real-IP $remote_addr;`
/// (or X-Forwarded-For) — otherwise this safely falls back to the peer IP.
fn client_ip(headers: &HeaderMap, addr: &SocketAddr) -> String {
    if !addr.ip().is_loopback() {
        return addr.ip().to_string();
    }
    if let Some(v) = headers.get("x-real-ip").and_then(|h| h.to_str().ok()) {
        let v = v.trim();
        if !v.is_empty() {
            return v.to_string();
        }
    }
    if let Some(v) = headers.get("x-forwarded-for").and_then(|h| h.to_str().ok()) {
        if let Some(last) = v.split(',').map(str::trim).filter(|s| !s.is_empty()).last() {
            return last.to_string();
        }
    }
    addr.ip().to_string()
}

// ---------------------------------------------------------------------------
// Error helper
// ---------------------------------------------------------------------------

pub struct ApiError(StatusCode, serde_json::Value);

impl ApiError {
    pub fn new(code: StatusCode, msg: &str) -> Self {
        ApiError(code, json!({ "error": msg }))
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(self.1)).into_response()
    }
}

type ApiResult<T> = Result<T, ApiError>;

pub fn db_err<E: std::fmt::Display>(e: E) -> ApiError {
    tracing::error!("db error: {}", e);
    ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal error")
}

// ---------------------------------------------------------------------------
// Authenticated-user context, injected by the session middleware.
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct AuthUser {
    pub user_id: String,
    #[allow(dead_code)]
    pub device_id: String,
    pub token_hash: String,
    pub recovery: bool,
}

/// Bearer-token session middleware. Looks up the session, checks expiry,
/// refreshes the device's last_seen, and attaches `AuthUser` to extensions.
pub async fn require_auth(
    State(state): State<AppState>,
    mut req: axum::extract::Request,
    next: Next,
) -> Response {
    let token = req
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .map(|s| s.to_string());

    let Some(token) = token else {
        return ApiError::new(StatusCode::UNAUTHORIZED, "missing bearer token").into_response();
    };

    let token_hash = crypto::token_hash(&token);
    let lookup = {
        let conn = state.db.conn.lock().unwrap();
        conn.query_row(
            "SELECT user_id, device_id, expires_at,recovery FROM sessions WHERE token = ?1",
            [&token_hash],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, i64>(3)? != 0,
                ))
            },
        )
        .optional()
    };

    let (user_id, device_id, expires_at, recovery) = match lookup {
        Ok(Some(row)) => row,
        Ok(None) => {
            return ApiError::new(StatusCode::UNAUTHORIZED, "invalid token").into_response()
        }
        Err(e) => return db_err(e).into_response(),
    };

    if expires_at < now() {
        let conn = state.db.conn.lock().unwrap();
        let _ = conn.execute("DELETE FROM sessions WHERE token = ?1", [&token_hash]);
        return ApiError::new(StatusCode::UNAUTHORIZED, "session expired").into_response();
    }

    {
        let conn = state.db.conn.lock().unwrap();
        let _ = conn.execute(
            "UPDATE devices SET last_seen = ?1 WHERE id = ?2",
            rusqlite::params![now(), device_id],
        );
        // Sliding session: renew the expiry on use, so an ACTIVE device is not
        // logged out every 30 days from login regardless of activity (that fixed
        // 30d-from-login expiry is exactly what produced the "invalid token"
        // surprise). Only bump when it has drifted by >1 day, to avoid a write
        // on every single request.
        if expires_at - now() < SESSION_TTL_SECS - 86400 {
            let _ = conn.execute(
                "UPDATE sessions SET expires_at = ?1 WHERE token = ?2",
                rusqlite::params![now() + SESSION_TTL_SECS, &token_hash],
            );
        }
    }

    if recovery && !matches!(req.uri().path(), "/v1/credentials" | "/v1/logout") {
        return ApiError::new(StatusCode::FORBIDDEN, "complete recovery first").into_response();
    }
    req.extensions_mut().insert(AuthUser {
        user_id,
        device_id,
        token_hash,
        recovery,
    });
    next.run(req).await
}

// ---------------------------------------------------------------------------
// Health
// ---------------------------------------------------------------------------

pub async fn health() -> impl IntoResponse {
    Json(json!({ "status": "ok", "service":"nexusbro-sync", "protocol":1 }))
}

// ---------------------------------------------------------------------------
// Register
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct RegisterReq {
    pub username: String,
    /// Client KDF output (HKDF(masterKey,"nexusbro-auth")), base64. NOT stored raw.
    pub auth_hash: String,
    /// Opaque blobs stored verbatim, returned to the client at login.
    pub account_salt: String,
    pub kdf_params: String,
    pub wrapped_user_key: String,
    pub recovery_wrapped_user_key: Option<String>,
    /// Client KDF output derived from the RECOVERY key (mirror of `auth_hash`).
    /// Stored as an argon2 verifier so a no-password recovery-login can prove
    /// possession of the recovery key. Optional for back-compat.
    pub recovery_auth_hash: Option<String>,
}

#[derive(Serialize)]
pub struct RegisterResp {
    pub user_id: String,
}

pub async fn register(
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(req): Json<RegisterReq>,
) -> ApiResult<impl IntoResponse> {
    if !state
        .ratelimit
        .check(&format!("register-ip|{}", client_ip(&headers, &addr)))
    {
        return Err(ApiError::new(StatusCode::TOO_MANY_REQUESTS, "rate limited"));
    }
    let username = req.username.trim().to_string();
    if username.is_empty() || username.len() > 128 {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "invalid username"));
    }
    if req.auth_hash.len() > 512
        || req.account_salt.len() > 128
        || req.kdf_params.len() > 512
        || req.wrapped_user_key.len() > 512
        || req
            .recovery_wrapped_user_key
            .as_ref()
            .is_some_and(|v| v.len() > 512)
        || req
            .recovery_auth_hash
            .as_ref()
            .is_some_and(|v| v.len() > 512)
    {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid registration fields",
        ));
    }
    if req.auth_hash.is_empty() || req.account_salt.is_empty() || req.wrapped_user_key.is_empty() {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "missing fields"));
    }

    // Wrap the client auth_hash with a fresh random salt (server never stores it raw).
    let server_hash = crypto::hash_auth(&req.auth_hash)
        .map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "hash error"))?;
    // Same for the recovery-key auth hash, if the client provided one.
    let recovery_hash = match req.recovery_auth_hash.as_deref().filter(|h| !h.is_empty()) {
        Some(h) => Some(
            crypto::hash_auth(h)
                .map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "hash error"))?,
        ),
        None => None,
    };
    let user_id = crypto::new_uuid();

    let conn = state.db.conn.lock().unwrap();
    let res = conn.execute(
        "INSERT INTO users
            (id, username, server_hash, account_salt, kdf_params,
             wrapped_user_key, recovery_wrapped_user_key, recovery_hash,
             totp_secret, totp_enabled, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, NULL, 0, ?9)",
        rusqlite::params![
            user_id,
            username,
            server_hash,
            req.account_salt,
            req.kdf_params,
            req.wrapped_user_key,
            req.recovery_wrapped_user_key,
            recovery_hash,
            now(),
        ],
    );

    match res {
        Ok(_) => Ok((StatusCode::CREATED, Json(RegisterResp { user_id }))),
        Err(rusqlite::Error::SqliteFailure(e, _))
            if e.code == rusqlite::ErrorCode::ConstraintViolation =>
        {
            Err(ApiError::new(StatusCode::CONFLICT, "username taken"))
        }
        Err(e) => Err(db_err(e)),
    }
}

// ---------------------------------------------------------------------------
// Prelogin: client needs account_salt + kdf_params to compute auth_hash.
// To avoid username enumeration we return deterministic dummy values for
// unknown usernames (stable per-username via HMAC-less hashing of the name).
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct PreloginReq {
    pub username: String,
}

#[derive(Serialize)]
pub struct PreloginResp {
    pub account_salt: String,
    pub kdf_params: String,
}

pub async fn prelogin(
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(req): Json<PreloginReq>,
) -> ApiResult<impl IntoResponse> {
    let username = req.username.trim().to_string();
    if username.is_empty() || username.len() > 128 {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "invalid username"));
    }
    let rl_key = format!("prelogin|{}|{}", username, client_ip(&headers, &addr));
    if !state.ratelimit.check(&rl_key) {
        return Err(ApiError::new(StatusCode::TOO_MANY_REQUESTS, "rate limited"));
    }

    let row = {
        let conn = state.db.conn.lock().unwrap();
        conn.query_row(
            "SELECT account_salt, kdf_params FROM users WHERE username = ?1",
            [&username],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
        )
        .optional()
        .map_err(db_err)?
    };

    let (account_salt, kdf_params) = match row {
        Some(v) => v,
        None => {
            // Deterministic dummy salt derived from username so repeated probes
            // get the same answer (real accounts also return a stable salt).
            let dummy_salt = crypto::hash_recovery_code(&format!("nexusbro-dummy-salt|{username}"));
            let salt_b64 = base64::Engine::encode(
                &base64::engine::general_purpose::STANDARD,
                &dummy_salt.as_bytes()[..16],
            );
            (
                salt_b64,
                json!({"m_cost":19456,"t_cost":2,"p_cost":1}).to_string(),
            )
        }
    };

    Ok(Json(PreloginResp {
        account_salt,
        kdf_params,
    }))
}

// ---------------------------------------------------------------------------
// Login: verify auth_hash, gate on TOTP, issue a session, return wrapped key.
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct LoginReq {
    pub username: String,
    pub auth_hash: String,
    pub device_name: Option<String>,
    pub totp: Option<String>,
    /// Optional one-time recovery code, used if the 2FA device is lost.
    pub recovery_code: Option<String>,
}

#[derive(Serialize)]
pub struct LoginResp {
    pub token: String,
    pub user_id: String,
    pub device_id: String,
    pub account_salt: String,
    pub kdf_params: String,
    pub wrapped_user_key: String,
    pub totp_enabled: bool,
}

struct UserRow {
    id: String,
    server_hash: String,
    account_salt: String,
    kdf_params: String,
    wrapped_user_key: String,
    totp_secret: Option<String>,
    totp_enabled: bool,
}

fn load_user_by_name(state: &AppState, username: &str) -> ApiResult<Option<UserRow>> {
    let conn = state.db.conn.lock().unwrap();
    conn.query_row(
        "SELECT id, server_hash, account_salt, kdf_params, wrapped_user_key,
                totp_secret, totp_enabled
         FROM users WHERE username = ?1",
        [username],
        |r| {
            Ok(UserRow {
                id: r.get(0)?,
                server_hash: r.get(1)?,
                account_salt: r.get(2)?,
                kdf_params: r.get(3)?,
                wrapped_user_key: r.get(4)?,
                totp_secret: r.get(5)?,
                totp_enabled: r.get::<_, i64>(6)? != 0,
            })
        },
    )
    .optional()
    .map_err(db_err)
}

/// Build a verification-only TOTP from a stored base32 secret. The account name
/// is irrelevant for code checking, so we pass a placeholder.
fn make_totp(secret_b32: &str) -> Result<TOTP, ApiError> {
    make_totp_named(secret_b32, "user")
}

/// Build a TOTP for a specific account (used to produce the otpauth:// URL).
fn make_totp_named(secret_b32: &str, account: &str) -> Result<TOTP, ApiError> {
    let bytes = Secret::Encoded(secret_b32.to_string())
        .to_bytes()
        .map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "totp secret error"))?;
    TOTP::new(
        Algorithm::SHA1,
        6,
        1,
        30,
        bytes,
        Some(TOTP_ISSUER.to_string()),
        account.to_string(),
    )
    .map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "totp init error"))
}

/// Check a recovery code: if it matches an unused stored hash, mark it used.
fn consume_recovery_code(state: &AppState, user_id: &str, code: &str) -> ApiResult<bool> {
    if code.len() > 128 {
        return Ok(false);
    }
    let target = crypto::hash_recovery_code(code);
    let conn = state.db.conn.lock().unwrap();
    let mut stmt = conn
        .prepare("SELECT code_hash FROM recovery_codes WHERE user_id = ?1 AND used = 0")
        .map_err(db_err)?;
    let hashes: Vec<String> = stmt
        .query_map([user_id], |r| r.get::<_, String>(0))
        .map_err(db_err)?
        .filter_map(|x| x.ok())
        .collect();
    drop(stmt);
    for h in hashes {
        if crypto::recovery_hash_eq(&h, &target) {
            conn.execute(
                "UPDATE recovery_codes SET used = 1 WHERE user_id = ?1 AND code_hash = ?2",
                rusqlite::params![user_id, h],
            )
            .map_err(db_err)?;
            return Ok(true);
        }
    }
    Ok(false)
}

pub async fn login(
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(req): Json<LoginReq>,
) -> ApiResult<impl IntoResponse> {
    let username = req.username.trim().to_string();
    if username.len() > 128
        || req.auth_hash.len() > 512
        || req.device_name.as_ref().is_some_and(|v| v.len() > 128)
    {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid login fields",
        ));
    }
    if !state
        .ratelimit
        .check(&format!("login-ip|{}", client_ip(&headers, &addr)))
    {
        return Err(ApiError::new(StatusCode::TOO_MANY_REQUESTS, "rate limited"));
    }
    let rl_key = format!("login|{}|{}", username, client_ip(&headers, &addr));
    if !state.ratelimit.check(&rl_key) {
        return Err(ApiError::new(StatusCode::TOO_MANY_REQUESTS, "rate limited"));
    }

    let user = load_user_by_name(&state, &username)?;
    let Some(user) = user else {
        // Burn the same Argon2 verify time as a real login so a missing account
        // isn't measurably faster (username enumeration). Same generic message.
        crypto::dummy_verify(&req.auth_hash);
        return Err(ApiError::new(
            StatusCode::UNAUTHORIZED,
            "invalid credentials",
        ));
    };

    if !crypto::verify_auth(&req.auth_hash, &user.server_hash) {
        return Err(ApiError::new(
            StatusCode::UNAUTHORIZED,
            "invalid credentials",
        ));
    }

    // Second factor.
    if user.totp_enabled {
        let mut ok = false;
        if let Some(code) = req.totp.as_deref().filter(|c| !c.is_empty()) {
            if let Some(secret) = user.totp_secret.as_deref() {
                ok = consume_totp(&state, &user.id, secret, code)?;
            }
        }
        if !ok {
            if let Some(rc) = req.recovery_code.as_deref().filter(|c| !c.is_empty()) {
                ok = consume_recovery_code(&state, &user.id, rc)?;
            }
        }
        if !ok {
            return Err(ApiError(
                StatusCode::UNAUTHORIZED,
                json!({ "totp_required": true, "error": "totp required" }),
            ));
        }
    }

    // Create/reuse a device for this name and issue a session token.
    let device_name = req
        .device_name
        .unwrap_or_else(|| "unnamed device".to_string());
    let token = crypto::random_token(32);
    let ts = now();

    let device_id = {
        let conn = state.db.conn.lock().unwrap();
        let existing: Option<String> = conn
            .query_row(
                "SELECT id FROM devices WHERE user_id = ?1 AND name = ?2",
                rusqlite::params![user.id, device_name],
                |r| r.get(0),
            )
            .optional()
            .map_err(db_err)?;
        let device_id = match existing {
            Some(id) => {
                conn.execute(
                    "UPDATE devices SET last_seen = ?1 WHERE id = ?2",
                    rusqlite::params![ts, id],
                )
                .map_err(db_err)?;
                id
            }
            None => {
                let id = crypto::new_uuid();
                conn.execute(
                    "INSERT INTO devices (id, user_id, name, created_at, last_seen)
                     VALUES (?1, ?2, ?3, ?4, ?4)",
                    rusqlite::params![id, user.id, device_name, ts],
                )
                .map_err(db_err)?;
                id
            }
        };
        conn.execute(
            "INSERT INTO sessions (token, user_id, device_id, created_at, expires_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![
                crypto::token_hash(&token),
                user.id,
                device_id,
                ts,
                ts + SESSION_TTL_SECS
            ],
        )
        .map_err(db_err)?;
        device_id
    };

    Ok(Json(LoginResp {
        token,
        user_id: user.id,
        device_id,
        account_salt: user.account_salt,
        kdf_params: user.kdf_params,
        wrapped_user_key: user.wrapped_user_key,
        totp_enabled: user.totp_enabled,
    }))
}

// ---------------------------------------------------------------------------
// TOTP enroll / verify / disable (all authenticated)
// ---------------------------------------------------------------------------

#[derive(Serialize)]
pub struct TotpEnrollResp {
    pub secret: String,
    pub otpauth_url: String,
}

pub async fn totp_enroll(
    State(state): State<AppState>,
    user: axum::Extension<AuthUser>,
) -> ApiResult<impl IntoResponse> {
    if state
        .db
        .conn
        .lock()
        .unwrap()
        .query_row(
            "SELECT totp_enabled FROM users WHERE id=?1",
            [&user.user_id],
            |r| r.get::<_, i64>(0),
        )
        .map_err(db_err)?
        != 0
    {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "disable existing 2FA before enrollment",
        ));
    }
    let username: String = {
        let conn = state.db.conn.lock().unwrap();
        conn.query_row(
            "SELECT username FROM users WHERE id = ?1",
            [&user.user_id],
            |r| r.get(0),
        )
        .map_err(db_err)?
    };

    // Fresh random base32 secret.
    let secret = Secret::generate_secret();
    let secret_b32 = match secret.to_encoded() {
        Secret::Encoded(s) => s,
        Secret::Raw(_) => unreachable!("to_encoded always yields Encoded"),
    };
    let totp = make_totp_named(&secret_b32, &username)?;
    let otpauth_url = totp.get_url();

    // Store secret but keep totp_enabled = 0 until verified.
    {
        let conn = state.db.conn.lock().unwrap();
        conn.execute(
            "UPDATE users SET totp_secret = ?1, totp_enabled = 0 WHERE id = ?2",
            rusqlite::params![
                crypto::seal_totp(&secret_b32, &state.totp_key).map_err(|_| ApiError::new(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "seed encryption failed"
                ))?,
                user.user_id
            ],
        )
        .map_err(db_err)?;
    }

    Ok(Json(TotpEnrollResp {
        secret: secret_b32,
        otpauth_url,
    }))
}

#[derive(Deserialize)]
pub struct TotpCodeReq {
    pub code: String,
}

#[derive(Serialize)]
pub struct TotpVerifyResp {
    pub totp_enabled: bool,
    pub recovery_codes: Vec<String>,
}

pub async fn totp_verify(
    State(state): State<AppState>,
    user: axum::Extension<AuthUser>,
    Json(req): Json<TotpCodeReq>,
) -> ApiResult<impl IntoResponse> {
    if !state
        .ratelimit
        .check(&format!("totp-verify|{}", user.user_id))
    {
        return Err(ApiError::new(StatusCode::TOO_MANY_REQUESTS, "rate limited"));
    }
    if state
        .db
        .conn
        .lock()
        .unwrap()
        .query_row(
            "SELECT totp_enabled FROM users WHERE id=?1",
            [&user.user_id],
            |r| r.get::<_, i64>(0),
        )
        .map_err(db_err)?
        != 0
    {
        return Err(ApiError::new(StatusCode::CONFLICT, "2FA already enabled"));
    }
    let secret: Option<String> = {
        let conn = state.db.conn.lock().unwrap();
        conn.query_row(
            "SELECT totp_secret FROM users WHERE id = ?1",
            [&user.user_id],
            |r| r.get(0),
        )
        .map_err(db_err)?
    };
    let Some(secret) = secret else {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "no enrollment in progress",
        ));
    };

    let plaintext = crypto::open_totp(&secret, &state.totp_key)
        .map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "seed unavailable"))?;
    let totp = make_totp(&plaintext)?;
    if !totp.check_current(&req.code).unwrap_or(false) {
        return Err(ApiError::new(StatusCode::UNAUTHORIZED, "invalid code"));
    }

    // Generate fresh one-time recovery codes; store only their hashes.
    let codes: Vec<String> = (0..RECOVERY_CODE_COUNT)
        .map(|_| crypto::random_recovery_code())
        .collect();

    {
        let conn = state.db.conn.lock().unwrap();
        conn.execute(
            "UPDATE users SET totp_enabled = 1 WHERE id = ?1",
            [&user.user_id],
        )
        .map_err(db_err)?;
        // Replace any previous codes.
        conn.execute(
            "DELETE FROM recovery_codes WHERE user_id = ?1",
            [&user.user_id],
        )
        .map_err(db_err)?;
        for c in &codes {
            conn.execute(
                "INSERT INTO recovery_codes (user_id, code_hash, used) VALUES (?1, ?2, 0)",
                rusqlite::params![user.user_id, crypto::hash_recovery_code(c)],
            )
            .map_err(db_err)?;
        }
    }

    {
        let c = state.db.conn.lock().unwrap();
        c.execute(
            "DELETE FROM sessions WHERE user_id=?1 AND token<>?2",
            rusqlite::params![user.user_id, user.token_hash],
        )
        .map_err(db_err)?;
    }
    Ok(Json(TotpVerifyResp {
        totp_enabled: true,
        recovery_codes: codes,
    }))
}

pub async fn totp_disable(
    State(state): State<AppState>,
    user: axum::Extension<AuthUser>,
    Json(req): Json<TotpCodeReq>,
) -> ApiResult<impl IntoResponse> {
    let (secret, enabled): (Option<String>, bool) = {
        let conn = state.db.conn.lock().unwrap();
        conn.query_row(
            "SELECT totp_secret, totp_enabled FROM users WHERE id = ?1",
            [&user.user_id],
            |r| Ok((r.get::<_, Option<String>>(0)?, r.get::<_, i64>(1)? != 0)),
        )
        .map_err(db_err)?
    };

    if !enabled {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "totp not enabled"));
    }
    let Some(secret) = secret else {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "totp not enabled"));
    };

    if !state
        .ratelimit
        .check(&format!("totp-disable|{}", user.user_id))
    {
        return Err(ApiError::new(StatusCode::TOO_MANY_REQUESTS, "rate limited"));
    }
    let ok_totp = consume_totp(&state, &user.user_id, &secret, &req.code)?;
    let ok = ok_totp || consume_recovery_code(&state, &user.user_id, &req.code)?;
    if !ok {
        return Err(ApiError::new(StatusCode::UNAUTHORIZED, "invalid code"));
    }

    {
        let conn = state.db.conn.lock().unwrap();
        conn.execute(
            "UPDATE users SET totp_enabled = 0, totp_secret = NULL,totp_last_step=-1 WHERE id = ?1",
            [&user.user_id],
        )
        .map_err(db_err)?;
        conn.execute(
            "DELETE FROM recovery_codes WHERE user_id = ?1",
            [&user.user_id],
        )
        .map_err(db_err)?;
    }

    Ok(Json(json!({ "totp_enabled": false })))
}

// ---------------------------------------------------------------------------
// Recovery-key login: prove possession of the recovery key (no password),
// get a session + the recovery-wrapped user key so the client can unwrap the
// user_key and then set a NEW password via /v1/credentials.
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct RecoveryLoginReq {
    pub username: String,
    pub recovery_auth_hash: String,
    pub device_name: Option<String>,
}

#[derive(Serialize)]
pub struct RecoveryLoginResp {
    pub token: String,
    pub user_id: String,
    pub account_salt: String,
    pub kdf_params: String,
    pub recovery_wrapped_user_key: String,
}

pub async fn recovery_login(
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(req): Json<RecoveryLoginReq>,
) -> ApiResult<impl IntoResponse> {
    let username = req.username.trim().to_string();
    if username.is_empty()
        || username.len() > 128
        || req.recovery_auth_hash.len() > 512
        || req.device_name.as_ref().is_some_and(|s| s.len() > 128)
    {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid recovery fields",
        ));
    }
    let rl_key = format!("recovery|{}|{}", username, client_ip(&headers, &addr));
    if !state.ratelimit.check(&rl_key) {
        return Err(ApiError::new(StatusCode::TOO_MANY_REQUESTS, "rate limited"));
    }

    // Load recovery material. Same generic 401 whether the user is absent or has
    // no recovery key configured — no enumeration / no "recovery unavailable" hint.
    let row: Option<(String, String, String, Option<String>, Option<String>)> = {
        let conn = state.db.conn.lock().unwrap();
        conn.query_row(
            "SELECT id, account_salt, kdf_params, recovery_hash, recovery_wrapped_user_key
             FROM users WHERE username = ?1",
            [&username],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .optional()
        .map_err(db_err)?
    };
    let Some((user_id, account_salt, kdf_params, Some(recovery_hash), Some(recovery_wrapped))) =
        row
    else {
        return Err(ApiError::new(
            StatusCode::UNAUTHORIZED,
            "invalid recovery key",
        ));
    };
    if !crypto::verify_auth(&req.recovery_auth_hash, &recovery_hash) {
        return Err(ApiError::new(
            StatusCode::UNAUTHORIZED,
            "invalid recovery key",
        ));
    }

    // Issue a session (mirrors login).
    let device_name = req.device_name.unwrap_or_else(|| "recovery".to_string());
    let token = crypto::random_token(32);
    let ts = now();
    {
        let conn = state.db.conn.lock().unwrap();
        let device_id = crypto::new_uuid();
        conn.execute(
            "INSERT INTO devices (id, user_id, name, created_at, last_seen)
             VALUES (?1, ?2, ?3, ?4, ?4)",
            rusqlite::params![device_id, user_id, device_name, ts],
        )
        .map_err(db_err)?;
        conn.execute(
            "INSERT INTO sessions (token, user_id, device_id, created_at, expires_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![
                crypto::token_hash(&token),
                user_id,
                device_id,
                ts,
                ts + SESSION_TTL_SECS
            ],
        )
        .map_err(db_err)?;
    }

    {
        let conn = state.db.conn.lock().unwrap();
        conn.execute(
            "UPDATE sessions SET recovery=1 WHERE token=?1",
            [crypto::token_hash(&token)],
        )
        .map_err(db_err)?;
    }
    Ok(Json(RecoveryLoginResp {
        token,
        user_id,
        account_salt,
        kdf_params,
        recovery_wrapped_user_key: recovery_wrapped,
    }))
}

// ---------------------------------------------------------------------------
// Update credentials (authenticated): re-key the account. Used by BOTH change-
// password (logged in normally) and recovery-finish (logged in via recovery).
// Rotates the password verifier + wrapped user key; optionally the recovery key.
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct CredentialsReq {
    pub current_auth_hash: Option<String>,
    pub totp: Option<String>,
    pub auth_hash: String,
    pub wrapped_user_key: String,
    pub recovery_auth_hash: Option<String>,
    pub recovery_wrapped_user_key: Option<String>,
}

pub async fn update_credentials(
    State(state): State<AppState>,
    user: axum::Extension<AuthUser>,
    Json(req): Json<CredentialsReq>,
) -> ApiResult<impl IntoResponse> {
    if !user.recovery {
        require_proof(&state, &user, &req.current_auth_hash, &req.totp)?;
    }
    if req.auth_hash.is_empty()
        || req.auth_hash.len() > 512
        || req.wrapped_user_key.is_empty()
        || req.wrapped_user_key.len() > 512
    {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "missing fields"));
    }
    let server_hash = crypto::hash_auth(&req.auth_hash)
        .map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "hash error"))?;
    let recovery_hash = match req.recovery_auth_hash.as_deref().filter(|h| !h.is_empty()) {
        Some(h) => Some(
            crypto::hash_auth(h)
                .map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "hash error"))?,
        ),
        None => None,
    };

    let mut conn = state.db.conn.lock().unwrap();
    let tx = conn.transaction().map_err(db_err)?;
    if recovery_hash.is_some() {
        tx.execute(
            "UPDATE users SET server_hash=?1, wrapped_user_key=?2,
                 recovery_hash=?3, recovery_wrapped_user_key=?4 WHERE id=?5",
            rusqlite::params![
                server_hash,
                req.wrapped_user_key,
                recovery_hash,
                req.recovery_wrapped_user_key,
                user.user_id,
            ],
        )
        .map_err(db_err)?;
    } else {
        tx.execute(
            "UPDATE users SET server_hash=?1, wrapped_user_key=?2 WHERE id=?3",
            rusqlite::params![server_hash, req.wrapped_user_key, user.user_id],
        )
        .map_err(db_err)?;
    }
    tx.execute(
        "DELETE FROM sessions WHERE user_id=?1 AND token<>?2",
        rusqlite::params![user.user_id, user.token_hash],
    )
    .map_err(db_err)?;
    tx.execute(
        "UPDATE sessions SET recovery=0 WHERE token=?1",
        [&user.token_hash],
    )
    .map_err(db_err)?;
    if user.recovery {
        tx.execute(
            "UPDATE users SET totp_enabled=0,totp_secret=NULL,totp_last_step=-1 WHERE id=?1",
            [&user.user_id],
        )
        .map_err(db_err)?;
        tx.execute(
            "DELETE FROM recovery_codes WHERE user_id=?1",
            [&user.user_id],
        )
        .map_err(db_err)?;
    }
    tx.commit().map_err(db_err)?;
    Ok(Json(json!({ "ok": true })))
}

// ---------------------------------------------------------------------------
// Delete account (authenticated): wipe the user and ALL their data from the
// server. Irreversible. Local hosts on the client are untouched (the client
// only clears its own sync bookkeeping).
// ---------------------------------------------------------------------------

pub async fn delete_account(
    State(state): State<AppState>,
    user: axum::Extension<AuthUser>,
    Json(req): Json<SensitiveReq>,
) -> ApiResult<impl IntoResponse> {
    require_proof(&state, &user, &req.auth_hash, &req.totp)?;
    let mut conn = state.db.conn.lock().unwrap();
    let tx = conn.transaction().map_err(db_err)?;
    let uid = &user.user_id;
    for sql in [
        "DELETE FROM items WHERE user_id = ?1",
        "DELETE FROM user_rev WHERE user_id = ?1",
        "DELETE FROM sessions WHERE user_id = ?1",
        "DELETE FROM devices WHERE user_id = ?1",
        "DELETE FROM recovery_codes WHERE user_id = ?1",
        "DELETE FROM users WHERE id = ?1",
    ] {
        tx.execute(sql, rusqlite::params![uid]).map_err(db_err)?;
    }
    tx.commit().map_err(db_err)?;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
pub struct SensitiveReq {
    pub auth_hash: Option<String>,
    pub totp: Option<String>,
}
fn require_proof(
    state: &AppState,
    user: &AuthUser,
    proof: &Option<String>,
    code: &Option<String>,
) -> ApiResult<()> {
    if !state
        .ratelimit
        .check(&format!("sensitive|{}", user.user_id))
    {
        return Err(ApiError::new(StatusCode::TOO_MANY_REQUESTS, "rate limited"));
    }
    let row: (String, Option<String>, i64) = {
        let c = state.db.conn.lock().unwrap();
        c.query_row(
            "SELECT server_hash,totp_secret,totp_enabled FROM users WHERE id=?1",
            [&user.user_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .map_err(db_err)?
    };
    if !proof
        .as_ref()
        .is_some_and(|p| p.len() <= 512 && crypto::verify_auth(p, &row.0))
    {
        return Err(ApiError::new(
            StatusCode::UNAUTHORIZED,
            "fresh password proof required",
        ));
    }
    if row.2 != 0
        && !match (row.1.as_deref(), code.as_deref()) {
            (Some(s), Some(c)) => consume_totp(state, &user.user_id, s, c)?,
            _ => false,
        }
    {
        return Err(ApiError::new(StatusCode::UNAUTHORIZED, "totp required"));
    }
    Ok(())
}
fn consume_totp(state: &AppState, uid: &str, encrypted: &str, code: &str) -> ApiResult<bool> {
    if code.len() != 6 || !code.bytes().all(|b| b.is_ascii_digit()) {
        return Ok(false);
    }
    let secret = crypto::open_totp(encrypted, &state.totp_key)
        .map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "seed unavailable"))?;
    let totp = make_totp(&secret)?;
    let step = now() / 30;
    for s in [step - 1, step, step + 1] {
        if crypto::recovery_hash_eq(&totp.generate((s * 30) as u64), code) {
            let c = state.db.conn.lock().unwrap();
            let changed = c
                .execute(
                    "UPDATE users SET totp_last_step=?1 WHERE id=?2 AND totp_last_step<?1",
                    rusqlite::params![s, uid],
                )
                .map_err(db_err)?;
            return Ok(changed == 1);
        }
    }
    Ok(false)
}
pub async fn logout(
    State(state): State<AppState>,
    user: axum::Extension<AuthUser>,
) -> ApiResult<impl IntoResponse> {
    let c = state.db.conn.lock().unwrap();
    c.execute("DELETE FROM sessions WHERE token=?1", [&user.token_hash])
        .map_err(db_err)?;
    Ok(Json(json!({"ok":true})))
}
