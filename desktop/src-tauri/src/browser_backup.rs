//! Explicit portable backups, separate authenticated password export, and bounded import previews.
//! Never copy profile directories, account keys, private sessions, or raw proxy configuration.
use crate::{passwords::Credential, secure_vault::VaultState};
use argon2::{Algorithm, Argon2, Params, Version};
use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use chacha20poly1305::{
    aead::{Aead, KeyInit, Payload},
    XChaCha20Poly1305, XNonce,
};
use rand_core::{OsRng, RngCore};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
    time::{Duration, Instant},
};
use tauri::{Emitter, Manager};
use tauri_plugin_dialog::DialogExt;
use zeroize::Zeroizing;

type Result<T> = std::result::Result<T, String>;
const MAX: usize = 16 * 1024 * 1024;
const MAX_ITEMS: usize = 20_000;
const JOURNAL: &str = "browser-backup-import.v1";
const FILES: [&str; 3] = ["nastrojki.json", "zakladki.json", "sohranennye.json"];
const PORTABLE: [&str; 9] = [
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
const AAD: &[u8] = b"NEXUSBRO_PORTABLE_PASSWORD_BACKUP_V1\0";
static SERIAL: Mutex<()> = Mutex::new(());
static PENDING: OnceLock<Mutex<BTreeMap<String, Pending>>> = OnceLock::new();
fn error() -> String {
    "Резервная копия повреждена или имеет неподдерживаемый формат".into()
}
fn unlocked_generation(expected: u64) -> Result<()> {
    let (locked, generation) = crate::browser_lock::engine_state();
    if locked || generation != expected {
        Err("Состояние защиты изменилось. Повторите операцию после разблокировки браузера.".into())
    } else {
        Ok(())
    }
}
fn changed() -> String {
    "Данные изменились после предварительного просмотра. Откройте копию заново.".into()
}
fn random<const N: usize>() -> Result<[u8; N]> {
    let mut value = [0; N];
    OsRng
        .try_fill_bytes(&mut value)
        .map_err(|_| "Недоступен источник случайных чисел")?;
    Ok(value)
}
fn id() -> Result<String> {
    Ok(random::<16>()?.iter().map(|b| format!("{b:02x}")).collect())
}
fn hash(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
fn normal(id: &str) -> bool {
    !id.is_empty()
        && id.as_bytes()[0].is_ascii_alphanumeric()
        && id.len() <= 128
        && !crate::privatnost::eto_id(id)
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
}
fn bounded(value: &str, max: usize) -> Result<()> {
    if value.len() > max || value.contains('\0') {
        Err(error())
    } else {
        Ok(())
    }
}
fn url(value: &str, blank: bool) -> Result<String> {
    if blank && value == "about:blank" {
        return Ok(value.into());
    }
    bounded(value, 8192)?;
    let mut u = reqwest::Url::parse(value).map_err(|_| error())?;
    if !matches!(u.scheme(), "http" | "https") || u.host_str().is_none() {
        return Err(error());
    }
    u.set_username("").map_err(|_| error())?;
    u.set_password(None).map_err(|_| error())?;
    // Portable metadata must not export URL credentials, signed queries or fragments.
    u.set_query(None);
    u.set_fragment(None);
    Ok(u.to_string())
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Super {
    id: String,
    name: String,
    color: String,
    start: String,
    block_ads: bool,
    empty: bool,
    order: i64,
    requires_route: bool,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Bookmark {
    url: String,
    title: String,
    folder: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    format: String,
    version: u32,
    settings: Value,
    supers: Vec<Super>,
    bookmarks: Vec<Bookmark>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PasswordFile {
    format: String,
    version: u32,
    passwords: Vec<Credential>,
    #[serde(default)]
    super_names: BTreeMap<String, String>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    format: String,
    version: u32,
    memory_kib: u32,
    iterations: u32,
    lanes: u32,
    salt: String,
    nonce: String,
    ciphertext: String,
}
fn derive(pass: &str, salt: &[u8]) -> Result<Zeroizing<[u8; 32]>> {
    if pass.chars().count() < 12 || pass.len() > 1024 {
        return Err("Пароль резервной копии должен содержать от 12 символов (до 1024 байт)".into());
    }
    let params = Params::new(19_456, 2, 1, Some(32)).map_err(|_| error())?;
    let mut key = Zeroizing::new([0u8; 32]);
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(pass.as_bytes(), salt, &mut *key)
        .map_err(|_| error())?;
    Ok(key)
}
fn seal(plain: &[u8], pass: &str) -> Result<Vec<u8>> {
    if plain.len() > MAX {
        return Err(error());
    }
    let salt = random::<16>()?;
    let nonce = random::<24>()?;
    let key = derive(pass, &salt)?;
    let ciphertext = XChaCha20Poly1305::new_from_slice(&*key)
        .map_err(|_| error())?
        .encrypt(
            XNonce::from_slice(&nonce),
            Payload {
                msg: plain,
                aad: AAD,
            },
        )
        .map_err(|_| error())?;
    serde_json::to_vec_pretty(&Envelope {
        format: "nexusbro-password-backup".into(),
        version: 1,
        memory_kib: 19_456,
        iterations: 2,
        lanes: 1,
        salt: B64.encode(salt),
        nonce: B64.encode(nonce),
        ciphertext: B64.encode(ciphertext),
    })
    .map_err(|_| error())
}
fn open(bytes: &[u8], pass: &str) -> Result<Zeroizing<Vec<u8>>> {
    if bytes.len() > MAX * 2 {
        return Err(error());
    }
    let e: Envelope = serde_json::from_slice(bytes).map_err(|_| error())?;
    // Reject hostile KDF parameters before allocation/work. Never trust file-supplied costs.
    if e.format != "nexusbro-password-backup"
        || e.version != 1
        || e.memory_kib != 19_456
        || e.iterations != 2
        || e.lanes != 1
        || e.salt.len() != 24
        || e.nonce.len() != 32
        || e.ciphertext.len() > MAX * 2
    {
        return Err(error());
    }
    let salt = B64.decode(e.salt).map_err(|_| error())?;
    let nonce = B64.decode(e.nonce).map_err(|_| error())?;
    if salt.len() != 16 || nonce.len() != 24 {
        return Err(error());
    }
    let ciphertext = B64.decode(e.ciphertext).map_err(|_| error())?;
    let key = derive(pass, &salt)?;
    let plain = Zeroizing::new(
        XChaCha20Poly1305::new_from_slice(&*key)
            .map_err(|_| error())?
            .decrypt(
                XNonce::from_slice(&nonce),
                Payload {
                    msg: &ciphertext,
                    aad: AAD,
                },
            )
            .map_err(|_| "Неверный пароль резервной копии или файл повреждён".to_string())?,
    );
    if plain.len() > MAX {
        return Err(error());
    }
    Ok(plain)
}
fn read(path: &Path, limit: usize) -> Result<Vec<u8>> {
    let meta = fs::symlink_metadata(path).map_err(|_| "Не удалось прочитать резервную копию")?;
    if !meta.is_file() || meta.file_type().is_symlink() || meta.len() > limit as u64 {
        return Err(error());
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(0x0020_0000);
    }
    let file = options.open(path).map_err(|_| error())?;
    // Check the opened handle, not only the path observed before open. Linux
    // NOFOLLOW and Windows OPEN_REPARSE_POINT prevent a swapped leaf symlink.
    let opened = file.metadata().map_err(|_| error())?;
    if !opened.is_file() || opened.file_type().is_symlink() || opened.len() > limit as u64 {
        return Err(error());
    }
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| error())?;
    if bytes.len() > limit {
        return Err(error());
    }
    Ok(bytes)
}
fn read_optional(path: &Path) -> Result<Option<Vec<u8>>> {
    match fs::symlink_metadata(path) {
        Ok(_) => read(path, MAX).map(Some),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err(error()),
    }
}
fn typed<T: serde::de::DeserializeOwned>(
    raw: Option<&Vec<u8>>,
    default: impl FnOnce() -> T,
) -> Result<T> {
    raw.map(|v| serde_json::from_slice(v).map_err(|_| error()))
        .unwrap_or_else(|| Ok(default()))
}
fn create_file(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path).map_err(|_| {
        "Не удалось создать файл. Выберите новое имя, существующие файлы не перезаписываются."
    })?;
    if file.write_all(bytes).and_then(|_| file.sync_all()).is_err() {
        drop(file);
        let _ = fs::remove_file(path);
        return Err("Не удалось полностью сохранить резервную копию".into());
    }
    Ok(())
}
fn sync_dir(root: &Path) {
    if let Ok(file) = File::open(root) {
        let _ = file.sync_all();
    }
}
fn atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    if fs::symlink_metadata(path).is_ok_and(|m| !m.is_file() || m.file_type().is_symlink()) {
        return Err(error());
    }
    let temp = path.with_file_name(format!(".backup-{}.tmp", id()?));
    create_file(&temp, bytes)?;
    let result =
        fs::rename(&temp, path).map_err(|_| "Не удалось применить резервную копию".to_string());
    if result.is_err() {
        let _ = fs::remove_file(temp);
    }
    if let Some(parent) = path.parent() {
        sync_dir(parent);
    }
    result
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Journal {
    version: u32,
    before: BTreeMap<String, Option<String>>,
}
/// Must run before ANY settings/profile reads at startup. Incomplete imports roll back.
pub fn recover_pending(root: &Path) -> Result<()> {
    let path = root.join(JOURNAL);
    let bytes = match fs::symlink_metadata(&path) {
        Ok(_) => read(&path, MAX * 5)?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(_) => return Err(error()),
    };
    let j: Journal = serde_json::from_slice(&bytes).map_err(|_| error())?;
    if j.version != 1
        || j.before.len() != FILES.len()
        || FILES.iter().any(|name| !j.before.contains_key(*name))
    {
        return Err(error());
    }
    let mut decoded = BTreeMap::new();
    for (name, before) in j.before {
        let raw = before
            .map(|encoded| B64.decode(encoded).map_err(|_| error()))
            .transpose()?;
        if raw.as_ref().is_some_and(|v| v.len() > MAX) {
            return Err(error());
        }
        decoded.insert(name, raw);
    }
    for (name, before) in decoded {
        let path = root.join(name);
        if let Some(raw) = before {
            atomic(&path, &raw)?;
        } else {
            match fs::remove_file(path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => return Err(error()),
            }
        }
    }
    fs::remove_file(root.join(JOURNAL)).map_err(|_| error())?;
    sync_dir(root);
    Ok(())
}
fn transaction(
    root: &Path,
    before: &BTreeMap<String, Option<Vec<u8>>>,
    after: &BTreeMap<String, Vec<u8>>,
) -> Result<()> {
    if root.join(JOURNAL).exists() {
        return Err(
            "Не завершено восстановление предыдущего импорта; перезапустите браузер".into(),
        );
    }
    fs::create_dir_all(root).map_err(|_| error())?;
    let journal = Journal {
        version: 1,
        before: before
            .iter()
            .map(|(k, v)| (k.clone(), v.as_ref().map(|b| B64.encode(b))))
            .collect(),
    };
    atomic(
        &root.join(JOURNAL),
        &serde_json::to_vec(&journal).map_err(|_| error())?,
    )?;
    for (name, bytes) in after {
        if atomic(&root.join(name), bytes).is_err() {
            recover_pending(root).map_err(|_| {
                "Откат импорта не завершён. Перезапустите браузер перед изменением данных."
            })?;
            return Err("Импорт не завершён; предыдущие данные восстановлены".into());
        }
    }
    // Keep the journal if final cleanup fails. Never claim success then roll back on next startup.
    fs::remove_file(root.join(JOURNAL))
        .map_err(|_| "Импорт ожидает завершения. Перезапустите браузер.")?;
    sync_dir(root);
    Ok(())
}
fn snapshot(root: &Path) -> Result<BTreeMap<String, Option<Vec<u8>>>> {
    FILES
        .iter()
        .map(|name| Ok((name.to_string(), read_optional(&root.join(name))?)))
        .collect()
}
fn state_hash(
    files: &BTreeMap<String, Option<Vec<u8>>>,
    live: &BTreeMap<String, crate::Nastrojki>,
) -> Result<String> {
    let mut bytes = serde_json::to_vec(files).map_err(|_| error())?;
    bytes.extend(serde_json::to_vec(live).map_err(|_| error())?);
    Ok(hash(&bytes))
}
fn password_hash(vault: &VaultState) -> Result<String> {
    let mut hasher = Sha256::new();
    for key in vault.keys("password/")? {
        hasher.update((key.len() as u64).to_le_bytes());
        hasher.update(key.as_bytes());
        if let Some(value) = vault.get(&key)? {
            hasher.update((value.len() as u64).to_le_bytes());
            hasher.update(value.as_bytes());
        }
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}
fn all_supers(
    files: &BTreeMap<String, Option<Vec<u8>>>,
    live: &BTreeMap<String, crate::Nastrojki>,
) -> Result<BTreeMap<String, crate::Nastrojki>> {
    let saved: Vec<crate::Nastrojki> = typed(files["sohranennye.json"].as_ref(), Vec::new)?;
    // A private declaration in either source wins. Legacy profiles can have
    // privatnaya=true without the modern pvt- prefix; conflicting stale saved
    // and live definitions must not turn private data into a portable record.
    let private: BTreeSet<_> = saved
        .iter()
        .chain(live.values())
        .filter(|n| n.privatnaya || !normal(&n.id))
        .map(|n| n.id.clone())
        .collect();
    let mut out: BTreeMap<_, _> = saved
        .into_iter()
        .filter(|n| !private.contains(&n.id))
        .map(|n| (n.id.clone(), n))
        .collect();
    out.extend(
        live.iter()
            .filter(|(id, _)| !private.contains(*id))
            .map(|(id, n)| (id.clone(), n.clone())),
    );

    Ok(out)
}
fn config(
    files: &BTreeMap<String, Option<Vec<u8>>>,
    live: &BTreeMap<String, crate::Nastrojki>,
) -> Result<Config> {
    let settings: crate::nastrojki::Nastrojki =
        typed(files["nastrojki.json"].as_ref(), Default::default)?;
    let all = serde_json::to_value(settings).map_err(|_| error())?;
    let mut portable = serde_json::Map::new();
    for key in PORTABLE {
        portable.insert(key.into(), all[key].clone());
    }
    portable.insert(
        "novaya_stranica".into(),
        json!(url(
            all["novaya_stranica"].as_str().ok_or_else(error)?,
            true
        )?),
    );
    let allowed = all_supers(files, live)?;
    let allowed_ids: BTreeSet<_> = allowed.keys().cloned().collect();
    let supers = allowed
        .into_values()
        .map(|n| {
            Ok(Super {
                id: n.id,
                name: n.nazvanie,
                color: n.cvet,
                start: url(&n.adres, true)?,
                block_ads: n.rezat_reklamu,
                empty: n.pustaya,
                order: n.poryadok,
                requires_route: !n.proxy.is_empty(),
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let bookmarks: Vec<crate::biblioteka::Zapis> =
        typed(files["zakladki.json"].as_ref(), Vec::new)?;
    let bookmarks = bookmarks
        .into_iter()
        .filter(|b| {
            b.super_id
                .as_ref()
                .is_none_or(|id| allowed_ids.contains(id))
        })
        .map(|b| {
            Ok(Bookmark {
                url: url(&b.adres, false)?,
                title: b.nazvanie,
                folder: b.papka,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let config = Config {
        format: "nexusbro-portable-config".into(),
        version: 1,
        settings: Value::Object(portable),
        supers,
        bookmarks,
    };
    validate_config(&config)?;
    Ok(config)
}
fn settings(
    config: &Value,
    mut current: crate::nastrojki::Nastrojki,
) -> Result<crate::nastrojki::Nastrojki> {
    let fields = config.as_object().ok_or_else(error)?;
    if fields.keys().any(|k| !PORTABLE.contains(&k.as_str())) {
        return Err(error());
    }
    let mut value = serde_json::to_value(&current).map_err(|_| error())?;
    for (key, v) in fields {
        value[key] = v.clone();
    }
    current = serde_json::from_value(value).map_err(|_| error())?;
    if ![
        "temnaya",
        "svetlaya",
        "sistemnaya",
        "grafit",
        "nexus",
        "okean",
        "sliva",
        "pesok",
    ]
    .contains(&current.tema.as_str())
        || !["system", "segoe", "arial", "verdana", "georgia", "consolas"]
            .contains(&current.shrift.as_str())
        || !["udobno", "kompaktno"].contains(&current.plotnost.as_str())
        || !["duckduckgo", "google", "yandex"].contains(&current.poisk.as_str())
        || !(12..=20).contains(&current.razmer_shrifta)
        || !(50..=200).contains(&current.masshtab_stranic)
        || current.yazyk_perevoda.is_empty()
        || current.yazyk_perevoda.len() > 12
        || !current
            .yazyk_perevoda
            .bytes()
            .all(|b| b.is_ascii_alphabetic() || b == b'-')
        || url(&current.novaya_stranica, true)? != current.novaya_stranica
    {
        return Err(error());
    }
    Ok(current)
}
fn validate_config(c: &Config) -> Result<()> {
    if c.format != "nexusbro-portable-config"
        || c.version != 1
        || c.supers.len() > 1000
        || c.bookmarks.len() > MAX_ITEMS
    {
        return Err(error());
    }
    settings(&c.settings, Default::default())?;
    let mut ids = BTreeSet::new();
    for s in &c.supers {
        if !normal(&s.id)
            || !ids.insert(&s.id)
            || s.name.trim().is_empty()
            || s.name.chars().count() > 80
            || !crate::supervkladki::cvet_dopustim(&s.color)
            || url(&s.start, true)? != s.start
        {
            return Err(error());
        }
        bounded(&s.name, 320)?;
    }
    for b in &c.bookmarks {
        if url(&b.url, false)? != b.url {
            return Err(error());
        }
        bounded(&b.title, 1024)?;
        bounded(&b.folder, 512)?;
    }
    Ok(())
}
fn passwords(bytes: &[u8], pass: &str) -> Result<PasswordFile> {
    let plain = open(bytes, pass)?;
    let p: PasswordFile = serde_json::from_slice(&plain).map_err(|_| error())?;
    if p.format != "nexusbro-password-records"
        || p.version != 1
        || p.passwords.len() > 5000
        || p.super_names.len() > 5000
    {
        return Err(error());
    }
    for (id, name) in &p.super_names {
        if !normal(id) {
            return Err(error());
        }
        bounded(name, 320)?;
    }
    let mut ids = BTreeSet::new();
    for c in &p.passwords {
        if !normal(&c.id)
            || !ids.insert(&c.id)
            || !normal(&c.super_id)
            || crate::passwords::secure_origin(&c.origin)? != c.origin
            || c.username.is_empty()
            || c.password.is_empty()
        {
            return Err(error());
        }
        bounded(&c.username, 512)?;
        bounded(&c.password, 4096)?;
        bounded(&c.title, 800)?;
        if c.title.chars().count() > 200 {
            return Err(error());
        }
    }
    Ok(p)
}
struct Pending {
    created: Instant,
    window: String,
    path: PathBuf,
    source_hash: String,
    target_hash: String,
    password_hash: Option<String>,
    password: bool,
    summary: Value,
    routes: BTreeMap<String, String>,
    mapping: BTreeMap<String, String>,
    settings: bool,
    ready: bool,
    lock_generation: u64,
}
fn cache() -> &'static Mutex<BTreeMap<String, Pending>> {
    PENDING.get_or_init(Default::default)
}
fn prune(entries: &mut BTreeMap<String, Pending>) {
    entries.retain(|_, p| p.created.elapsed() < Duration::from_secs(120));
}
fn maps(data: &Value, name: &str) -> Result<BTreeMap<String, String>> {
    if data[name].is_null() {
        return Ok(BTreeMap::new());
    }
    serde_json::from_value(data[name].clone()).map_err(|_| error())
}
fn choose(app: &tauri::AppHandle, save: bool, password: bool) -> Result<Option<PathBuf>> {
    let builder = app.dialog().file().add_filter("NexusBro", &["json"]);
    let picked = if save {
        builder
            .set_file_name(if password {
                "nexusbro-passwords.encrypted.json"
            } else {
                "nexusbro-settings.json"
            })
            .blocking_save_file()
    } else {
        builder.blocking_pick_file()
    };
    picked
        .map(|p| p.into_path().map_err(|_| "Выберите локальный файл".into()))
        .transpose()
}
#[tauri::command]
pub async fn security_backup(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
    vault: tauri::State<'_, Arc<VaultState>>,
    action: String,
    data: Option<Value>,
) -> Result<Value> {
    let generation = crate::browser_lock::generation();
    let owner = window.label().to_string();
    let vault = Arc::clone(&vault);
    let mut data = data.unwrap_or(Value::Null);
    // Take, rather than clone, the frontend passphrase; never retain it in the preview cache.
    let pass = Zeroizing::new(
        match data.as_object_mut().and_then(|m| m.remove("passphrase")) {
            Some(Value::String(value)) => value,
            _ => String::new(),
        },
    );
    tauri::async_runtime::spawn_blocking(move || run(&app, &owner, &vault, &action, &data, &pass, generation))
        .await
        .map_err(|_| "Ошибка резервного копирования".to_string())?
}
fn run(
    app: &tauri::AppHandle,
    owner: &str,
    vault: &VaultState,
    action: &str,
    data: &Value,
    pass: &str,
    lock_generation: u64,
) -> Result<Value> {
    let _serial = SERIAL.lock().map_err(|_| "Резервное копирование занято")?;
    unlocked_generation(lock_generation)?;
    if action == "cancel" {
        if let Some(token) = data["token"].as_str() {
            let mut entries = cache().lock().map_err(|_| error())?;
            if entries.get(token).is_some_and(|p| p.window == owner) {
                entries.remove(token);
            }
        }
        return Ok(json!({"cancelled":true}));
    }
    let password = matches!(action, "export_passwords" | "preview_passwords");
    let chosen = if action.starts_with("export_") || action.starts_with("preview_") {
        let Some(path) = choose(app, action.starts_with("export_"), password)? else {
            return Ok(json!({"cancelled":true}));
        };
        Some(path)
    } else {
        None
    };
    // Never hold the async vault-operation queue while a native file picker
    // waits for the user. Recheck the invocation epoch after joining the queue.
    let sync = app.state::<crate::browser_sync::SyncState>();
    let _operation = sync.serial.blocking_lock();
    // A native file chooser can stay open while another window locks and even
    // unlocks the browser. Both the bit and generation must still match.
    unlocked_generation(lock_generation)?;
    let root = crate::brauzer::papka_profilej()
        .parent()
        .ok_or_else(error)?
        .to_path_buf();
    // Lock order: live -> settings -> bookmarks -> saved -> vault. Existing
    // sync apply/history restore already holds live before saved (then vault).
    // Never wait for live while holding saved, or hold a vault guard before live.
    // File dialogs and await points are outside this complete synchronous scope.
    let state: tauri::State<crate::Sostoyanie> = app.state();
    let live_guard = state.vkladki.lock().map_err(|_| error())?;
    let _settings = crate::nastrojki::ZAPIS.lock().map_err(|_| error())?;
    let _bookmarks = crate::biblioteka::IO.lock().map_err(|_| error())?;
    let _saved = crate::supervkladki::SOHRANENNYE
        .lock()
        .map_err(|_| error())?;
    let live: BTreeMap<_, _> = live_guard
        .iter()
        .map(|(id, v)| (id.clone(), v.nastrojki.clone()))
        .collect();
    let files = snapshot(&root)?;
    let target_hash = state_hash(&files, &live)?;
    let current: crate::nastrojki::Nastrojki =
        typed(files["nastrojki.json"].as_ref(), Default::default)?;
    let all = all_supers(&files, &live)?;
    if action == "export_config" {
        let bytes = serde_json::to_vec_pretty(&config(&files, &live)?).map_err(|_| error())?;
        if bytes.len() > MAX {
            return Err(error());
        }
        unlocked_generation(lock_generation)?;
        create_file(chosen.as_ref().ok_or_else(error)?, &bytes)?;
        return Ok(json!({"exported":true,"kind":"config","url_queries_removed":true}));
    }
    if action == "export_passwords" {
        let mut rows = vec![];
        for key in vault.keys("password/")? {
            if let Some(raw) = vault.get(&key)? {
                let row: Credential = serde_json::from_str(&raw).map_err(|_| error())?;
                if all.contains_key(&row.super_id) {
                    rows.push(row);
                }
            }
        }
        let file = PasswordFile {
            format: "nexusbro-password-records".into(),
            version: 1,
            super_names: all
                .iter()
                .map(|(id, n)| (id.clone(), n.nazvanie.clone()))
                .collect(),
            passwords: rows,
        };
        let plain = Zeroizing::new(serde_json::to_vec(&file).map_err(|_| error())?);
        let bytes = seal(&plain, pass)?;
        unlocked_generation(lock_generation)?;
        create_file(chosen.as_ref().ok_or_else(error)?, &bytes)?;
        return Ok(json!({"exported":true,"kind":"passwords","count":file.passwords.len()}));
    }
    if action == "preview_config" || action == "preview_passwords" {
        let path = chosen.ok_or_else(error)?;
        let bytes = read(&path, if password { MAX * 2 } else { MAX })?;
        let summary = if password {
            let file = passwords(&bytes, pass)?;
            let source_ids: BTreeSet<_> =
                file.passwords.iter().map(|c| c.super_id.clone()).collect();
            json!({"kind":"passwords","passwords":file.passwords.len(),"supers":source_ids.into_iter().enumerate().map(|(i,id)|json!({"name":file.super_names.get(&id).cloned().unwrap_or_else(||format!("Группа {}",i+1)),"id":id})).collect::<Vec<_>>()})
        } else {
            let c: Config = serde_json::from_slice(&bytes).map_err(|_| error())?;
            validate_config(&c)?;
            json!({"kind":"config","bookmarks":c.bookmarks.len(),"settings_available":true,"supers":c.supers.iter().map(|s|json!({"id":s.id,"name":s.name,"requires_route":s.requires_route})).collect::<Vec<_>>()})
        };
        let token = id()?;
        let mut out = summary.clone();
        out["token"] = json!(token);
        out["ready"] = json!(false);
        out["local_routes"] = json!(current
            .vyhody
            .iter()
            .map(|v| json!({"id":v.id,"name":v.nazvanie}))
            .collect::<Vec<_>>());
        out["local_supers"] = json!(all
            .values()
            .map(|s| json!({"id":s.id,"name":s.nazvanie}))
            .collect::<Vec<_>>());
        let mut entries = cache().lock().map_err(|_| error())?;
        prune(&mut entries);
        if entries.len() >= 4 {
            return Err("Закройте предыдущие предварительные просмотры".into());
        }
        entries.insert(
            token,
            Pending {
                created: Instant::now(),
                window: owner.into(),
                path,
                source_hash: hash(&bytes),
                target_hash,
                password_hash: if password {
                    Some(password_hash(vault)?)
                } else {
                    None
                },
                password,
                summary,
                routes: BTreeMap::new(),
                mapping: BTreeMap::new(),
                settings: false,
                ready: false,
                lock_generation,
            },
        );
        return Ok(out);
    }
    let token = data["token"].as_str().ok_or_else(error)?;
    let mut entries = cache().lock().map_err(|_| error())?;
    prune(&mut entries);
    let pending = entries
        .get_mut(token)
        .filter(|p| p.window == owner)
        .ok_or("Предварительный просмотр истёк. Откройте файл заново.")?;
    unlocked_generation(pending.lock_generation)?;
    if pending.target_hash != target_hash
        || pending
            .password_hash
            .as_ref()
            .is_some_and(|h| password_hash(vault).as_ref() != Ok(h))
    {
        return Err(changed());
    }
    let bytes = read(&pending.path, if pending.password { MAX * 2 } else { MAX })?;
    if hash(&bytes) != pending.source_hash {
        return Err(changed());
    }
    if action == "prepare" {
        let routes = maps(data, "routes")?;
        let mapping = maps(data, "super_mapping")?;
        for s in pending.summary["supers"].as_array().ok_or_else(error)? {
            let source = s["id"].as_str().ok_or_else(error)?;
            if pending.password {
                if !mapping
                    .get(source)
                    .is_some_and(|target| all.contains_key(target))
                {
                    return Err("Выберите обычную супер-вкладку для каждой группы паролей".into());
                }
            } else {
                let route = routes
                    .get(source)
                    .ok_or("Выберите путь для каждой импортируемой супер-вкладки")?;
                if route != "direct" && !current.vyhody.iter().any(|v| &v.id == route) {
                    return Err("Выбранное подключение недоступно".into());
                }
            }
        }
        pending.routes = routes;
        pending.mapping = mapping;
        pending.settings = data["settings"].as_bool().unwrap_or(false);
        pending.ready = true;
        let mut summary = pending.summary.clone();
        summary["token"] = json!(token);
        summary["ready"] = json!(true);
        summary["settings"] = json!(pending.settings);
        return Ok(summary);
    }
    if action != "apply" || data["confirm"] != true || !pending.ready {
        return Err("Сначала проверьте состав импорта и подтвердите его".into());
    }
    if crate::browser_lock::locked_global() {
        return Err("Браузер заблокирован".into());
    }
    if pending.password {
        let file = passwords(&bytes, pass)?;
        let mut owned: Vec<(String, Option<Zeroizing<String>>, Option<Zeroizing<String>>)> = vec![];
        // Preserve exact existing values under the vault's single compare-and-set commit.
        for key in vault.keys("password/")? {
            if let Some(raw) = vault.get(&key)? {
                owned.push((key, Some(raw.clone()), Some(raw)));
            }
        }
        for mut c in file.passwords {
            c.id = id()?;
            c.super_id = pending.mapping.get(&c.super_id).ok_or_else(error)?.clone();
            c.updated_at = crate::passwords::now();
            let value = Zeroizing::new(serde_json::to_string(&c).map_err(|_| error())?);
            owned.push((format!("password/{}", c.id), None, Some(value)));
        }
        let refs: Vec<_> = owned
            .iter()
            .map(|(k, e, v)| {
                (
                    k.as_str(),
                    e.as_ref().map(|s| s.as_str()),
                    v.as_ref().map(|s| s.as_str()),
                )
            })
            .collect();
        unlocked_generation(lock_generation)?;
        vault.compare_and_set_many(&refs)?;
    } else {
        let c: Config = serde_json::from_slice(&bytes).map_err(|_| error())?;
        validate_config(&c)?;
        let mut saved: Vec<crate::Nastrojki> = typed(files["sohranennye.json"].as_ref(), Vec::new)?;
        for s in c.supers {
            let route = pending.routes.get(&s.id).ok_or_else(error)?;
            let (proxy, node) = if route == "direct" {
                (String::new(), String::new())
            } else {
                let found = current
                    .vyhody
                    .iter()
                    .find(|v| &v.id == route)
                    .ok_or_else(error)?;
                (found.adres.clone(), found.proxy_uzel.clone())
            };
            saved.push(crate::Nastrojki {
                id: id()?,
                nazvanie: s.name,
                cvet: s.color,
                poryadok: s.order,
                proxy,
                proxy_uzel: node,
                adres: s.start,
                rezat_reklamu: s.block_ads,
                pustaya: s.empty,
                privatnaya: false,
            });
        }
        let mut bookmarks: Vec<crate::biblioteka::Zapis> =
            typed(files["zakladki.json"].as_ref(), Vec::new)?;
        for b in c.bookmarks {
            if !bookmarks.iter().any(|old| old.adres == b.url) {
                bookmarks.push(crate::biblioteka::Zapis {
                    super_id: None,
                    id: format!("b-{}", id()?),
                    adres: b.url,
                    nazvanie: b.title,
                    vremya: crate::biblioteka::vremya(),
                    papka: b.folder,
                });
            }
        }
        if bookmarks.len() > MAX_ITEMS || saved.len() > 2000 {
            return Err("Слишком много записей для импорта".into());
        }
        let settings = if pending.settings {
            settings(&c.settings, current)?
        } else {
            current
        };
        let mut after = BTreeMap::new();
        after.insert(
            "nastrojki.json".into(),
            serde_json::to_vec_pretty(&settings).map_err(|_| error())?,
        );
        after.insert(
            "zakladki.json".into(),
            serde_json::to_vec(&bookmarks).map_err(|_| error())?,
        );
        after.insert(
            "sohranennye.json".into(),
            serde_json::to_vec_pretty(&saved).map_err(|_| error())?,
        );
        for bytes in after.values() {
            if bytes.len() > MAX {
                return Err(error());
            }
        }
        unlocked_generation(lock_generation)?;
        transaction(&root, &files, &after)?;
    }
    entries.remove(token);
    drop(entries);
    drop(_saved);
    drop(_bookmarks);
    drop(_settings);
    drop(live_guard);
    let _ = app.emit("vkladki-izmenilis", "");
    let _ = app.emit("zakladki-izmenilis", ());
    let _ = app.emit("nastrojki-izmenilis", crate::nastrojki::zagruzit());
    Ok(json!({"imported":true}))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn authenticated_password_backup_rejects_wrong_password_tamper_and_hostile_cost() {
        let plain = b"secret credential fixture";
        let pass = "independent backup password";
        let blob = seal(plain, pass).unwrap();
        assert!(!String::from_utf8_lossy(&blob).contains("secret credential fixture"));
        assert_eq!(&*open(&blob, pass).unwrap(), plain);
        assert!(open(&blob, "different backup password").is_err());
        let mut value: Value = serde_json::from_slice(&blob).unwrap();
        value["memory_kib"] = json!(u32::MAX);
        assert!(open(&serde_json::to_vec(&value).unwrap(), pass).is_err());
        let mut value: Value = serde_json::from_slice(&blob).unwrap();
        let mut ct = B64.decode(value["ciphertext"].as_str().unwrap()).unwrap();
        ct[0] ^= 1;
        value["ciphertext"] = json!(B64.encode(ct));
        assert!(open(&serde_json::to_vec(&value).unwrap(), pass).is_err());
    }
    #[test]
    fn exported_urls_strip_auth_queries_fragments_and_reject_local_schemes() {
        assert_eq!(
            url(
                "https://user:password@example.test/path?token=secret#secret",
                false
            )
            .unwrap(),
            "https://example.test/path"
        );
        assert!(url("file:///private", false).is_err());
        assert!(url("javascript:alert(1)", false).is_err());
    }
    #[test]
    fn incomplete_transaction_recovers_previous_files_and_deletes_only_known_new_files() {
        let root = std::env::temp_dir().join(format!("nexusbro-backup-test-{}", id().unwrap()));
        fs::create_dir(&root).unwrap();
        fs::write(root.join("nastrojki.json"), b"before").unwrap();
        let before = snapshot(&root).unwrap();
        let j = Journal {
            version: 1,
            before: before
                .iter()
                .map(|(k, v)| (k.clone(), v.as_ref().map(|b| B64.encode(b))))
                .collect(),
        };
        atomic(&root.join(JOURNAL), &serde_json::to_vec(&j).unwrap()).unwrap();
        fs::write(root.join("nastrojki.json"), b"partial after").unwrap();
        fs::write(root.join("zakladki.json"), b"new partial").unwrap();
        recover_pending(&root).unwrap();
        assert_eq!(fs::read(root.join("nastrojki.json")).unwrap(), b"before");
        assert!(!root.join("zakladki.json").exists());
        assert!(!root.join(JOURNAL).exists());
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn ordinary_export_excludes_private_profiles_proxy_secrets_and_url_tokens() {
        let mut n = crate::nastrojki::Nastrojki::default();
        n.vyhody.push(crate::nastrojki::Vyhod {
            id: "route-secret".into(),
            nazvanie: "Route".into(),
            adres: "socks5://secret-user:secret-pass@host.test:1080".into(),
            proxy_uzel: String::new(),
        });
        let normal:crate::Nastrojki=serde_json::from_value(json!({"id":"ordinary","nazvanie":"Ordinary","proxy":"socks5://proxy-user:proxy-password@host.test:1080","adres":"https://user:password@site.test/path?auth=secret#fragment"})).unwrap();
        let private:crate::Nastrojki=serde_json::from_value(json!({"id":"pvt-hidden","nazvanie":"Private secret name","privatnaya":true,"adres":"https://private.test/"})).unwrap();
        let mut files = BTreeMap::new();
        files.insert(
            "nastrojki.json".into(),
            Some(serde_json::to_vec(&n).unwrap()),
        );
        files.insert(
            "sohranennye.json".into(),
            Some(serde_json::to_vec(&vec![normal, private]).unwrap()),
        );
        files.insert("zakladki.json".into(), Some(b"[]".to_vec()));
        let exported = serde_json::to_string(&config(&files, &BTreeMap::new()).unwrap()).unwrap();
        for forbidden in [
            "secret-user",
            "secret-pass",
            "proxy-user",
            "proxy-password",
            "auth=secret",
            "pvt-hidden",
            "Private secret name",
            "private.test",
            "vyhody",
            "proxy_uzel",
        ] {
            assert!(
                !exported.contains(forbidden),
                "Export leaked forbidden field"
            );
        }
        assert!(exported.contains("requires_route\":true"));
        assert!(exported.contains("https://site.test/path"));
    }
    #[test]
    fn legacy_private_and_conflicting_saved_live_definitions_never_export() {
        let ordinary: crate::Nastrojki =
            serde_json::from_value(json!({"id":"ordinary","nazvanie":"Ordinary"})).unwrap();
        let legacy: crate::Nastrojki = serde_json::from_value(
            json!({"id":"legacy-normal-id","nazvanie":"Legacy private","privatnaya":true}),
        )
        .unwrap();
        let conflicting_saved: crate::Nastrojki =
            serde_json::from_value(json!({"id":"conflicting","nazvanie":"Stale ordinary saved"}))
                .unwrap();
        let conflicting_live: crate::Nastrojki = serde_json::from_value(
            json!({"id":"conflicting","nazvanie":"Current private","privatnaya":true}),
        )
        .unwrap();
        let mut files = BTreeMap::new();
        files.insert(
            "sohranennye.json".into(),
            Some(serde_json::to_vec(&vec![ordinary, legacy, conflicting_saved]).unwrap()),
        );
        let mut live = BTreeMap::new();
        live.insert(conflicting_live.id.clone(), conflicting_live);
        files.insert("nastrojki.json".into(), None);
        files.insert(
            "zakladki.json".into(),
            Some(
                serde_json::to_vec(&vec![
                    crate::biblioteka::Zapis {
                        super_id: Some("legacy-normal-id".into()),
                        id: "private-bookmark".into(),
                        adres: "https://legacy-private.test/".into(),
                        nazvanie: "Private bookmark".into(),
                        vremya: 0,
                        papka: String::new(),
                    },
                    crate::biblioteka::Zapis {
                        super_id: None,
                        id: "public-bookmark".into(),
                        adres: "https://ordinary.test/".into(),
                        nazvanie: "Ordinary bookmark".into(),
                        vremya: 0,
                        papka: String::new(),
                    },
                ])
                .unwrap(),
            ),
        );
        let portable = config(&files, &live).unwrap();
        assert_eq!(portable.bookmarks.len(), 1);
        assert_eq!(portable.bookmarks[0].title, "Ordinary bookmark");
        let allowed = all_supers(&files, &live).unwrap();
        assert_eq!(
            allowed.keys().cloned().collect::<Vec<_>>(),
            vec!["ordinary".to_string()]
        );
        // Password export uses this exact allowlist; a syntactically normal ID
        // by itself is never enough to authorize credential export.
        assert!(!allowed.contains_key("legacy-normal-id"));
        assert!(!allowed.contains_key("conflicting"));
    }
    #[test]
    fn password_backup_group_names_are_optional_for_older_files() {
        let old: PasswordFile = serde_json::from_value(
            json!({"format":"nexusbro-password-records","version":1,"passwords":[]}),
        )
        .unwrap();
        assert!(old.super_names.is_empty());
        let file = PasswordFile {
            format: "nexusbro-password-records".into(),
            version: 1,
            passwords: vec![],
            super_names: BTreeMap::from([("ordinary".into(), "Рабочие аккаунты".into())]),
        };
        let plain = Zeroizing::new(serde_json::to_vec(&file).unwrap());
        let encrypted = seal(&plain, "backup independent password").unwrap();
        assert!(!String::from_utf8_lossy(&encrypted).contains("Рабочие аккаунты"));
        let restored = passwords(&encrypted, "backup independent password").unwrap();
        assert_eq!(restored.super_names["ordinary"], "Рабочие аккаунты");
    }
    #[test]
    fn multi_file_failure_rolls_back_previous_successful_writes() {
        let root = std::env::temp_dir().join(format!("nexusbro-backup-test-{}", id().unwrap()));
        fs::create_dir(&root).unwrap();
        fs::write(root.join("nastrojki.json"), b"before").unwrap();
        let before = snapshot(&root).unwrap();
        let mut after = BTreeMap::new();
        after.insert("nastrojki.json".into(), b"after".to_vec());
        after.insert("zz-missing-parent/file.json".into(), b"fail".to_vec());
        assert!(transaction(&root, &before, &after).is_err());
        assert_eq!(fs::read(root.join("nastrojki.json")).unwrap(), b"before");
        assert!(!root.join(JOURNAL).exists());
        fs::remove_dir_all(root).unwrap();
    }
    #[cfg(unix)]
    #[test]
    fn reading_backup_rejects_symlink_and_over_limit_files() {
        use std::os::unix::fs::symlink;
        let root = std::env::temp_dir().join(format!("nexusbro-backup-test-{}", id().unwrap()));
        fs::create_dir(&root).unwrap();
        fs::write(root.join("file"), b"data").unwrap();
        symlink(root.join("file"), root.join("link")).unwrap();
        assert!(read(&root.join("link"), 100).is_err());
        assert!(read(&root.join("file"), 3).is_err());
        assert_eq!(read(&root.join("file"), 4).unwrap(), b"data");
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn malformed_journal_cannot_escape_data_root() {
        let root = std::env::temp_dir().join(format!("nexusbro-backup-test-{}", id().unwrap()));
        fs::create_dir(&root).unwrap();
        fs::write(
            root.join(JOURNAL),
            br#"{"version":1,"before":{"../outside":null}}"#,
        )
        .unwrap();
        assert!(recover_pending(&root).is_err());
        fs::remove_dir_all(root).unwrap();
    }
}
