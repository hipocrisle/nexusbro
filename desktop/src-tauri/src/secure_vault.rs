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
    sync::{atomic::{AtomicU64, Ordering}, Arc, Mutex},
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
/// Metadata only: recovery APIs never expose filesystem paths or plaintext.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct VaultBackup {
    pub id: String,
    /// Unix seconds, matching NexuSSH's backup-date contract.
    pub created: u64,
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
fn validate_new_password(password: &str) -> Result<()> {
    if password.chars().count() < 12 || password.len() > 1024 {
        return Err("Мастер-пароль должен содержать от 12 символов (не более 1024 байт)".into());
    }
    Ok(())
}
fn fresh_vault(password: &str) -> Result<Unlocked> {
    validate_new_password(password)?;
    let header = Header::new(&random::<16>()?);
    let wrapping_key = derive(password, &header)?;
    let key = Zeroizing::new(random::<32>()?);
    let nonce = random::<24>()?;
    let mut wrapped = nonce.to_vec();
    wrapped.extend(encrypt(&wrapping_key, &nonce, &*key, &header.aad()?)?);
    Ok(Unlocked {
        secrets: BTreeMap::new(), key, header,
        wrapped_key: B64.encode(wrapped), touched: Instant::now(),
    })
}

const RECOVERY_PREFIX: &str = "browser-vault.v1.recovery-";
static RECOVERY_STAMP: AtomicU64 = AtomicU64::new(0);
fn recovery_stamp() -> Result<u64> {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| "Неверное системное время")?.as_nanos();
    let now = u64::try_from(now).map_err(|_| "Недопустимое системное время")?;
    // Even a coarse or backwards-moving clock cannot randomly put .bak ahead
    // of the subsequently archived active file in the same application run.
    let previous = RECOVERY_STAMP.fetch_update(Ordering::SeqCst, Ordering::SeqCst,
        |old| old.checked_add(1).map(|next| now.max(next)))
        .map_err(|_| "Исчерпаны идентификаторы резервных копий")?;
    Ok(now.max(previous + 1))
}
fn recovery_metadata(id: &str) -> Result<VaultBackup> {
    let bytes = id.as_bytes();
    if bytes.len() != 53 || bytes[20] != b'-'
        || !bytes[..20].iter().all(u8::is_ascii_digit)
        || !bytes[21..].iter().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(b))
    {
        return Err("Недопустимый идентификатор резервной копии".into());
    }
    let nanos = id[..20].parse::<u64>().map_err(|_| "Недопустимая дата резервной копии")?;
    Ok(VaultBackup { id: id.into(), created: nanos / 1_000_000_000 })
}
fn recovery_path(root: &Path, id: &str) -> Result<PathBuf> {
    recovery_metadata(id)?;
    Ok(root.join(format!("{RECOVERY_PREFIX}{id}")))
}
fn recovery_root(root: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(root).map_err(|_| "Каталог хранилища недоступен")?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err("Недопустимый каталог хранилища".into());
    }
    Ok(())
}
fn immutable_archive(root: &Path, bytes: &[u8]) -> Result<VaultBackup> {
    // Stage ciphertext and fsync it before publishing a no-replace hard link.
    // The archive is independent from the active inode and cannot be overwritten.
    let temp = temporary(root, bytes)?;
    for _ in 0..8 {
        let nanos = recovery_stamp()?;
        let suffix: String = random::<16>()?.iter().map(|b| format!("{b:02x}")).collect();
        let id = format!("{nanos:020}-{suffix}");
        let path = recovery_path(root, &id)?;
        match fs::hard_link(&temp.0, &path) {
            Ok(()) => { sync_dir(root); return recovery_metadata(&id); }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(_) => return Err("Не удалось сохранить зашифрованную резервную копию; сброс отменён".into()),
        }
    }
    Err("Не удалось создать уникальную резервную копию".into())
}
fn archive_and_replace(root: &Path, original: &[u8], replacement: &[u8]) -> Result<VaultBackup> {
    archive_and_replace_with(root, original, replacement, |from, to| fs::rename(from, to))
}
fn archive_and_replace_with(
    root: &Path, original: &[u8], replacement: &[u8],
    publish: impl FnOnce(&Path, &Path) -> std::io::Result<()>,
) -> Result<VaultBackup> {
    recovery_root(root)?;
    let target = root.join(FILE_NAME);
    if read_file(&target)?.as_slice() != original {
        return Err("Хранилище изменилось; повторите действие".into());
    }
    let backup = root.join(format!("{FILE_NAME}.bak"));
    let prior_backup = if regular(&backup)? { Some(read_file(&backup)?) } else { None };
    let staged = temporary(root, replacement)?;
    // Preserve the ordinary .bak too: a later normal write may replace it.
    if let Some(bytes) = prior_backup { immutable_archive(root, &bytes)?; }
    let archived = immutable_archive(root, original)?;
    // A final disk-CAS refuses an unrelated writer's replacement. Within this
    // application the inner mutex already serializes every vault operation.
    if read_file(&target)?.as_slice() != original {
        return Err("Хранилище изменилось; замена отменена".into());
    }
    // Never remove the active file. Rename publishes old OR new ciphertext,
    // including on Windows. A failed publish leaves the old startup lock intact.
    publish(&staged.0, &target).map_err(|_| IO_ERROR.to_string())?;
    sync_dir(root);
    Ok(archived)
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
        validate_new_password(password)?;
        let mut inner = self.inner.lock().map_err(|_| LOCKED.to_string())?;
        if !inner.timer_ready {
            return Err("Контроль автоблокировки недоступен".into());
        }
        if regular(&root.join(FILE_NAME))? {
            return Err("Хранилище уже существует".into());
        }
        let vault = fresh_vault(password)?;
        persist(root, &encode(&vault, &vault.secrets)?, true)?;
        inner.unlocked = Some(vault);
        Ok(())
    }
    /// List immutable, program-created recovery archives, newest first.
    /// Unknown entries and symbolic links are never accepted as backups.
    pub fn recovery_backups(&self) -> Result<Vec<VaultBackup>> {
        let mut inner = self.inner.lock().map_err(|_| LOCKED.to_string())?;
        expire(&mut inner);
        let result: Result<Vec<VaultBackup>> = (|| {
            let root = inner.root.as_ref().ok_or("Хранилище не настроено")?;
            recovery_root(root)?;
            let entries = fs::read_dir(root).map_err(|_| "Не удалось прочитать резервные копии")?;
            let mut backups = Vec::new();
            for entry in entries {
                let entry = entry.map_err(|_| "Не удалось прочитать резервные копии")?;
                let name = entry.file_name();
                let Some(id) = name.to_str().and_then(|n| n.strip_prefix(RECOVERY_PREFIX)) else { continue; };
                let Ok(metadata) = recovery_metadata(id) else { continue; };
                let kind = entry.file_type().map_err(|_| "Не удалось проверить резервную копию")?;
                if kind.is_file() && !kind.is_symlink() { backups.push(metadata); }
            }
            backups.sort_by(|a, b| b.created.cmp(&a.created).then_with(|| b.id.cmp(&a.id)));
            Ok(backups)
        })();
        if result.is_err() { inner.unlocked = None; }
        result
    }
    /// Forgotten-password escape hatch: keep all old ciphertext and atomically
    /// publish a fresh empty vault. This does not contact or modify the cloud.
    pub fn reset_with_password(&self, password: &str) -> Result<VaultBackup> {
        let mut inner = self.inner.lock().map_err(|_| LOCKED.to_string())?;
        inner.unlocked = None;
        let root = inner.root.clone().ok_or("Хранилище не настроено")?;
        if !inner.timer_ready { return Err("Контроль автоблокировки недоступен".into()); }
        validate_new_password(password)?;
        recovery_root(&root)?;
        let original = read_file(&root.join(FILE_NAME))?;
        let mut vault = fresh_vault(password)?;
        let bytes = encode(&vault, &vault.secrets)?;
        let backup = archive_and_replace(&root, &original, &bytes)?;
        vault.touched = Instant::now();
        inner.unlocked = Some(vault);
        Ok(backup)
    }
    /// Authenticate the selected archive before touching any current file.
    /// On failure all keys are dropped and current ciphertext remains intact.
    pub fn restore_recovery_backup(&self, id: &str, password: &str) -> Result<VaultBackup> {
        let mut inner = self.inner.lock().map_err(|_| LOCKED.to_string())?;
        inner.unlocked = None;
        let root = inner.root.clone().ok_or("Хранилище не настроено")?;
        if !inner.timer_ready { return Err("Контроль автоблокировки недоступен".into()); }
        recovery_root(&root)?;
        let selected = read_file(&recovery_path(&root, id)?)?;
        let mut vault = decode(&selected, password)?;
        let original = read_file(&root.join(FILE_NAME))?;
        let backup = archive_and_replace(&root, &original, &selected)?;
        vault.touched = Instant::now();
        inner.unlocked = Some(vault);
        Ok(backup)
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
    /// A batch imports credentials with one encrypted durable commit. No partial RAM or disk mutation.
    pub(crate) fn compare_and_set_many(&self, entries: &[(&str, Option<&str>, Option<&str>)]) -> Result<()> {
        if entries.len()>10_000 {return Err("Слишком много записей".into());}
        let mut seen=std::collections::HashSet::new();
        for (key,expected,value) in entries {
            if key.is_empty()||key.len()>512||!seen.insert(*key)||expected.is_some_and(|v|v.len()>MAX_VALUE)||value.is_some_and(|v|v.len()>MAX_VALUE){return Err("Недопустимые записи хранилища".into());}
        }
        let mut inner=self.inner.lock().map_err(|_|LOCKED.to_string())?;
        expire(&mut inner);
        let root=inner.root.clone().ok_or_else(||LOCKED.to_string())?;
        let vault=inner.unlocked.as_mut().ok_or_else(||LOCKED.to_string())?;
        for (key,expected,_) in entries {if vault.secrets.get(*key).map(|v|v.as_str())!=*expected{return Err("Записи изменились; повторите предпросмотр".into());}}
        let mut next=vault.secrets.clone();
        for(key,_,value)in entries{match value{Some(value)=>{next.insert((*key).into(),Zeroizing::new((*value).into()));},None=>{next.remove(*key);}}}
        let bytes=encode(vault,&next)?;
        persist(&root,&bytes,false)?;
        vault.secrets=next;vault.touched=Instant::now();Ok(())
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
    fn recovery_disk(root: &Path) -> BTreeMap<String, Vec<u8>> {
        fs::read_dir(root).unwrap().map(|e| e.unwrap())
            .filter(|e| e.file_type().unwrap().is_file())
            .map(|e| (e.file_name().to_str().unwrap().to_owned(), fs::read(e.path()).unwrap()))
            .collect()
    }
    fn archive_bytes(root: &Path, backup: &VaultBackup) -> Vec<u8> {
        fs::read(recovery_path(root, &backup.id).unwrap()).unwrap()
    }
    #[test]
    fn reset_empty_then_restore_old_preserves_both_passwords_and_restart_lock() {
        const NEW: &str = "a separate new master password";
        let root = Directory::new(); let vault = VaultState::for_browser_session();
        vault.create(&root.0, PASSWORD).unwrap();
        vault.put("password/old", "old credential secret").unwrap();
        vault.put("account.browser.v1", "old account token").unwrap();
        vault.put("history/protected", "old encrypted history").unwrap();
        let old = fs::read(root.0.join(FILE_NAME)).unwrap();
        let old_bak = fs::read(root.0.join(format!("{FILE_NAME}.bak"))).unwrap();
        vault.lock(); let old_copy = vault.reset_with_password(NEW).unwrap();
        assert!(vault.status().unlocked); assert!(vault.keys("").unwrap().is_empty());
        assert_eq!(archive_bytes(&root.0, &old_copy), old);
        assert_eq!(fs::read(root.0.join(format!("{FILE_NAME}.bak"))).unwrap(), old_bak);
        assert!(decode(&archive_bytes(&root.0, &old_copy), PASSWORD).is_ok());
        assert!(decode(&archive_bytes(&root.0, &old_copy), NEW).is_err());
        let empty_new = fs::read(root.0.join(FILE_NAME)).unwrap();
        assert!(decode(&empty_new, NEW).unwrap().secrets.is_empty());
        assert!(decode(&empty_new, PASSWORD).is_err());
        assert_eq!(vault.recovery_backups().unwrap()[0], old_copy);
        assert!(vault.recovery_backups().unwrap().iter().any(|b| archive_bytes(&root.0, b) == old_bak));
        vault.put("password/new", "new credential secret").unwrap();
        let current_new = fs::read(root.0.join(FILE_NAME)).unwrap();
        let bak_before_restore = fs::read(root.0.join(format!("{FILE_NAME}.bak"))).unwrap();
        let before = recovery_disk(&root.0);
        assert!(vault.restore_recovery_backup(&old_copy.id, NEW).is_err());
        assert!(!vault.status().unlocked); assert_eq!(before, recovery_disk(&root.0));
        let new_copy = vault.restore_recovery_backup(&old_copy.id, PASSWORD).unwrap();
        assert_eq!(archive_bytes(&root.0, &new_copy), current_new);
        assert!(decode(&archive_bytes(&root.0, &new_copy), NEW).is_ok());
        assert!(decode(&archive_bytes(&root.0, &new_copy), PASSWORD).is_err());
        assert_eq!(fs::read(root.0.join(FILE_NAME)).unwrap(), old);
        assert_eq!(fs::read(root.0.join(format!("{FILE_NAME}.bak"))).unwrap(), bak_before_restore);
        assert_eq!(vault.get("password/old").unwrap().unwrap().as_str(), "old credential secret");
        assert_eq!(vault.get("account.browser.v1").unwrap().unwrap().as_str(), "old account token");
        assert_eq!(vault.get("history/protected").unwrap().unwrap().as_str(), "old encrypted history");
        assert!(vault.get("password/new").unwrap().is_none());
        for bytes in recovery_disk(&root.0).values() {
            for secret in ["old credential secret", "new credential secret", "old account token", "old encrypted history"] {
                assert!(!bytes.windows(secret.len()).any(|w| w == secret.as_bytes()));
            }
        }
        let restarted = VaultState::for_browser_session(); restarted.configure(&root.0).unwrap();
        assert!(restarted.status().exists); assert!(!restarted.status().unlocked);
        assert!(restarted.get("password/old").is_err());
        assert!(restarted.unlock(&root.0, NEW).is_err());
        restarted.unlock(&root.0, PASSWORD).unwrap();
        assert_eq!(restarted.get("password/old").unwrap().unwrap().as_str(), "old credential secret");
    }
    #[test]
    fn recovery_invalid_password_corrupt_or_traversal_backup_preserves_current_locked() {
        let root = Directory::new(); let vault = VaultState::for_browser_session();
        vault.create(&root.0, PASSWORD).unwrap(); vault.put("item", "keep me").unwrap();
        let before = recovery_disk(&root.0);
        assert!(vault.reset_with_password("too short").is_err());
        assert!(!vault.status().unlocked); assert_eq!(before, recovery_disk(&root.0));
        let bad_id = format!("{:020}-{}", recovery_stamp().unwrap(), "e".repeat(32));
        fs::write(recovery_path(&root.0, &bad_id).unwrap(), b"corrupt ciphertext").unwrap();
        let before = recovery_disk(&root.0);
        assert!(vault.restore_recovery_backup(&bad_id, PASSWORD).is_err());
        assert!(!vault.status().unlocked); assert_eq!(before, recovery_disk(&root.0));
        let missing_id = format!("{:020}-{}", recovery_stamp().unwrap(), "d".repeat(32));
        let ids = ["../browser-vault.v1".to_owned(), "..\\browser-vault.v1".to_owned(),
            "/tmp/arbitrary".to_owned(), String::new(), "x".repeat(100_000), format!("{bad_id}/.."),
            format!("{bad_id}\\other"), format!("{:020}-{}", 1, "A".repeat(32)), missing_id];
        for id in ids {
            vault.unlock(&root.0, PASSWORD).unwrap();
            assert!(vault.restore_recovery_backup(&id, PASSWORD).is_err());
            assert!(!vault.status().unlocked); assert_eq!(before, recovery_disk(&root.0));
        }
    }
    #[test]
    fn recovery_missing_active_does_not_create_a_new_unlocked_vault() {
        let root = Directory::new(); let vault = VaultState::for_browser_session();
        vault.configure(&root.0).unwrap();
        assert!(vault.recovery_backups().unwrap().is_empty());
        assert!(vault.reset_with_password(PASSWORD).is_err());
        assert!(!vault.status().exists); assert!(!vault.status().unlocked);
        assert!(recovery_disk(&root.0).is_empty());
    }
    #[test]
    fn recovery_archive_precondition_failure_never_replaces_active_or_keeps_keys() {
        let root = Directory::new(); let vault = VaultState::for_browser_session();
        vault.create(&root.0, PASSWORD).unwrap(); vault.put("item", "unchanged").unwrap();
        let original = fs::read(root.0.join(FILE_NAME)).unwrap();
        let selected = immutable_archive(&root.0, &original).unwrap();
        let ordinary_backup = root.0.join(format!("{FILE_NAME}.bak"));
        fs::remove_file(&ordinary_backup).unwrap(); fs::create_dir(&ordinary_backup).unwrap();
        let before = recovery_disk(&root.0);
        assert!(vault.reset_with_password("valid replacement password").is_err());
        assert!(!vault.status().unlocked); assert_eq!(before, recovery_disk(&root.0));
        vault.unlock(&root.0, PASSWORD).unwrap();
        assert!(vault.restore_recovery_backup(&selected.id, PASSWORD).is_err());
        assert!(!vault.status().unlocked); assert_eq!(before, recovery_disk(&root.0));
    }
    #[test]
    fn recovery_failed_publish_keeps_active_present_and_archives_are_ciphertext() {
        let root = Directory::new(); let vault = VaultState::for_browser_session();
        vault.create(&root.0, PASSWORD).unwrap(); vault.put("item", "original").unwrap(); vault.lock();
        let original = fs::read(root.0.join(FILE_NAME)).unwrap();
        let bak = fs::read(root.0.join(format!("{FILE_NAME}.bak"))).unwrap();
        let new = fresh_vault("replacement test password").unwrap(); let bytes = encode(&new, &new.secrets).unwrap();
        let result = archive_and_replace_with(&root.0, &original, &bytes, |staged, target| {
            assert_eq!(fs::read(target).unwrap(), original); // No remove-first gap.
            assert_eq!(fs::read(staged).unwrap(), bytes);
            Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, "injected publish failure"))
        });
        assert!(result.is_err()); assert!(!vault.status().unlocked);
        assert_eq!(fs::read(root.0.join(FILE_NAME)).unwrap(), original);
        assert_eq!(fs::read(root.0.join(format!("{FILE_NAME}.bak"))).unwrap(), bak);
        let backups = vault.recovery_backups().unwrap(); assert_eq!(backups.len(), 2);
        assert!(backups.iter().any(|b| archive_bytes(&root.0, b) == original));
        assert!(backups.iter().any(|b| archive_bytes(&root.0, b) == bak));
        assert!(!fs::read_dir(&root.0).unwrap().any(|e| e.unwrap().file_name().to_string_lossy().ends_with(".tmp")));
    }
    #[test]
    fn repeated_resets_never_overwrite_archives_and_current_is_first() {
        let root = Directory::new(); let vault = VaultState::for_browser_session();
        vault.create(&root.0, PASSWORD).unwrap(); vault.put("old", "old value").unwrap();
        let first = vault.reset_with_password("first replacement password").unwrap();
        let prior: BTreeMap<_,_> = vault.recovery_backups().unwrap().into_iter()
            .map(|b| (b.id.clone(), archive_bytes(&root.0, &b))).collect();
        let current_before = fs::read(root.0.join(FILE_NAME)).unwrap();
        let second = vault.reset_with_password("second replacement password").unwrap();
        let list = vault.recovery_backups().unwrap(); assert_eq!(list[0], second);
        assert_ne!(first.id, second.id); assert_eq!(archive_bytes(&root.0, &second), current_before);
        for (id, bytes) in prior { assert_eq!(fs::read(recovery_path(&root.0, &id).unwrap()).unwrap(), bytes); }
        for pair in list.windows(2) {
            assert!(pair[0].created > pair[1].created || (pair[0].created == pair[1].created && pair[0].id > pair[1].id));
        }
        let a = recovery_stamp().unwrap(); let b = recovery_stamp().unwrap(); assert!(b > a);
    }
    #[cfg(unix)]
    #[test]
    fn recovery_rejects_archive_and_root_symlinks_and_keeps_permissions_private() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let root = Directory::new(); let vault = VaultState::for_browser_session();
        vault.create(&root.0, PASSWORD).unwrap(); vault.put("item", "private").unwrap();
        let original = fs::read(root.0.join(FILE_NAME)).unwrap();
        let created = vault.reset_with_password("another replacement password").unwrap();
        for backup in vault.recovery_backups().unwrap() {
            assert_eq!(fs::metadata(recovery_path(&root.0, &backup.id).unwrap()).unwrap().permissions().mode() & 0o777, 0o600);
        }
        let outside = Directory::new(); let outside_file = outside.0.join("outside"); fs::write(&outside_file, &original).unwrap();
        let fake = format!("{:020}-{}", recovery_stamp().unwrap(), "f".repeat(32));
        symlink(&outside_file, recovery_path(&root.0, &fake).unwrap()).unwrap();
        let before = recovery_disk(&root.0);
        assert!(!vault.recovery_backups().unwrap().iter().any(|b| b.id == fake));
        assert!(vault.restore_recovery_backup(&fake, PASSWORD).is_err());
        assert!(!vault.status().unlocked); assert_eq!(before, recovery_disk(&root.0));
        assert_eq!(fs::read(&outside_file).unwrap(), original);
        let link_root = outside.0.join("linked-root"); symlink(&root.0, &link_root).unwrap();
        let linked = VaultState::for_browser_session(); linked.configure(&link_root).unwrap();
        assert!(linked.reset_with_password("replacement master password").is_err());
        assert!(linked.restore_recovery_backup(&created.id, PASSWORD).is_err());
        assert!(!linked.status().unlocked); assert_eq!(before, recovery_disk(&root.0));
    }
    #[cfg(unix)]
    #[test]
    fn recovery_never_follows_active_or_ordinary_backup_symlinks() {
        use std::os::unix::fs::symlink;
        let root = Directory::new(); let outside = Directory::new();
        let vault = VaultState::for_browser_session(); vault.create(&root.0, PASSWORD).unwrap();
        vault.put("item", "keep original").unwrap();
        let original = fs::read(root.0.join(FILE_NAME)).unwrap();
        let selected = immutable_archive(&root.0, &original).unwrap();
        let outside_file = outside.0.join("must-not-change"); fs::write(&outside_file, &original).unwrap();
        let bak = root.0.join(format!("{FILE_NAME}.bak")); let bak_bytes = fs::read(&bak).unwrap();
        fs::remove_file(&bak).unwrap(); symlink(&outside_file, &bak).unwrap();
        let before = recovery_disk(&root.0);
        assert!(vault.reset_with_password("new recovery password").is_err());
        assert!(vault.restore_recovery_backup(&selected.id, PASSWORD).is_err());
        assert!(!vault.status().unlocked); assert_eq!(before, recovery_disk(&root.0));
        assert_eq!(fs::read_link(&bak).unwrap(), outside_file);
        fs::remove_file(&bak).unwrap(); fs::write(&bak, bak_bytes).unwrap();
        let active = root.0.join(FILE_NAME); fs::remove_file(&active).unwrap(); symlink(&outside_file, &active).unwrap();
        let before = recovery_disk(&root.0);
        assert!(vault.reset_with_password("new recovery password").is_err());
        assert!(vault.restore_recovery_backup(&selected.id, PASSWORD).is_err());
        assert!(!vault.status().unlocked); assert_eq!(before, recovery_disk(&root.0));
        assert_eq!(fs::read_link(&active).unwrap(), outside_file);
        assert_eq!(fs::read(&outside_file).unwrap(), original);
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
    #[test]
    fn batch_import_stale_second_entry_never_partially_writes_first() {
        let root=Directory::new();let vault=VaultState::default();vault.create(&root.0,PASSWORD).unwrap();
        vault.put("a","old-a").unwrap();vault.put("b","old-b").unwrap();let before=fs::read(root.0.join(FILE_NAME)).unwrap();
        assert!(vault.compare_and_set_many(&[("a",Some("old-a"),Some("new-a")),("b",Some("stale"),Some("new-b"))]).is_err());
        assert_eq!(vault.get("a").unwrap().unwrap().as_str(),"old-a");assert_eq!(before,fs::read(root.0.join(FILE_NAME)).unwrap());
        vault.compare_and_set_many(&[("a",Some("old-a"),Some("new-a")),("b",Some("old-b"),None),("c",None,Some("new-c"))]).unwrap();
        vault.lock();vault.unlock(&root.0,PASSWORD).unwrap();assert_eq!(vault.get("a").unwrap().unwrap().as_str(),"new-a");assert!(vault.get("b").unwrap().is_none());assert_eq!(vault.get("c").unwrap().unwrap().as_str(),"new-c");
        assert!(vault.compare_and_set_many(&[("same",None,Some("x")),("same",None,Some("y"))]).is_err());
    }

}
