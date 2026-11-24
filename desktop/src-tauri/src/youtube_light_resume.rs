//! Exact-Super/context resume positions. Caller performs all file IO on a worker,
//! NEVER CEF UI. The root is parent-derived effective cache context, not renderer
//! input. None/new() is irrevocably RAM-only; no default-profile fallback.
//! Watch history and local paths intentionally have NO Debug implementation.
use crate::youtube_light::VideoId;
use rand_core::{OsRng, RngCore};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    fs::{File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};
pub const MAX_ENTRIES: usize = 512;
pub const MAX_JSON_BYTES: usize = 128 * 1024;
pub const MAX_SECONDS: f64 = 7.0 * 24.0 * 60.0 * 60.0;
const FILE_NAME: &str = "youtube-resume.json";
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResumeError {
    InvalidScope,
    InvalidPosition,
    InvalidData,
    UnsupportedVersion,
    Oversized,
    StorageUnavailable,
    UnsafeFile,
}
impl fmt::Display for ResumeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Не удалось безопасно сохранить или прочитать позицию видео")
    }
}
impl std::error::Error for ResumeError {}
#[derive(Clone, PartialEq, Eq)]
pub struct ResumeScope {
    super_id: String,
    context_key: String,
}
impl ResumeScope {
    pub fn new(super_id: &str, context_key: &str) -> Result<Self, ResumeError> {
        if super_id.is_empty()
            || super_id.len() > 128
            || !super_id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
            || context_key.len() != 64
            || !context_key
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(ResumeError::InvalidScope);
        }
        Ok(Self {
            super_id: super_id.into(),
            context_key: context_key.into(),
        })
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResumePoint {
    pub video: VideoId,
    pub position_seconds: f64,
    pub duration_seconds: Option<f64>,
    pub updated_epoch_seconds: u64,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredScope {
    super_id: String,
    context_key: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Document {
    schema: u32,
    scope: StoredScope,
    entries: Vec<ResumePoint>,
}
struct Entry {
    point: ResumePoint,
    used: u64,
}
pub struct ResumeBook {
    scope: ResumeScope,
    root: Option<PathBuf>,
    entries: BTreeMap<String, Entry>,
    sequence: u64,
}
fn valid(position: f64, duration: Option<f64>) -> bool {
    position.is_finite()
        && (0.0..=MAX_SECONDS).contains(&position)
        && duration.is_none_or(|d| d.is_finite() && d > 0.0 && d <= MAX_SECONDS && position <= d)
}
fn completed(position: f64, duration: Option<f64>) -> bool {
    position == 0.0 || duration.is_some_and(|d| d - position <= 5.0)
}
fn regular(file: &File) -> Result<(), ResumeError> {
    let metadata = file
        .metadata()
        .map_err(|_| ResumeError::StorageUnavailable)?;
    if !metadata.is_file() {
        return Err(ResumeError::UnsafeFile);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return Err(ResumeError::UnsafeFile);
        }
    }
    Ok(())
}
fn root_path(root: &Path) -> Result<PathBuf, ResumeError> {
    let canonical = root
        .canonicalize()
        .map_err(|_| ResumeError::StorageUnavailable)?;
    if !canonical.is_dir() {
        return Err(ResumeError::UnsafeFile);
    }
    Ok(canonical)
}
fn check_target(path: &Path) -> Result<(), ResumeError> {
    match std::fs::symlink_metadata(path) {
        Ok(m) => {
            if m.file_type().is_symlink() || !m.is_file() {
                return Err(ResumeError::UnsafeFile);
            }
            #[cfg(windows)]
            {
                use std::os::windows::fs::MetadataExt;
                if m.file_attributes() & 0x400 != 0 {
                    return Err(ResumeError::UnsafeFile);
                }
            }
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(ResumeError::StorageUnavailable),
    }
}
impl ResumeBook {
    pub fn new(scope: ResumeScope) -> Self {
        Self {
            scope,
            root: None,
            entries: BTreeMap::new(),
            sequence: 0,
        }
    }
    pub fn is_ram_only(&self) -> bool {
        self.root.is_none()
    }
    pub fn len(&self) -> usize {
        self.entries.len()
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
    fn tick(&mut self) -> u64 {
        if self.sequence == u64::MAX {
            let mut order = self
                .entries
                .iter()
                .map(|(id, e)| (e.used, id.clone()))
                .collect::<Vec<_>>();
            order.sort();
            for (index, (_, id)) in order.into_iter().enumerate() {
                self.entries.get_mut(&id).unwrap().used = index as u64;
            }
            self.sequence = self.entries.len() as u64;
        }
        self.sequence += 1;
        self.sequence
    }
    pub fn record(
        &mut self,
        video: &VideoId,
        position_seconds: f64,
        duration_seconds: Option<f64>,
        updated_epoch_seconds: u64,
    ) -> Result<(), ResumeError> {
        if !valid(position_seconds, duration_seconds) {
            return Err(ResumeError::InvalidPosition);
        }
        if completed(position_seconds, duration_seconds) {
            self.remove(video);
            return Ok(());
        }
        let used = self.tick();
        self.entries.insert(
            video.as_str().into(),
            Entry {
                point: ResumePoint {
                    video: video.clone(),
                    position_seconds,
                    duration_seconds,
                    updated_epoch_seconds,
                },
                used,
            },
        );
        if self.entries.len() > MAX_ENTRIES {
            let id = self
                .entries
                .iter()
                .min_by_key(|(_, e)| e.used)
                .unwrap()
                .0
                .clone();
            self.entries.remove(&id);
        }
        Ok(())
    }
    pub fn position(&mut self, video: &VideoId) -> Option<ResumePoint> {
        if !self.entries.contains_key(video.as_str()) {
            return None;
        }
        let used = self.tick();
        let entry = self.entries.get_mut(video.as_str())?;
        entry.used = used;
        Some(entry.point.clone())
    }
    pub fn remove(&mut self, video: &VideoId) -> bool {
        self.entries.remove(video.as_str()).is_some()
    }
    fn encoded(&self) -> Result<Vec<u8>, ResumeError> {
        let mut entries = self.entries.values().collect::<Vec<_>>();
        entries.sort_by_key(|e| e.used);
        let document = Document {
            schema: 1,
            scope: StoredScope {
                super_id: self.scope.super_id.clone(),
                context_key: self.scope.context_key.clone(),
            },
            entries: entries.into_iter().map(|e| e.point.clone()).collect(),
        };
        let bytes = serde_json::to_vec(&document).map_err(|_| ResumeError::InvalidData)?;
        if bytes.len() > MAX_JSON_BYTES {
            return Err(ResumeError::Oversized);
        }
        Ok(bytes)
    }
    pub fn load(
        scope: ResumeScope,
        context_cache_root: Option<&Path>,
    ) -> Result<Self, ResumeError> {
        let Some(root) = context_cache_root else {
            return Ok(Self::new(scope));
        };
        let root = root_path(root)?;
        let path = root.join(FILE_NAME);
        check_target(&path)?;
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            options.custom_flags(0x0020_0000);
        }
        let file = match options.open(&path) {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let mut book = Self::new(scope);
                book.root = Some(root);
                return Ok(book);
            }
            Err(_) => return Err(ResumeError::StorageUnavailable),
        };
        regular(&file)?;
        if file
            .metadata()
            .map_err(|_| ResumeError::StorageUnavailable)?
            .len()
            > MAX_JSON_BYTES as u64
        {
            return Err(ResumeError::Oversized);
        }
        let mut bytes = Vec::new();
        file.take((MAX_JSON_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|_| ResumeError::StorageUnavailable)?;
        if bytes.len() > MAX_JSON_BYTES {
            return Err(ResumeError::Oversized);
        }
        let document: Document =
            serde_json::from_slice(&bytes).map_err(|_| ResumeError::InvalidData)?;
        if document.schema != 1 {
            return Err(ResumeError::UnsupportedVersion);
        }
        if ResumeScope::new(&document.scope.super_id, &document.scope.context_key)? != scope
            || document.entries.len() > MAX_ENTRIES
        {
            return Err(ResumeError::InvalidData);
        }
        let mut seen = BTreeSet::new();
        for point in &document.entries {
            if !seen.insert(point.video.as_str().to_owned())
                || !valid(point.position_seconds, point.duration_seconds)
                || completed(point.position_seconds, point.duration_seconds)
            {
                return Err(ResumeError::InvalidData);
            }
        }
        let mut book = Self::new(scope);
        book.root = Some(root);
        for point in document.entries {
            book.record(
                &point.video,
                point.position_seconds,
                point.duration_seconds,
                point.updated_epoch_seconds,
            )?;
        }
        Ok(book)
    }
    pub fn save(&self) -> Result<(), ResumeError> {
        let Some(root) = &self.root else {
            return Ok(());
        };
        if root_path(root)? != *root {
            return Err(ResumeError::UnsafeFile);
        }
        let path = root.join(FILE_NAME);
        check_target(&path)?;
        let bytes = self.encoded()?;
        let mut last = None;
        for _ in 0..4 {
            let mut nonce = [0u8; 16];
            OsRng
                .try_fill_bytes(&mut nonce)
                .map_err(|_| ResumeError::StorageUnavailable)?;
            let nonce = nonce.iter().map(|b| format!("{b:02x}")).collect::<String>();
            let temp = root.join(format!(".youtube-resume-{nonce}.tmp"));
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            match options.open(&temp) {
                Ok(mut file) => {
                    struct Cleanup(PathBuf);
                    impl Drop for Cleanup {
                        fn drop(&mut self) {
                            let _ = std::fs::remove_file(&self.0);
                        }
                    }
                    let _cleanup = Cleanup(temp.clone());
                    regular(&file)?;
                    file.write_all(&bytes)
                        .and_then(|_| file.sync_all())
                        .map_err(|_| ResumeError::StorageUnavailable)?;
                    drop(file);
                    check_target(&path)?;
                    // Rust std uses replace-existing rename semantics on Windows. Never remove
                    // the old destination first or retry via a lossy delete/copy fallback.
                    std::fs::rename(&temp, &path).map_err(|_| ResumeError::StorageUnavailable)?;
                    #[cfg(unix)]
                    File::open(root)
                        .and_then(|f| f.sync_all())
                        .map_err(|_| ResumeError::StorageUnavailable)?;
                    return Ok(());
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    last = Some(ResumeError::StorageUnavailable)
                }
                Err(_) => return Err(ResumeError::StorageUnavailable),
            }
        }
        Err(last.unwrap_or(ResumeError::StorageUnavailable))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn scope() -> ResumeScope {
        ResumeScope::new("superA", &"a".repeat(64)).unwrap()
    }
    fn id(n: u32) -> VideoId {
        VideoId::parse(&format!("video{n:06}")).unwrap()
    }
    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            let mut random = [0u8; 16];
            OsRng.fill_bytes(&mut random);
            let path = std::env::temp_dir().join(format!(
                "nexusbro-resume-{}-{:x}",
                std::process::id(),
                u128::from_le_bytes(random)
            ));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    #[test]
    fn ram_only_never_creates_a_profile_or_file() {
        let mut book = ResumeBook::load(scope(), None).unwrap();
        assert!(book.is_ram_only());
        book.record(&id(1), 20.5, Some(100.0), 1).unwrap();
        book.save().unwrap();
        assert_eq!(book.position(&id(1)).unwrap().position_seconds, 20.5);
        assert!(book.root.is_none());
    }
    #[test]
    fn actual_file_roundtrip_is_exact_scope_and_replaces_existing_atomically() {
        let temp = Temp::new();
        let mut book = ResumeBook::load(scope(), Some(&temp.0)).unwrap();
        book.record(&id(1), 20.5, Some(100.0), 123).unwrap();
        book.save().unwrap();
        book.record(&id(1), 30.0, None, 124).unwrap();
        book.save().unwrap();
        let mut restored = ResumeBook::load(scope(), Some(&temp.0)).unwrap();
        let point = restored.position(&id(1)).unwrap();
        assert_eq!(point.position_seconds, 30.0);
        assert_eq!(point.updated_epoch_seconds, 124);
        assert_eq!(std::fs::read_dir(&temp.0).unwrap().count(), 1);
        assert!(ResumeBook::load(
            ResumeScope::new("other", &"a".repeat(64)).unwrap(),
            Some(&temp.0)
        )
        .is_err());
        assert!(ResumeBook::load(
            ResumeScope::new("superA", &"b".repeat(64)).unwrap(),
            Some(&temp.0)
        )
        .is_err());
    }
    #[test]
    fn finite_bounds_and_near_end_remove_without_mutating_on_invalid_input() {
        let mut book = ResumeBook::new(scope());
        book.record(&id(1), 10.0, Some(100.0), 1).unwrap();
        for (position, duration) in [
            (f64::NAN, None),
            (f64::INFINITY, None),
            (-1.0, None),
            (MAX_SECONDS + 1.0, None),
            (100.0, Some(99.0)),
            (1.0, Some(f64::NAN)),
            (1.0, Some(0.0)),
        ] {
            assert_eq!(
                book.record(&id(1), position, duration, 2),
                Err(ResumeError::InvalidPosition)
            );
        }
        assert_eq!(book.position(&id(1)).unwrap().position_seconds, 10.0);
        book.record(&id(1), 95.0, Some(100.0), 3).unwrap();
        assert!(book.is_empty());
        book.record(&id(1), 10.0, None, 4).unwrap();
        book.record(&id(1), 0.0, None, 5).unwrap();
        assert!(book.is_empty());
    }
    #[test]
    fn bounded_lru_survives_save_order_and_counter_wrap() {
        let mut book = ResumeBook::new(scope());
        for n in 0..MAX_ENTRIES as u32 {
            book.record(&id(n), 10.0, None, n as u64).unwrap();
        }
        book.position(&id(0));
        book.sequence = u64::MAX;
        book.record(&id(999), 10.0, None, 999).unwrap();
        assert_eq!(book.len(), MAX_ENTRIES);
        assert!(book.position(&id(1)).is_none());
        assert!(book.position(&id(0)).is_some());
        assert!(book.encoded().unwrap().len() <= MAX_JSON_BYTES);
    }
    #[test]
    fn malformed_tampered_unknown_fields_duplicate_ids_and_future_schema_refused() {
        let temp = Temp::new();
        let mut book = ResumeBook::load(scope(), Some(&temp.0)).unwrap();
        book.record(&id(1), 10.0, None, 1).unwrap();
        let original: serde_json::Value = serde_json::from_slice(&book.encoded().unwrap()).unwrap();
        for mutation in 0..5 {
            let mut doc = original.clone();
            match mutation {
                0 => doc["schema"] = 2.into(),
                1 => doc["unexpected"] = true.into(),
                2 => doc["entries"][0]["position_seconds"] = (-1).into(),
                3 => doc["entries"][0]["video"] = "../../bad".into(),
                _ => {
                    let point = doc["entries"][0].clone();
                    doc["entries"].as_array_mut().unwrap().push(point);
                }
            }
            std::fs::write(temp.0.join(FILE_NAME), serde_json::to_vec(&doc).unwrap()).unwrap();
            assert!(ResumeBook::load(scope(), Some(&temp.0)).is_err());
        }
        std::fs::write(temp.0.join(FILE_NAME), b"{broken").unwrap();
        assert!(ResumeBook::load(scope(), Some(&temp.0)).is_err());
        std::fs::write(temp.0.join(FILE_NAME), vec![b' '; MAX_JSON_BYTES + 1]).unwrap();
        assert!(matches!(
            ResumeBook::load(scope(), Some(&temp.0)),
            Err(ResumeError::Oversized)
        ));
    }
    #[cfg(unix)]
    #[test]
    fn symlink_target_is_never_read_or_overwritten() {
        use std::os::unix::{fs::symlink, fs::PermissionsExt};
        let temp = Temp::new();
        let other = temp.0.join("sentinel");
        std::fs::write(&other, b"original").unwrap();
        let book = ResumeBook::load(scope(), Some(&temp.0)).unwrap();
        symlink(&other, temp.0.join(FILE_NAME)).unwrap();
        assert!(matches!(
            ResumeBook::load(scope(), Some(&temp.0)),
            Err(ResumeError::UnsafeFile)
        ));
        assert_eq!(book.save(), Err(ResumeError::UnsafeFile));
        assert_eq!(std::fs::read(&other).unwrap(), b"original");
        std::fs::remove_file(temp.0.join(FILE_NAME)).unwrap();
        book.save().unwrap();
        assert_eq!(
            std::fs::metadata(temp.0.join(FILE_NAME))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    #[test]
    fn nonregular_target_is_not_deleted_on_failed_save() {
        let temp = Temp::new();
        let book = ResumeBook::load(scope(), Some(&temp.0)).unwrap();
        std::fs::create_dir(temp.0.join(FILE_NAME)).unwrap();
        std::fs::write(temp.0.join(FILE_NAME).join("sentinel"), b"original").unwrap();
        assert_eq!(book.save(), Err(ResumeError::UnsafeFile));
        assert_eq!(
            std::fs::read(temp.0.join(FILE_NAME).join("sentinel")).unwrap(),
            b"original"
        );
    }
    #[cfg(windows)]
    #[test]
    fn windows_locked_old_file_is_never_deleted_to_retry_replacement() {
        use std::os::windows::fs::OpenOptionsExt;
        let temp = Temp::new();
        let mut book = ResumeBook::load(scope(), Some(&temp.0)).unwrap();
        book.record(&id(1), 10.0, None, 1).unwrap();
        book.save().unwrap();
        let old = std::fs::read(temp.0.join(FILE_NAME)).unwrap();
        let locked = OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(temp.0.join(FILE_NAME))
            .unwrap();
        book.record(&id(1), 20.0, None, 2).unwrap();
        assert!(book.save().is_err());
        drop(locked);
        assert_eq!(std::fs::read(temp.0.join(FILE_NAME)).unwrap(), old);
        assert_eq!(std::fs::read_dir(&temp.0).unwrap().count(), 1);
    }
}
