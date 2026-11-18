//! Explicitly authorized live HTTPS acceptance using the production browser client.
//! Secrets are retained only in encrypted receipts; stdout contains static stages.
#[path = "../../desktop/src-tauri/src/browser_account.rs"]
mod browser_account;
#[path = "../../desktop/src-tauri/src/browser_crypto.rs"]
mod browser_crypto;
#[path = "../../desktop/src-tauri/src/secure_vault.rs"]
mod secure_vault;
use browser_account::{AccountState, SyncItem};
use secure_vault::VaultState;
use serde_json::{json, Value};
use std::{cell::Cell, os::unix::fs::MetadataExt, path::PathBuf, time::Duration};
use zeroize::Zeroizing;
const URL: &str = "https://nexusbro-sync.hipogas.org";
type Result<T> = std::result::Result<T, ()>;
fn check(value: bool) -> Result<()> {
    if value {
        Ok(())
    } else {
        Err(())
    }
}
fn stage(current: &Cell<&'static str>, name: &'static str) {
    current.set(name);
    println!("{{\"stage\":\"{name}\",\"started\":true}}");
}
fn field(value: &Value, name: &str) -> Result<Zeroizing<String>> {
    value[name]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(|s| Zeroizing::new(s.into()))
        .ok_or(())
}
fn random(prefix: &str) -> Zeroizing<String> {
    Zeroizing::new(format!("{prefix}-{}", uuid::Uuid::new_v4().simple()))
}
fn item(value: &str, modified: u64) -> SyncItem {
    SyncItem {
        id: "nexusbro.v1/password/live-acceptance".into(),
        value: value.into(),
        modified,
        deleted: false,
    }
}
struct Suite {
    a: AccountState,
    b: AccountState,
    va: VaultState,
    vb: VaultState,
    username: Zeroizing<String>,
    password: Zeroizing<String>,
    recovery: Option<Zeroizing<String>>,
    registered: bool,
    deleted: bool,
}
impl Suite {
    fn receipt(&self, pending: Option<&str>) -> Result<()> {
        let raw=Zeroizing::new(serde_json::to_string(&json!({"username":self.username.as_str(),"password":self.password.as_str(),"recovery_key":self.recovery.as_ref().map(|s|s.as_str()),"pending_password":pending,"server":URL})).map_err(|_|())?);
        self.va.put("live.acceptance.receipt", &raw).map_err(|_| ())
    }
    async fn dispatch(&self, device: bool, action: &str, data: Value) -> Result<Value> {
        let (a, v) = if device {
            (&self.b, &self.vb)
        } else {
            (&self.a, &self.va)
        };
        a.dispatch(v, action, data).await.map_err(|_| ())
    }
    async fn login(&self, device: bool, totp: Option<&str>) -> Result<Value> {
        self.dispatch(device,"login",json!({"username":self.username.as_str(),"password":self.password.as_str(),"totp":totp})).await
    }
    async fn run(&mut self, current: &Cell<&'static str>) -> Result<()> {
        stage(current, "https-marker");
        for device in [false, true] {
            check(
                self.dispatch(device, "configure", json!({"server":URL}))
                    .await?["configured"]
                    == true,
            )?;
        }
        stage(current, "register-login");
        self.receipt(None)?;
        // Mark intent before sending: a lost registration response still requires cleanup.
        self.registered = true;
        let registered = self
            .dispatch(
                false,
                "register",
                json!({"username":self.username.as_str(),"password":self.password.as_str()}),
            )
            .await?;
        self.recovery = Some(field(&registered, "recovery_key")?);
        self.receipt(None)?;
        check(registered["logged_in"] == true)?;
        check(self.login(true, None).await?["logged_in"] == true)?;
        stage(current, "encrypted-two-client-roundtrip");
        let original = item(&random("synthetic-value"), 1000);
        check(
            self.a
                .exchange(&self.va, vec![original.clone()])
                .await
                .map_err(|_| ())?
                .pushed
                == 1,
        )?;
        let incoming = self.b.exchange(&self.vb, vec![]).await.map_err(|_| ())?;
        check(incoming.remote.len() == 1 && incoming.remote[0].value == original.value)?;
        self.b
            .exchange(&self.vb, vec![original])
            .await
            .map_err(|_| ())?;
        stage(current, "conflict-preserve-and-resolve");
        let edit_a = item(&random("synthetic-A"), 1001);
        let edit_b = item(&random("synthetic-B"), 1002);
        self.a
            .exchange(&self.va, vec![edit_a.clone()])
            .await
            .map_err(|_| ())?;
        let conflict = self
            .b
            .exchange(&self.vb, vec![edit_b.clone()])
            .await
            .map_err(|_| ())?;
        check(conflict.conflicts == vec![edit_b.id.clone()] && conflict.remote.is_empty())?;
        let chosen = self
            .b
            .prepare_resolution(&self.vb, &edit_b.id, "local")
            .await
            .map_err(|_| ())?;
        check(chosen.value == edit_b.value)?;
        self.b
            .commit_resolution(&self.vb, &edit_b.id, "local")
            .await
            .map_err(|_| ())?;
        self.b
            .exchange(&self.vb, vec![chosen])
            .await
            .map_err(|_| ())?;
        let merged = self
            .a
            .exchange(&self.va, vec![edit_a])
            .await
            .map_err(|_| ())?;
        check(merged.remote.len() == 1 && merged.remote[0].value == edit_b.value)?;
        stage(current, "totp-enroll-verify-replay");
        let enrolled = self.dispatch(false, "totp_enroll", json!({})).await?;
        let seed = field(&enrolled, "secret")?;
        let totp = totp_rs::TOTP::new(
            totp_rs::Algorithm::SHA1,
            6,
            1,
            30,
            totp_rs::Secret::Encoded(seed.to_string())
                .to_bytes()
                .map_err(|_| ())?,
            Some("NexusBro".into()),
            "synthetic-acceptance".into(),
        )
        .map_err(|_| ())?;
        let code = Zeroizing::new(totp.generate_current().map_err(|_| ())?);
        let verified = self
            .dispatch(false, "totp_verify", json!({"code":code.as_str()}))
            .await?;
        check(
            verified["totp_enabled"] == true
                && verified["recovery_codes"]
                    .as_array()
                    .is_some_and(|v| v.len() == 10),
        )?;
        tokio::time::sleep(Duration::from_secs(6)).await;
        // Verification does not consume the login replay counter; first login succeeds.
        let login_code = Zeroizing::new(totp.generate_current().map_err(|_| ())?);
        check(self.login(true, Some(&login_code)).await?["logged_in"] == true)?;
        check(self.login(true, Some(&login_code)).await.is_err())?;
        stage(current, "recovery-rotates-password-preserves-items");
        let recovered_password = random("recovered-password");
        self.receipt(Some(&recovered_password))?;
        tokio::time::sleep(Duration::from_secs(6)).await;
        let recovery = self.recovery.as_ref().ok_or(())?;
        let recovered=self.dispatch(false,"recover",json!({"username":self.username.as_str(),"recovery_key":recovery.as_str(),"new_password":recovered_password.as_str()})).await?;
        self.password = recovered_password;
        self.receipt(None)?;
        check(recovered["logged_in"] == true && recovered["totp_enabled"] == false)?;
        let retained = self.a.exchange(&self.va, vec![]).await.map_err(|_| ())?;
        check(retained.remote.iter().any(|v| v.value == edit_b.value))?;
        stage(current, "change-password-and-relogin");
        let rotated = random("rotated-password");
        self.receipt(Some(&rotated))?;
        self.dispatch(
            false,
            "change_password",
            json!({"old_password":self.password.as_str(),"new_password":rotated.as_str()}),
        )
        .await?;
        self.password = rotated;
        self.receipt(None)?;
        tokio::time::sleep(Duration::from_secs(6)).await;
        check(self.login(true, None).await?["logged_in"] == true)?;
        let retained = self.b.exchange(&self.vb, vec![]).await.map_err(|_| ())?;
        check(retained.remote.iter().any(|v| v.value == edit_b.value))?;
        stage(current, "delete-synthetic-account");
        let deleted = self
            .dispatch(
                false,
                "delete_account",
                json!({"password":self.password.as_str()}),
            )
            .await?;
        check(deleted["logged_in"] == false)?;
        self.deleted = true;
        // A separate real client session must be revoked with the deleted account.
        let revoked = self.b.exchange(&self.vb, vec![]).await;
        check(matches!(revoked, Err(ref message) if message.contains("ACCOUNT_SESSION_EXPIRED")))?;
        Ok(())
    }
    async fn cleanup(&mut self) -> Result<()> {
        if !self.registered || self.deleted {
            return Ok(());
        }
        // Recovery handles uncertain password rotation and any enabled TOTP, using
        // the same production client; no API/DB bypass and no unchecked delete.
        if let Some(recovery) = self.recovery.as_ref() {
            let password = random("cleanup-password");
            self.receipt(Some(&password))?;
            self.dispatch(false,"recover",json!({"username":self.username.as_str(),"recovery_key":recovery.as_str(),"new_password":password.as_str()})).await?;
            self.password = password;
            self.receipt(None)?;
        } else {
            self.login(false, None).await?;
        }
        self.dispatch(
            false,
            "delete_account",
            json!({"password":self.password.as_str()}),
        )
        .await?;
        self.deleted = true;
        Ok(())
    }
}
fn args() -> Result<(PathBuf, PathBuf)> {
    let mut args = std::env::args().skip(1);
    let (mut url, mut allowed, mut master, mut receipt) = (None, false, None, None);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--url" => url = args.next(),
            "--allow-live-account-mutations" => allowed = true,
            "--receipt-master-file" => master = args.next().map(PathBuf::from),
            "--receipt-dir" => receipt = args.next().map(PathBuf::from),
            _ => return Err(()),
        }
    }
    check(allowed && url.as_deref() == Some(URL))?;
    Ok((master.ok_or(())?, receipt.ok_or(())?))
}
async fn main_result() -> Result<()> {
    let (master_path, receipt_root) = args()?;
    let master_meta = std::fs::symlink_metadata(&master_path).map_err(|_| ())?;
    let receipt_meta = std::fs::symlink_metadata(&receipt_root).map_err(|_| ())?;
    check(
        master_path.starts_with("/run/")
            && master_meta.is_file()
            && master_meta.mode() & 0o777 == 0o600
            && master_meta.uid() == 0
            && master_meta.len() <= 4096,
    )?;
    check(
        receipt_root.starts_with("/var/lib/")
            && receipt_meta.is_dir()
            && receipt_meta.mode() & 0o777 == 0o700
            && receipt_meta.uid() == 0,
    )?;
    let master = Zeroizing::new(std::fs::read_to_string(master_path).map_err(|_| ())?);
    check(master.trim().len() >= 24)?;
    let receipts = tempfile::Builder::new()
        .prefix("receipt-")
        .tempdir_in(&receipt_root)
        .map_err(|_| ())?;
    let va = VaultState::default();
    let vb = VaultState::default();
    va.create(&receipts.path().join("device-a"), master.trim())
        .map_err(|_| ())?;
    vb.create(&receipts.path().join("device-b"), master.trim())
        .map_err(|_| ())?;
    let mut suite = Suite {
        a: AccountState::default(),
        b: AccountState::default(),
        va,
        vb,
        username: random("live-acceptance"),
        password: random("account-password"),
        recovery: None,
        registered: false,
        deleted: false,
    };
    let current = Cell::new("startup");
    let result = tokio::time::timeout(Duration::from_secs(180), suite.run(&current)).await;
    let passed = matches!(result, Ok(Ok(())));
    if !passed {
        println!("{{\"stage\":\"{}\",\"passed\":false}}", current.get());
    }
    let clean = matches!(
        tokio::time::timeout(Duration::from_secs(60), suite.cleanup()).await,
        Ok(Ok(()))
    );
    suite.va.lock();
    suite.vb.lock();
    if !clean {
        let _retained = receipts.keep();
        println!("{{\"manual_cleanup_needed\":true,\"encrypted_receipt_retained\":true}}");
        return Err(());
    }
    println!("{{\"synthetic_account_cleanup\":true}}");
    check(passed)?;
    println!("{{\"live_https_acceptance\":true}}");
    Ok(())
}
#[tokio::main]
async fn main() {
    if main_result().await.is_err() {
        eprintln!("live acceptance failed; no credentials printed");
        std::process::exit(1);
    }
}
