//! NexusBro-only encrypted local KV vault. No Tauri commands and no plaintext files.
//!
//! The envelope approach follows NexuSSH (MIT, copyright 2026 hipocrisle):
//! a password-derived wrapping key protects a random DEK; writes use only that
//! DEK. The on-disk format, AAD and storage are intentionally browser-specific.
//! Dependencies: argon2 0.5.3, chacha20poly1305 0.10/getrandom,
//! rand_core 0.6/getrandom, zeroize 1.8, serde, serde_json, base64.
use argon2::{Algorithm, Argon2, Params, Version};
use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use chacha20poly1305::{
    aead::{Aead, KeyInit, Payload},
    XChaCha20Poly1305, XNonce,
};
use rand_core::{OsRng, RngCore};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use zeroize::Zeroizing;

const FILE_NAME: &str = "browser-vault.v1";
const MAX_VALUE: usize = 1024 * 1024;
const MAX_PAYLOAD: usize = 16 * 1024 * 1024;
const MAX_FILE: u64 = 24 * 1024 * 1024;
const IDLE: Duration = Duration::from_secs(15 * 60);
const LOCKED: &str = "Хранилище заблокировано";
const CORRUPT: &str = "Неверный мастер-пароль или повреждено хранилище";
const IO_ERROR: &str = "Не удалось безопасно сохранить хранилище; предыдущие данные сохранены";

