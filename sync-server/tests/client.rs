//! Real browser client + real HTTP server + independent encrypted local vaults.
//! Run in GitHub CI; no CEF dependency and no production requests.
#[path = "../../desktop/src-tauri/src/browser_account.rs"]
mod browser_account;
#[path = "../../desktop/src-tauri/src/browser_crypto.rs"]
mod browser_crypto;
#[path = "../../desktop/src-tauri/src/secure_vault.rs"]
mod secure_vault;
use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use browser_account::{AccountState, SyncItem};
use nexusbro_sync_server::{app, build_state, AppState};
use secure_vault::VaultState;
use serde_json::{json, Value};
use sha2::Digest;
const PASSWORD: &str = "fixture-browser-password-42";
async fn server() -> (String, AppState, tokio::task::JoinHandle<()>) {
    let state = build_state(":memory:").unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let cloned = state.clone();
    let task = tokio::spawn(async move {
        axum::serve(
            listener,
            app(cloned).into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await
        .unwrap();
    });
    (url, state, task)
}
fn vault() -> (tempfile::TempDir, VaultState) {
    let dir = tempfile::tempdir().unwrap();
    let v = VaultState::default();
    v.create(dir.path(), "local-master-password-2026").unwrap();
    (dir, v)
}
async fn configure(a: &AccountState, v: &VaultState, url: &str) {
    let r = a
        .dispatch(v, "configure", json!({"server":url}))
        .await
        .unwrap();
    assert_eq!(r["configured"], true);
    assert_eq!(r["logged_in"], false);
}
async fn login(a: &AccountState, v: &VaultState, password: &str) {
    a.dispatch(
        v,
        "login",
        json!({"username":"browser-owner","password":password}),
    )
    .await
    .unwrap();
}
fn item(value: &str, time: u64) -> SyncItem {
    SyncItem {
        id: "nexusbro.v1/password/opaque-fixture-id".into(),
        value: value.into(),
        modified: time,
        deleted: false,
    }
}
fn token(v: &VaultState) -> String {
    let s = v.get("account.browser.v1").unwrap().unwrap();
    serde_json::from_str::<Value>(&s).unwrap()["session"]["token"]
        .as_str()
        .unwrap()
        .into()
}
fn totp(secret: &str) -> String {
    totp_rs::TOTP::new(
        totp_rs::Algorithm::SHA1,
        6,
        1,
        30,
        totp_rs::Secret::Encoded(secret.into()).to_bytes().unwrap(),
        Some("NexusBro".into()),
        "owner".into(),
    )
    .unwrap()
    .generate_current()
    .unwrap()
}
#[tokio::test]
async fn two_real_clients_encrypted_sync_conflict_and_authenticated_tombstone() {
    let (url, state, task) = server().await;
    let (_da, va) = vault();
    let (_db, vb) = vault();
    let a = AccountState::default();
    let b = AccountState::default();
    configure(&a, &va, &url).await;
    configure(&b, &vb, &url).await;
    let registered = a
        .dispatch(
            &va,
            "register",
            json!({"username":"browser-owner","password":PASSWORD}),
        )
        .await
        .unwrap();
    assert!(registered["recovery_key"].as_str().unwrap().len() > 32);
    assert_eq!(registered["logged_in"], true);
    login(&b, &vb, PASSWORD).await;
    let old = item("fixture-secret-password", 1000);
    assert_eq!(a.exchange(&va, vec![old.clone()]).await.unwrap().pushed, 1);
    let incoming = b.exchange(&vb, vec![]).await.unwrap();
    assert_eq!(incoming.remote.len(), 1);
    assert_eq!(incoming.remote[0].value, old.value);
    assert_eq!(b.exchange(&vb, vec![old.clone()]).await.unwrap().pushed, 0);
    let new = item("fixture-new-password", 1001);
    assert_eq!(a.exchange(&va, vec![new.clone()]).await.unwrap().pushed, 1);
    let incoming = b.exchange(&vb, vec![old]).await.unwrap();
    assert_eq!(incoming.remote[0].value, new.value);
    assert!(incoming.conflicts.is_empty());
    b.exchange(&vb, vec![new]).await.unwrap();
    let edit_a = item("independent-edit-A", 1002);
    let edit_b = item("independent-edit-B", 1003);
    a.exchange(&va, vec![edit_a]).await.unwrap();
    let conflict = b.exchange(&vb, vec![edit_b.clone()]).await.unwrap();
    assert_eq!(conflict.conflicts, vec![edit_b.id.clone()]);
    assert!(conflict.remote.is_empty());
    assert!(vb
        .get(&format!("account.conflict.{}", edit_b.id))
        .unwrap()
        .unwrap()
        .contains("independent-edit-B"));
    let deleted = SyncItem {
        id: edit_b.id.clone(),
        value: String::new(),
        modified: 1004,
        deleted: true,
    };
    a.exchange(&va, vec![deleted]).await.unwrap();
    let (_dc, vc) = vault();
    let c = AccountState::default();
    configure(&c, &vc, &url).await;
    login(&c, &vc, PASSWORD).await;
    let last = c.exchange(&vc, vec![]).await.unwrap();
    assert!(last.remote[0].deleted);
    let ciphertext: Vec<u8> = state
        .db
        .conn
        .lock()
        .unwrap()
        .query_row("SELECT ciphertext FROM items LIMIT 1", [], |r| r.get(0))
        .unwrap();
    assert!(ciphertext.len() > 40);
    assert!(!String::from_utf8_lossy(&ciphertext).contains("fixture-secret"));
    // Server metadata alone cannot synthesize a destructive tombstone.
    state
        .db
        .conn
        .lock()
        .unwrap()
        .execute("UPDATE items SET deleted=0", [])
        .unwrap();
    assert!(c
        .exchange(&vc, vec![])
        .await
        .unwrap_err()
        .contains("метаданные"));
    task.abort();
}
#[tokio::test]
async fn real_totp_enrollment_seed_at_rest_replay_and_logout() {
    let (url, state, task) = server().await;
    let (_da, va) = vault();
    let (_db, vb) = vault();
    let a = AccountState::default();
    let b = AccountState::default();
    configure(&a, &va, &url).await;
    configure(&b, &vb, &url).await;
    a.dispatch(
        &va,
        "register",
        json!({"username":"browser-owner","password":PASSWORD}),
    )
    .await
    .unwrap();
    login(&b, &vb, PASSWORD).await;
    let old_other_token = token(&vb);
    let enrolled = a.dispatch(&va, "totp_enroll", json!({})).await.unwrap();
    let secret = enrolled["secret"].as_str().unwrap();
    assert!(enrolled["otpauth_uri"]
        .as_str()
        .unwrap()
        .contains("NexusBro"));
    let stored: String = state
        .db
        .conn
        .lock()
        .unwrap()
        .query_row("SELECT totp_secret FROM users", [], |r| r.get(0))
        .unwrap();
    assert_ne!(stored, secret);
    let verified = a
        .dispatch(&va, "totp_verify", json!({"code":totp(secret)}))
        .await
        .unwrap();
    assert_eq!(verified["totp_enabled"], true);
    assert_eq!(verified["recovery_codes"].as_array().unwrap().len(), 10);
    assert!(a.dispatch(&va, "totp_enroll", json!({})).await.is_err());
    let response = reqwest::Client::new()
        .get(format!("{url}/v1/items?since=0"))
        .bearer_auth(old_other_token)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 401);
    assert!(b
        .dispatch(
            &vb,
            "login",
            json!({"username":"browser-owner","password":PASSWORD})
        )
        .await
        .unwrap_err()
        .contains("TOTP_REQUIRED"));
    let used_code = totp(secret);
    b.dispatch(
        &vb,
        "login",
        json!({"username":"browser-owner","password":PASSWORD,"totp":used_code}),
    )
    .await
    .unwrap();
    assert!(b
        .dispatch(
            &vb,
            "login",
            json!({"username":"browser-owner","password":PASSWORD,"totp":used_code})
        )
        .await
        .is_err());
    let logged = a.dispatch(&va, "logout", json!({})).await.unwrap();
    assert_eq!(logged["logged_in"], false);
    assert_eq!(logged["server_revoked"], true);
    task.abort();
}
#[tokio::test]
async fn account_recovery_rotates_password_without_losing_encrypted_items() {
    let (url, _state, task) = server().await;
    let (_da, va) = vault();
    let (_db, vb) = vault();
    let a = AccountState::default();
    let b = AccountState::default();
    configure(&a, &va, &url).await;
    configure(&b, &vb, &url).await;
    let reg = a
        .dispatch(
            &va,
            "register",
            json!({"username":"browser-owner","password":PASSWORD}),
        )
        .await
        .unwrap();
    a.exchange(&va, vec![item("survives-recovery", 1000)])
        .await
        .unwrap();
    let result=b.dispatch(&vb,"recover",json!({"username":"browser-owner","recovery_key":reg["recovery_key"],"new_password":"new-browser-password-2026"})).await.unwrap();
    assert_eq!(result["logged_in"], true);
    assert_eq!(
        b.exchange(&vb, vec![]).await.unwrap().remote[0].value,
        "survives-recovery"
    );
    assert!(a
        .dispatch(
            &va,
            "login",
            json!({"username":"browser-owner","password":PASSWORD})
        )
        .await
        .is_err());
    login(&a, &va, "new-browser-password-2026").await;
    a.dispatch(&va,"change_password",json!({"old_password":"new-browser-password-2026","new_password":"rotated-again-password-2026"})).await.unwrap();
    assert!(b
        .exchange(&vb, vec![])
        .await
        .unwrap_err()
        .contains("ACCOUNT_SESSION_EXPIRED"));
    assert_eq!(
        b.dispatch(&vb, "status", json!({})).await.unwrap()["logged_in"],
        false
    );
    a.dispatch(
        &va,
        "delete_account",
        json!({"password":"rotated-again-password-2026"}),
    )
    .await
    .unwrap();
    assert_eq!(
        a.dispatch(&va, "status", json!({})).await.unwrap()["logged_in"],
        false
    );
    task.abort();
}
#[tokio::test]
async fn ssh_service_namespace_tls_and_vault_lock_fail_closed() {
    let (url, _state, task) = server().await;
    let (_d, v) = vault();
    let a = AccountState::default();
    assert!(a
        .dispatch(&v, "configure", json!({"server":"http://example.invalid"}))
        .await
        .is_err());
    configure(&a, &v, &url).await;
    a.dispatch(
        &v,
        "register",
        json!({"username":"browser-owner","password":PASSWORD}),
    )
    .await
    .unwrap();
    let t = token(&v);
    let r=reqwest::Client::new().post(format!("{url}/v1/items")).bearer_auth(t).json(&json!({"changes":[{"item_id":"host.ssh","type":"host","ciphertext":B64.encode([7u8;64]),"updated_at":1}]})).send().await.unwrap();
    assert_eq!(r.status().as_u16(), 400);
    v.lock();
    assert!(a.dispatch(&v, "status", json!({})).await.is_err());
    assert!(a.exchange(&v, vec![]).await.is_err());
    task.abort();
}
#[test]
fn browser_kdf_domain_is_distinct_from_ssh() {
    let key = [7u8; 32];
    let salt = [9u8; 16];
    let browser = browser_crypto::auth_hash(&key, &salt).unwrap();
    let hk = hkdf::Hkdf::<sha2::Sha256>::new(Some(&salt), &key);
    let mut ssh = [0u8; 32];
    hk.expand(b"nexussh-auth-v1", &mut ssh).unwrap();
    assert_ne!(browser, ssh);
    assert_eq!(sha2::Sha256::digest(b"fixture").len(), 32);
}
