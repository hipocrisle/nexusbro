//! Read-only media diagnostics: CI or explicit isolated Linux fixture opt-in.
//! This does not enable arbitrary script evaluation or relax document guards.
pub fn allowed() -> bool {
    if std::env::var("GITHUB_ACTIONS").as_deref() == Ok("true") {
        return std::env::var_os("NEXUSBRO_TEST_DATA").is_some();
    }
    local_allowed()
}

#[cfg(target_os = "linux")]
fn local_allowed() -> bool {
    use std::os::unix::fs::MetadataExt;
    let Some(root) = std::env::var_os("NEXUSBRO_TEST_DATA").map(std::path::PathBuf::from) else { return false; };
    let Some(home) = dirs::home_dir() else { return false; };
    let Ok(uid) = std::fs::metadata("/proc/self").map(|m| m.uid()) else { return false; };
    local_root_allowed(&root, &home, uid)
}

#[cfg(target_os = "linux")]
fn local_root_allowed(root: &std::path::Path, home: &std::path::Path, uid: u32) -> bool {
    use std::os::unix::fs::MetadataExt;
    if uid == 0 || !root.is_absolute() { return false; }
    let Ok(canonical) = root.canonicalize() else { return false; };
    let Ok(ai) = home.join("AI").canonicalize() else { return false; };
    if canonical != root || canonical == ai || !canonical.starts_with(&ai) { return false; }
    // Every directory from the fixture root through AI belongs to the caller
    // and cannot be changed by another local user. Symlink roots are rejected.
    let mut current = Some(canonical.as_path());
    while let Some(path) = current {
        let Ok(meta) = std::fs::symlink_metadata(path) else { return false; };
        if !meta.is_dir() || meta.uid() != uid || meta.mode() & 0o022 != 0 { return false; }
        if path == ai { break; }
        current = path.parent();
        if current.is_none() { return false; }
    }
    let Ok(meta) = std::fs::metadata(root) else { return false; };
    if meta.mode() & 0o777 != 0o700 { return false; }
    let opt_in = root.join(".youtube-benchmark-opt-in");
    let Ok(meta) = std::fs::symlink_metadata(&opt_in) else { return false; };
    if !meta.is_file() || meta.uid() != uid || meta.mode() & 0o777 != 0o600 || meta.len() != b"nexusbro-local-youtube-benchmark-v1\n".len() as u64 { return false; }
    std::fs::read(opt_in).is_ok_and(|v| v == b"nexusbro-local-youtube-benchmark-v1\n")
}

#[cfg(not(target_os = "linux"))]
fn local_allowed() -> bool { false }

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use std::{fs, os::unix::fs::{MetadataExt, PermissionsExt, symlink}};
    #[test]
    fn only_private_explicit_owned_nonroot_fixture_is_admitted() {
        let home = std::env::temp_dir().join(format!("nb-benchmark-gate-{}", std::process::id()));
        let root = home.join("AI/fixture");
        fs::create_dir_all(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        fs::set_permissions(home.join("AI"), fs::Permissions::from_mode(0o700)).unwrap();
        // The validator is pure over filesystem ownership; root is forbidden
        // in production. A root-run test cannot create nonroot-owned fixtures.
        let uid = fs::metadata(&root).unwrap().uid();
        assert!(!local_root_allowed(&root, &home, 0));
        assert!(!local_root_allowed(&root, &home, uid));
        if uid != 0 {
            let opt_in = root.join(".youtube-benchmark-opt-in");
            fs::write(&opt_in, b"nexusbro-local-youtube-benchmark-v1\n").unwrap();
            fs::set_permissions(&opt_in, fs::Permissions::from_mode(0o600)).unwrap();
            assert!(local_root_allowed(&root, &home, uid));
            fs::set_permissions(&root, fs::Permissions::from_mode(0o755)).unwrap();
            assert!(!local_root_allowed(&root, &home, uid));
            fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
            fs::set_permissions(&opt_in, fs::Permissions::from_mode(0o644)).unwrap();
            assert!(!local_root_allowed(&root, &home, uid));
            fs::set_permissions(&opt_in, fs::Permissions::from_mode(0o600)).unwrap();
            fs::write(&opt_in, b"unconfirmed").unwrap();
            assert!(!local_root_allowed(&root, &home, uid));
            fs::remove_file(&opt_in).unwrap();
            let outside = home.join("outside");
            fs::write(&outside, b"nexusbro-local-youtube-benchmark-v1\n").unwrap();
            fs::set_permissions(&outside, fs::Permissions::from_mode(0o600)).unwrap();
            symlink(&outside, &opt_in).unwrap();
            assert!(!local_root_allowed(&root, &home, uid));
        }
        fs::remove_dir_all(home).unwrap();
    }
}