type Result<T> = std::result::Result<T, String>;
type Secrets = BTreeMap<String, Zeroizing<String>>;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Header {
    format: String,
    version: u32,
    memory_kib: u32,
    iterations: u32,
    lanes: u32,
    salt: String,
}
impl Header {
    fn new(salt: &[u8]) -> Self {
        Self {
            format: "nexusbro-vault".into(),
            version: 1,
            memory_kib: 19_456,
            iterations: 2,
            lanes: 1,
            salt: B64.encode(salt),
        }
    }
    fn validate(&self) -> Result<Vec<u8>> {
        // Fixed parameters reject hostile KDF settings before any expensive work.
        if self.format != "nexusbro-vault"
            || self.version != 1
            || self.memory_kib != 19_456
            || self.iterations != 2
            || self.lanes != 1
            || self.salt.len() != 24
        {
            return Err(CORRUPT.into());
        }
        let salt = B64.decode(&self.salt).map_err(|_| CORRUPT.to_string())?;
        if salt.len() != 16 {
            return Err(CORRUPT.into());
        }
        Ok(salt)
    }
    fn aad(&self) -> Result<Vec<u8>> {
        let mut aad = b"NEXUSBRO_LOCAL_VAULT_V1\0".to_vec();
        aad.extend(serde_json::to_vec(self).map_err(|_| CORRUPT.to_string())?);
        Ok(aad)
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    header: Header,
    wrapped_key: String,
    payload_nonce: String,
    payload: String,
}
struct Unlocked {
    secrets: Secrets,
    key: Zeroizing<[u8; 32]>,
    header: Header,
    wrapped_key: String,
    touched: Instant,
}
struct Inner {
    root: Option<PathBuf>,
    unlocked: Option<Unlocked>,
    timer_ready: bool,
    idle_timeout: Option<Duration>,
}

#[derive(Clone, Serialize)]
pub struct VaultStatus {
    pub exists: bool,
    pub unlocked: bool,
}
pub struct VaultState {
    inner: Arc<Mutex<Inner>>,
}
impl Default for VaultState {
    fn default() -> Self {
        Self::with_idle_timeout(Some(IDLE))
    }
}
impl VaultState {
    /// Browser and vault share one unlock session, like NexuSSH. The key stays
    /// only in memory until explicit lock or process exit, never across restarts.
    pub fn for_browser_session() -> Self {
        Self::with_idle_timeout(None)
    }
    fn with_idle_timeout(idle_timeout: Option<Duration>) -> Self {
        let inner = Arc::new(Mutex::new(Inner {
            root: None,
            unlocked: None,
            timer_ready: false,
            idle_timeout,
        }));
        if idle_timeout.is_none() {
            inner.lock().unwrap().timer_ready = true;
            return Self { inner };
        }
        let weak = Arc::downgrade(&inner);
        let ready = std::thread::Builder::new()
            .name("nexusbro-vault-lock".into())
            .spawn(move || loop {
                std::thread::sleep(Duration::from_secs(15));
                let Some(inner) = weak.upgrade() else {
                    break;
                };
                let Ok(mut inner) = inner.lock() else {
                    break;
                };
                expire(&mut inner);
            })
            .is_ok();
        if let Ok(mut guard) = inner.lock() {
            guard.timer_ready = ready;
        }
        Self { inner }
    }
}
fn expire(inner: &mut Inner) {
    if inner.unlocked.as_ref().is_some_and(|v| {
        inner
            .idle_timeout
            .is_some_and(|timeout| v.touched.elapsed() >= timeout)
    }) {
        inner.unlocked = None;
    }
}
fn random<const N: usize>() -> Result<[u8; N]> {
    let mut value = [0; N];
    OsRng
        .try_fill_bytes(&mut value)
        .map_err(|_| "Источник случайности недоступен".to_string())?;
    Ok(value)
}
fn derive(password: &str, header: &Header) -> Result<Zeroizing<[u8; 32]>> {
    let salt = header.validate()?;
    if password.len() > 1024 {
        return Err(CORRUPT.into());
    }
    let params = Params::new(header.memory_kib, header.iterations, header.lanes, Some(32))
        .map_err(|_| CORRUPT.to_string())?;
    let mut key = Zeroizing::new([0; 32]);
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(password.as_bytes(), &salt, &mut *key)
        .map_err(|_| CORRUPT.to_string())?;
    Ok(key)
}
fn encrypt(key: &[u8; 32], nonce: &[u8], bytes: &[u8], aad: &[u8]) -> Result<Vec<u8>> {
    if nonce.len() != 24 {
        return Err(CORRUPT.into());
    }
    XChaCha20Poly1305::new_from_slice(key)
        .map_err(|_| CORRUPT.to_string())?
        .encrypt(XNonce::from_slice(nonce), Payload { msg: bytes, aad })
        .map_err(|_| CORRUPT.to_string())
}
fn decrypt(key: &[u8; 32], nonce: &[u8], bytes: &[u8], aad: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
    if nonce.len() != 24 {
        return Err(CORRUPT.into());
    }
    XChaCha20Poly1305::new_from_slice(key)
        .map_err(|_| CORRUPT.to_string())?
        .decrypt(XNonce::from_slice(nonce), Payload { msg: bytes, aad })
        .map(Zeroizing::new)
        .map_err(|_| CORRUPT.to_string())
}
fn validate_items(items: &Secrets) -> Result<()> {
    if items.len() > 10_000
        || items
            .iter()
            .any(|(k, v)| k.is_empty() || k.len() > 512 || v.len() > MAX_VALUE)
    {
        return Err("Превышен допустимый размер хранилища".into());
    }
    Ok(())
}
fn encode(vault: &Unlocked, items: &Secrets) -> Result<Vec<u8>> {
    validate_items(items)?;
    let borrowed: BTreeMap<&str, &str> = items
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    let plaintext = Zeroizing::new(
        serde_json::to_vec(&borrowed)
            .map_err(|_| "Не удалось подготовить данные хранилища".to_string())?,
    );
    if plaintext.len() > MAX_PAYLOAD {
        return Err("Превышен допустимый размер хранилища".into());
    }
    let nonce = random::<24>()?;
    let mut aad = vault.header.aad()?;
    aad.extend_from_slice(vault.wrapped_key.as_bytes());
    let encrypted = encrypt(&vault.key, &nonce, &plaintext, &aad)?;
    serde_json::to_vec(&Envelope {
        header: vault.header.clone(),
        wrapped_key: vault.wrapped_key.clone(),
        payload_nonce: B64.encode(nonce),
        payload: B64.encode(encrypted),
    })
    .map_err(|_| IO_ERROR.into())
}
fn decode(bytes: &[u8], password: &str) -> Result<Unlocked> {
    let envelope: Envelope = serde_json::from_slice(bytes).map_err(|_| CORRUPT.to_string())?;
    // Validate all lengths and the fixed KDF before derivation/decode allocations.
    envelope.header.validate()?;
    if envelope.wrapped_key.len() != 96
        || envelope.payload_nonce.len() != 32
        || envelope.payload.len() > (MAX_PAYLOAD + 16).div_ceil(3) * 4
    {
        return Err(CORRUPT.into());
    }
    let wrapped = B64
        .decode(&envelope.wrapped_key)
        .map_err(|_| CORRUPT.to_string())?;
    if wrapped.len() != 72 {
        return Err(CORRUPT.into());
    }
    let wrapping_key = derive(password, &envelope.header)?;
    let raw_key = decrypt(
        &wrapping_key,
        &wrapped[..24],
        &wrapped[24..],
        &envelope.header.aad()?,
    )?;
    if raw_key.len() != 32 {
        return Err(CORRUPT.into());
    }
    let mut key = Zeroizing::new([0; 32]);
    key.copy_from_slice(&raw_key);
    let nonce = B64
        .decode(&envelope.payload_nonce)
        .map_err(|_| CORRUPT.to_string())?;
    let ciphertext = B64
        .decode(&envelope.payload)
        .map_err(|_| CORRUPT.to_string())?;
    let mut aad = envelope.header.aad()?;
    aad.extend_from_slice(envelope.wrapped_key.as_bytes());
    let plaintext = decrypt(&key, &nonce, &ciphertext, &aad)?;
    if plaintext.len() > MAX_PAYLOAD {
        return Err(CORRUPT.into());
    }
    let map: BTreeMap<String, String> =
        serde_json::from_slice(&plaintext).map_err(|_| CORRUPT.to_string())?;
    let secrets: Secrets = map
        .into_iter()
        .map(|(k, v)| (k, Zeroizing::new(v)))
        .collect();
    validate_items(&secrets).map_err(|_| CORRUPT.to_string())?;
    Ok(Unlocked {
        secrets,
        key,
        header: envelope.header,
        wrapped_key: envelope.wrapped_key,
        touched: Instant::now(),
    })
}
fn regular(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(m) if m.is_file() && !m.file_type().is_symlink() => Ok(true),
        Ok(_) => Err("Недопустимый файл хранилища".into()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(_) => Err("Файл хранилища недоступен".into()),
    }
}
fn read_file(path: &Path) -> Result<Vec<u8>> {
    if !regular(path)? {
        return Err("Хранилище ещё не создано".into());
    }
    let file = File::open(path).map_err(|_| "Файл хранилища недоступен".to_string())?;
    let mut bytes = Vec::new();
    file.take(MAX_FILE + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| CORRUPT.to_string())?;
    if bytes.len() as u64 > MAX_FILE {
        return Err(CORRUPT.into());
    }
    Ok(bytes)
}
struct Temporary(PathBuf);
impl Drop for Temporary {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}
fn temporary(root: &Path, bytes: &[u8]) -> Result<Temporary> {
    let name = B64
        .encode(random::<18>()?)
        .replace('/', "_")
        .replace('+', "-");
    let path = root.join(format!(".{FILE_NAME}.{name}.tmp"));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&path).map_err(|_| IO_ERROR.to_string())?;
    let temp = Temporary(path);
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(|_| IO_ERROR.to_string())?;
    Ok(temp)
}
fn sync_dir(root: &Path) {
    #[cfg(unix)]
    {
        if let Ok(dir) = File::open(root) {
            let _ = dir.sync_all();
        }
    }
    #[cfg(not(unix))]
    {
        let _ = root;
    }
}
fn persist(root: &Path, bytes: &[u8], create: bool) -> Result<()> {
    fs::create_dir_all(root).map_err(|_| IO_ERROR.to_string())?;
    let target = root.join(FILE_NAME);
    let backup = root.join(format!("{FILE_NAME}.bak"));
    let exists = regular(&target)?;
    if create && exists {
        return Err("Хранилище уже существует".into());
    }
    if !create && !exists {
        return Err("Файл хранилища исчез; запись остановлена".into());
    }
    if !create {
        // Ciphertext-only backup, replaced atomically before touching the original.
        regular(&backup)?;
        let previous = read_file(&target)?;
        let copy = temporary(root, &previous)?;
        fs::rename(&copy.0, &backup).map_err(|_| IO_ERROR.to_string())?;
    }
    let new = temporary(root, bytes)?;
    if create {
        // An atomic no-replace publication: a racing creator can never be overwritten.
        fs::hard_link(&new.0, &target).map_err(|_| {
            "Хранилище уже существует или не удалось создать его безопасно".to_string()
        })?;
    } else {
        fs::rename(&new.0, &target).map_err(|_| IO_ERROR.to_string())?;
    }
    sync_dir(root);
    Ok(())
}
impl VaultState {
    /// Call once at startup so status can report an existing locked vault.
    /// Changing roots always drops all previously unlocked data.
    pub fn configure(&self, root: &Path) -> Result<()> {
        let mut inner = self.inner.lock().map_err(|_| LOCKED.to_string())?;
        if inner.root.as_deref() != Some(root) {
            inner.unlocked = None;
            inner.root = Some(root.to_path_buf());
        }
        Ok(())
    }
    pub fn status(&self) -> VaultStatus {
        let Ok(mut inner) = self.inner.lock() else {
            return VaultStatus {
                exists: true,
                unlocked: false,
            };
        };
        expire(&mut inner);
        VaultStatus {
            exists: inner
                .root
                .as_ref()
                .is_some_and(|p| p.join(FILE_NAME).exists()),
            unlocked: inner.unlocked.is_some(),
        }
    }
    pub fn create(&self, root: &Path, password: &str) -> Result<()> {
        self.configure(root)?;
        if password.chars().count() < 12 || password.len() > 1024 {
            return Err(
                "Мастер-пароль должен содержать от 12 символов (не более 1024 байт)".into(),
            );
        }
        let mut inner = self.inner.lock().map_err(|_| LOCKED.to_string())?;
        if !inner.timer_ready {
            return Err("Контроль автоблокировки недоступен".into());
        }
        if regular(&root.join(FILE_NAME))? {
            return Err("Хранилище уже существует".into());
        }
        let header = Header::new(&random::<16>()?);
        let wrapping_key = derive(password, &header)?;
        let key = Zeroizing::new(random::<32>()?);
        let nonce = random::<24>()?;
        let mut wrapped = nonce.to_vec();
        wrapped.extend(encrypt(&wrapping_key, &nonce, &*key, &header.aad()?)?);
        let vault = Unlocked {
            secrets: BTreeMap::new(),
            key,
            header,
            wrapped_key: B64.encode(wrapped),
            touched: Instant::now(),
        };
        persist(root, &encode(&vault, &vault.secrets)?, true)?;
        inner.unlocked = Some(vault);
        Ok(())
    }
    pub fn unlock(&self, root: &Path, password: &str) -> Result<()> {
        self.configure(root)?;
        let mut inner = self.inner.lock().map_err(|_| LOCKED.to_string())?;
        // A failed unlock never leaves an earlier unlocked key in memory.
        inner.unlocked = None;
        if !inner.timer_ready {
            return Err("Контроль автоблокировки недоступен".into());
        }
        let vault = decode(&read_file(&root.join(FILE_NAME))?, password)?;
        inner.unlocked = Some(vault);
        Ok(())
    }
    pub fn lock(&self) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.unlocked = None;
        }
    }
    pub fn get(&self, key: &str) -> Result<Option<Zeroizing<String>>> {
        let mut inner = self.inner.lock().map_err(|_| LOCKED.to_string())?;
        expire(&mut inner);
        let vault = inner.unlocked.as_mut().ok_or_else(|| LOCKED.to_string())?;
        vault.touched = Instant::now();
        Ok(vault.secrets.get(key).cloned())
    }
    pub fn keys(&self, prefix: &str) -> Result<Vec<String>> {
        let mut inner = self.inner.lock().map_err(|_| LOCKED.to_string())?;
        expire(&mut inner);
        let vault = inner.unlocked.as_mut().ok_or_else(|| LOCKED.to_string())?;
        vault.touched = Instant::now();
        Ok(vault
            .secrets
            .keys()
            .filter(|k| k.starts_with(prefix))
            .cloned()
            .collect())
    }
    pub fn put(&self, key: &str, value: &str) -> Result<()> {
        if key.is_empty() || key.len() > 512 || value.len() > MAX_VALUE {
            return Err("Недопустимый размер записи хранилища".into());
        }
        self.mutate(|items| {
            items.insert(key.into(), Zeroizing::new(value.into()));
        })
    }
    pub fn remove(&self, key: &str) -> Result<()> {
        self.mutate(|items| {
            items.remove(key);
        })
    }
    /// Compare, write and commit under the same vault mutex. A stale caller
    /// cannot replace a credential edited since the sync/capture snapshot.
    pub fn compare_and_set(
        &self,
        key: &str,
        expected: Option<&str>,
        value: Option<&str>,
    ) -> Result<()> {
        if key.is_empty()
            || key.len() > 512
            || expected.is_some_and(|v| v.len() > MAX_VALUE)
            || value.is_some_and(|v| v.len() > MAX_VALUE)
        {
            return Err("Недопустимый размер записи хранилища".into());
        }
        let mut inner = self.inner.lock().map_err(|_| LOCKED.to_string())?;
        expire(&mut inner);
        let root = inner.root.clone().ok_or_else(|| LOCKED.to_string())?;
        let vault = inner.unlocked.as_mut().ok_or_else(|| LOCKED.to_string())?;
        if vault.secrets.get(key).map(|v| v.as_str()) != expected {
            return Err("Запись изменилась; повторите действие".into());
        }
        let mut next = vault.secrets.clone();
        match value {
            Some(value) => {
                next.insert(key.into(), Zeroizing::new(value.into()));
            }
            None => {
                next.remove(key);
            }
        }
        let bytes = encode(vault, &next)?;
        persist(&root, &bytes, false)?;
        vault.secrets = next;
        vault.touched = Instant::now();
        Ok(())
    }
    fn mutate(&self, change: impl FnOnce(&mut Secrets)) -> Result<()> {
        let mut inner = self.inner.lock().map_err(|_| LOCKED.to_string())?;
        expire(&mut inner);
        let root = inner.root.clone().ok_or_else(|| LOCKED.to_string())?;
        let vault = inner.unlocked.as_mut().ok_or_else(|| LOCKED.to_string())?;
        let mut next = vault.secrets.clone();
        change(&mut next);
        // Keep RAM and disk in agreement: commit the in-memory map only after IO succeeds.
        let bytes = encode(vault, &next)?;
        persist(&root, &bytes, false)?;
        vault.secrets = next;
        vault.touched = Instant::now();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const PASSWORD: &str = "local test master password";
    struct Directory(PathBuf);
    impl Directory {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "nexusbro-vault-test-{}-{}",
                std::process::id(),
                B64.encode(random::<12>().unwrap()).replace('/', "_")
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Directory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    #[test]
    fn persistence_wrong_password_and_plaintext_absence() {
        let root = Directory::new();
        let vault = VaultState::default();
        vault.configure(&root.0).unwrap();
        assert!(!vault.status().exists);
        vault.create(&root.0, PASSWORD).unwrap();
        vault
            .put("password.random-id", "secret-unique-payload")
            .unwrap();
        vault
            .put("account.session", "private-session-token")
            .unwrap();
        for entry in fs::read_dir(&root.0).unwrap() {
            let bytes = fs::read(entry.unwrap().path()).unwrap();
            assert!(!bytes
                .windows(b"secret-unique-payload".len())
                .any(|w| w == b"secret-unique-payload"));
            assert!(!bytes
                .windows(b"private-session-token".len())
                .any(|w| w == b"private-session-token"));
        }
        vault.lock();
        assert!(vault.get("password.random-id").is_err());
        let original = fs::read(root.0.join(FILE_NAME)).unwrap();
        assert!(vault.unlock(&root.0, "wrong password long enough").is_err());
        assert_eq!(original, fs::read(root.0.join(FILE_NAME)).unwrap());
        assert!(!vault.status().unlocked);
        vault.unlock(&root.0, PASSWORD).unwrap();
        assert_eq!(
            vault.get("password.random-id").unwrap().unwrap().as_str(),
            "secret-unique-payload"
        );
        assert_eq!(vault.keys("password.").unwrap(), vec!["password.random-id"]);
    }
    #[test]
    fn header_tamper_rejected_and_ciphertexts_are_fresh() {
        let root = Directory::new();
        let vault = VaultState::default();
        vault.create(&root.0, PASSWORD).unwrap();
        vault.put("item", "same secret").unwrap();
        let first = fs::read(root.0.join(FILE_NAME)).unwrap();
        vault.put("item", "same secret").unwrap();
        let second = fs::read(root.0.join(FILE_NAME)).unwrap();
        assert_ne!(first, second);
        let mut bad: serde_json::Value = serde_json::from_slice(&second).unwrap();
        bad["header"]["memory_kib"] = serde_json::json!(u32::MAX);
        let bad = serde_json::to_vec(&bad).unwrap();
        fs::write(root.0.join(FILE_NAME), &bad).unwrap();
        assert!(vault.unlock(&root.0, PASSWORD).is_err());
        assert_eq!(fs::read(root.0.join(FILE_NAME)).unwrap(), bad);
    }
    #[test]
    fn failed_write_keeps_original_and_memory_and_no_recreate() {
        let root = Directory::new();
        let vault = VaultState::default();
        vault.create(&root.0, PASSWORD).unwrap();
        vault.put("item", "old").unwrap();
        let original = fs::read(root.0.join(FILE_NAME)).unwrap();
        assert!(vault.create(&root.0, PASSWORD).is_err());
        assert_eq!(fs::read(root.0.join(FILE_NAME)).unwrap(), original);
        let backup = root.0.join(format!("{FILE_NAME}.bak"));
        fs::remove_file(&backup).unwrap();
        fs::create_dir(&backup).unwrap();
        assert!(vault.put("item", "new").is_err());
        assert_eq!(vault.get("item").unwrap().unwrap().as_str(), "old");
        assert_eq!(fs::read(root.0.join(FILE_NAME)).unwrap(), original);
    }
    #[test]
    fn browser_session_unlock_is_shared_until_explicit_lock_or_restart() {
        let root = Directory::new();
        let first = Arc::new(VaultState::for_browser_session());
        assert!(!first.status().exists);
        first.create(&root.0, PASSWORD).unwrap();
        first.put("password/site", "session secret").unwrap();
        let before = fs::read(root.0.join(FILE_NAME)).unwrap();
        drop(first);

        let restarted = Arc::new(VaultState::for_browser_session());
        restarted.configure(&root.0).unwrap();
        assert!(restarted.status().exists);
        assert!(!restarted.status().unlocked);
        assert!(restarted
            .unlock(&root.0, "wrong password long enough")
            .is_err());
        assert!(!restarted.status().unlocked);
        restarted.unlock(&root.0, PASSWORD).unwrap();
        let settings_window = Arc::clone(&restarted);
        // Advancing last-secret access models a long active browsing session:
        // browser and vault must not acquire independent unlock requirements.
        restarted
            .inner
            .lock()
            .unwrap()
            .unlocked
            .as_mut()
            .unwrap()
            .touched = Instant::now() - Duration::from_secs(24 * 60 * 60);
        assert!(settings_window.status().unlocked);
        assert_eq!(
            settings_window
                .get("password/site")
                .unwrap()
                .unwrap()
                .as_str(),
            "session secret"
        );
        assert_eq!(fs::read(root.0.join(FILE_NAME)).unwrap(), before);
        restarted.lock();
        assert!(!settings_window.status().unlocked);
        assert!(settings_window.get("password/site").is_err());
    }
    #[test]
    fn idle_expiry_is_enforced_and_instances_are_isolated() {
        let first = Directory::new();
        let second = Directory::new();
        let a = VaultState::default();
        let b = VaultState::default();
        a.create(&first.0, PASSWORD).unwrap();
        b.create(&second.0, PASSWORD).unwrap();
        a.put("item", "only first").unwrap();
        assert!(b.get("item").unwrap().is_none());
        a.inner.lock().unwrap().unlocked.as_mut().unwrap().touched =
            Instant::now() - IDLE - Duration::from_secs(1);
        assert!(a.put("item", "forbidden").is_err());
        assert!(!a.status().unlocked);
        assert!(a.keys("").is_err());
        assert!(b.status().unlocked);
    }
    #[test]
    fn short_password_size_limit_and_corruption_fail_closed() {
        let root = Directory::new();
        let vault = VaultState::default();
        assert!(vault.create(&root.0, "short").is_err());
        vault.create(&root.0, PASSWORD).unwrap();
        assert!(vault.put("x", &"a".repeat(MAX_VALUE + 1)).is_err());
        let mut bytes = fs::read(root.0.join(FILE_NAME)).unwrap();
        bytes.truncate(bytes.len() / 2);
        fs::write(root.0.join(FILE_NAME), &bytes).unwrap();
        assert!(vault.unlock(&root.0, PASSWORD).is_err());
        assert!(!vault.status().unlocked);
        assert_eq!(bytes, fs::read(root.0.join(FILE_NAME)).unwrap());
    }
    #[test]
    fn compare_and_set_create_update_delete_and_stale_disk_unchanged() {
        let root = Directory::new();
        let vault = VaultState::default();
        vault.create(&root.0, PASSWORD).unwrap();
        vault.put("unrelated", "preserved").unwrap();
        vault.compare_and_set("item", None, Some("first")).unwrap();
        let disk = || {
            (
                fs::read(root.0.join(FILE_NAME)).unwrap(),
                fs::read(root.0.join(format!("{FILE_NAME}.bak"))).unwrap(),
            )
        };
        let before = disk();
        assert!(vault
            .compare_and_set("item", None, Some("clobber"))
            .is_err());
        assert_eq!(before, disk());
        assert_eq!(vault.get("item").unwrap().unwrap().as_str(), "first");
        vault
            .compare_and_set("item", Some("first"), Some("second"))
            .unwrap();
        let before = disk();
        assert!(vault.compare_and_set("item", Some("first"), None).is_err());
        assert_eq!(before, disk());
        assert_eq!(vault.get("item").unwrap().unwrap().as_str(), "second");
        vault.compare_and_set("item", Some("second"), None).unwrap();
        let before = disk();
        assert!(vault.compare_and_set("item", Some("second"), None).is_err());
        assert_eq!(before, disk());
        vault.lock();
        assert!(vault
            .compare_and_set("item", None, Some("locked write"))
            .is_err());
        assert_eq!(before, disk());
        vault.unlock(&root.0, PASSWORD).unwrap();
        assert!(vault.get("item").unwrap().is_none());
        assert_eq!(
            vault.get("unrelated").unwrap().unwrap().as_str(),
            "preserved"
        );
    }
    #[test]
    fn concurrent_compare_and_set_has_one_winner_and_persists_it() {
        let root = Directory::new();
        let vault = Arc::new(VaultState::default());
        vault.create(&root.0, PASSWORD).unwrap();
        vault.put("item", "snapshot").unwrap();
        let barrier = Arc::new(std::sync::Barrier::new(3));
        let mut handles = Vec::new();
        for value in ["left", "right"] {
            let vault = Arc::clone(&vault);
            let barrier = Arc::clone(&barrier);
            handles.push(std::thread::spawn(move || {
                barrier.wait();
                vault
                    .compare_and_set("item", Some("snapshot"), Some(value))
                    .is_ok()
            }));
        }
        barrier.wait();
        let winners = handles
            .into_iter()
            .map(|h| h.join().unwrap())
            .filter(|won| *won)
            .count();
        assert_eq!(winners, 1);
        let saved = vault.get("item").unwrap().unwrap();
        assert!(matches!(saved.as_str(), "left" | "right"));
        vault.lock();
        vault.unlock(&root.0, PASSWORD).unwrap();
        assert_eq!(vault.get("item").unwrap().unwrap().as_str(), saved.as_str());
    }
    #[cfg(unix)]
    #[test]
    fn unix_permissions_and_symlink_rejection() {
        use std::os::unix::{fs::symlink, fs::PermissionsExt};
        let root = Directory::new();
        let vault = VaultState::default();
        vault.create(&root.0, PASSWORD).unwrap();
        vault.put("item", "secret").unwrap();
        assert_eq!(
            fs::metadata(root.0.join(FILE_NAME))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(root.0.join(format!("{FILE_NAME}.bak")))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        let other = Directory::new();
        symlink(root.0.join(FILE_NAME), other.0.join(FILE_NAME)).unwrap();
        let second = VaultState::default();
        assert!(second.unlock(&other.0, PASSWORD).is_err());
    }
}
