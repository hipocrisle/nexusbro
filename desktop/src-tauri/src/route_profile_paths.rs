//! CEF156 Chrome profiles must be immediate children of root_cache_path.
//! A contained nested directory silently becomes OffTheRecord in CEF.
use std::{
    fs, io,
    path::{Path, PathBuf},
};

pub const MAX_OWNED_PROFILES: usize = 128;
pub const MAX_ROOT_ENTRIES: usize = 1024;
const PREFIX: &str = "route-";
fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
pub fn valid_key(key: &str) -> bool {
    key.len() == 64
        && key
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn key_from_leaf(leaf: &str) -> Option<&str> {
    leaf.strip_prefix(PREFIX).filter(|key| valid_key(key))
}
pub fn disk_path(root: &Path, key: &str, legacy_key: &str) -> io::Result<PathBuf> {
    if !valid_key(key) || !valid_key(legacy_key) || !root.is_absolute() {
        return Err(invalid("Invalid owned profile identity"));
    }
    Ok(root.join(if key == legacy_key {
        "Default".into()
    } else {
        format!("{PREFIX}{key}")
    }))
}
/// Reject nested paths and aliases even when the alias stays inside the Super.
/// Keep the original path spelling for CEF; canonical paths are for validation.
pub fn validate_existing(root: &Path, path: &Path) -> io::Result<()> {
    if !root.is_absolute() || path.parent() != Some(root) {
        return Err(invalid("Profile must be a direct child of the Super root"));
    }
    let root_canonical = root.canonicalize()?;
    let profile_canonical = path.canonicalize()?;
    let leaf = path
        .file_name()
        .ok_or_else(|| invalid("Invalid profile leaf"))?;
    if profile_canonical.parent() != Some(root_canonical.as_path())
        || profile_canonical != root_canonical.join(leaf)
    {
        return Err(invalid("Profile directory is redirected"));
    }
    if !fs::metadata(path)?.is_dir() {
        return Err(invalid("Profile is not a directory"));
    }
    Ok(())
}
pub fn create_profile(root: &Path, key: &str, legacy_key: &str) -> io::Result<PathBuf> {
    let path = disk_path(root, key, legacy_key)?;
    // Single-level creation cannot create a nested cache that CEF would make
    // ephemeral. Existing junctions/symlinks are validated before CEF sees them.
    match fs::create_dir(&path) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error),
    }
    validate_existing(root, &path)?;
    Ok(path)
}
/// Enumerate only owned direct-child profiles. Unmatched directories are kept.
/// Validate the complete list before returning any contexts for stats/clear.
pub fn owned_profiles(root: &Path, legacy_key: &str) -> io::Result<Vec<(String, PathBuf)>> {
    if !valid_key(legacy_key) || !root.is_absolute() {
        return Err(invalid("Invalid owned profile root"));
    }
    let mut profiles = Vec::new();
    for (index, entry) in fs::read_dir(root)?.enumerate() {
        if index >= MAX_ROOT_ENTRIES {
            return Err(invalid("Too many root directory entries"));
        }
        let entry = entry?;
        let name = entry.file_name();
        let Some(leaf) = name.to_str() else {
            continue;
        };
        let key = if leaf == "Default" {
            legacy_key
        } else if let Some(key) = key_from_leaf(leaf) {
            if key == legacy_key {
                return Err(invalid("Duplicate legacy profile binding"));
            }
            key
        } else {
            continue;
        };
        let path = entry.path();
        validate_existing(root, &path)?;
        if profiles.len() >= MAX_OWNED_PROFILES {
            return Err(invalid("Too many owned historical profiles"));
        }
        profiles.push((key.into(), path));
    }
    profiles.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(profiles)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    struct OwnedTemp(PathBuf);
    impl OwnedTemp {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "nb-route-paths-{}-{stamp}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for OwnedTemp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn legacy() -> String {
        "0".repeat(64)
    }
    fn other() -> String {
        "a".repeat(64)
    }
    #[test]
    fn disk_layout_keeps_default_and_all_overrides_immediate_children() {
        let temp = OwnedTemp::new();
        let root = &temp.0;
        let default = create_profile(root, &legacy(), &legacy()).unwrap();
        let route = create_profile(root, &other(), &legacy()).unwrap();
        assert_eq!(default, root.join("Default"));
        assert_eq!(route, root.join(format!("route-{}", other())));
        assert_eq!(route.parent(), Some(root.as_path()));
        assert!(!root.join("routes").exists());
        assert_eq!(owned_profiles(root, &legacy()).unwrap().len(), 2);
    }
    #[test]
    fn nested_and_malformed_identities_are_never_accepted() {
        let temp = OwnedTemp::new();
        let root = &temp.0;
        let nested = root.join("routes").join(other());
        fs::create_dir_all(&nested).unwrap();
        assert!(validate_existing(root, &nested).is_err());
        for key in [
            "../Default".into(),
            "A".repeat(64),
            "a".repeat(63),
            format!("{}x", other()),
        ] {
            assert!(disk_path(root, &key, &legacy()).is_err());
        }
        assert!(disk_path(Path::new("relative"), &other(), &legacy()).is_err());
    }
    #[test]
    fn scan_preserves_unmatched_and_old_unreleased_nested_directories() {
        let temp = OwnedTemp::new();
        let root = &temp.0;
        fs::create_dir_all(root.join("routes").join(other())).unwrap();
        fs::create_dir(root.join("route-runtime")).unwrap();
        fs::create_dir(root.join("route-not-a-key")).unwrap();
        fs::write(root.join("route-legacy.json"), legacy()).unwrap();
        let path = create_profile(root, &other(), &legacy()).unwrap();
        let profiles = owned_profiles(root, &legacy()).unwrap();
        assert_eq!(profiles, vec![(other(), path)]);
        assert!(root.join("routes").join(other()).is_dir());
        assert!(root.join("route-runtime").is_dir());
    }
    #[test]
    fn all_owned_limits_fail_without_returning_a_partial_list() {
        let temp = OwnedTemp::new();
        let root = &temp.0;
        for n in 1..=MAX_OWNED_PROFILES + 1 {
            fs::create_dir(root.join(format!("route-{n:064x}"))).unwrap();
        }
        assert!(owned_profiles(root, &legacy()).is_err());
    }
    #[test]
    fn unrelated_root_entry_limit_is_bounded_too() {
        let temp = OwnedTemp::new();
        for n in 0..=MAX_ROOT_ENTRIES {
            fs::write(temp.0.join(format!("unmatched-{n}")), b"").unwrap();
        }
        assert!(owned_profiles(&temp.0, &legacy()).is_err());
    }
    #[cfg(unix)]
    #[test]
    fn symlink_escape_and_same_root_profile_alias_are_rejected_without_deleting_them() {
        use std::os::unix::fs::symlink;
        let root = OwnedTemp::new();
        let outside = OwnedTemp::new();
        let alias = root.0.join(format!("route-{}", other()));
        symlink(&outside.0, &alias).unwrap();
        assert!(create_profile(&root.0, &other(), &legacy()).is_err());
        assert!(owned_profiles(&root.0, &legacy()).is_err());
        assert_eq!(fs::read_dir(&outside.0).unwrap().count(), 0);
        fs::remove_file(&alias).unwrap();
        let original = create_profile(&root.0, &"b".repeat(64), &legacy()).unwrap();
        symlink(&original, &alias).unwrap();
        assert!(owned_profiles(&root.0, &legacy()).is_err());
        assert!(fs::symlink_metadata(alias)
            .unwrap()
            .file_type()
            .is_symlink());
    }
}
