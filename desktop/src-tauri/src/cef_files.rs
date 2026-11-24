//! Locate complete CEF resources in explicit or application-owned locations.
//! Discovery only: package integrity and native loading are verified separately.
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};

#[cfg(target_os = "windows")]
const LIBRARY: &str = "libcef.dll";
#[cfg(not(target_os = "windows"))]
const LIBRARY: &str = "libcef.so";
const ARG: &str = "--nb-cef-resources";
const MAX_CANDIDATES: usize = 6;

/// A malformed or incomplete explicit parent hint fails closed. There is no
/// PATH lookup, working-directory lookup, filesystem traversal, or env mutation.
pub fn directory() -> Option<PathBuf> {
    resolve(
        std::env::args_os(),
        std::env::var_os("CEF_PATH"),
        crate::brauzer::papka_resursov(),
        std::env::current_exe().ok(),
    )
}

fn explicit_hint(args: impl IntoIterator<Item = OsString>) -> Result<Option<PathBuf>, ()> {
    let mut explicit = None;
    for arg in args {
        let text = arg.to_string_lossy();
        if text == ARG {
            return Err(());
        } // Only one --name=value argument.
        let Some(value) = text.strip_prefix("--nb-cef-resources=") else {
            continue;
        };
        if explicit.is_some() || arg.to_str().is_none() {
            return Err(());
        }
        let path = PathBuf::from(value);
        if !path.is_absolute() {
            return Err(());
        }
        explicit = Some(path);
    }
    Ok(explicit)
}

fn complete(path: &Path) -> bool {
    path.is_absolute()
        && [LIBRARY, "icudtl.dat"].iter().all(|name| {
            path.join(name)
                .metadata()
                .is_ok_and(|m| m.is_file() && m.len() > 0)
        })
        && path.join("locales").is_dir()
}

fn resolve(
    args: impl IntoIterator<Item = OsString>,
    legacy: Option<OsString>,
    resources: Option<PathBuf>,
    executable: Option<PathBuf>,
) -> Option<PathBuf> {
    if let Some(explicit) = explicit_hint(args).ok()? {
        return complete(&explicit).then_some(explicit);
    }
    let mut candidates = Vec::with_capacity(MAX_CANDIDATES);
    let mut add = |path: PathBuf| {
        if path.is_absolute() && candidates.len() < MAX_CANDIDATES && !candidates.contains(&path) {
            candidates.push(path);
        }
    };
    if let Some(path) = legacy {
        add(path.into());
    }
    if let Some(path) = resources {
        add(path);
    }
    if let Some(path) = executable.filter(|p| p.is_absolute()) {
        if let Some(parent) = path.parent() {
            add(parent.to_path_buf());
            #[cfg(target_os = "linux")]
            add(parent.join("../lib/NexusBro"));
        }
    }
    candidates.into_iter().find(|path| complete(path))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        sync::atomic::{AtomicU64, Ordering},
        time::{SystemTime, UNIX_EPOCH},
    };
    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "nb-cef-files-tests-{}-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
        fn assets(&self, name: &str) -> PathBuf {
            let path = self.0.join(name);
            fs::create_dir_all(path.join("locales")).unwrap();
            // Presence-test markers only. These files are never loaded/executed.
            fs::write(path.join(LIBRARY), b"fixture library marker").unwrap();
            fs::write(path.join("icudtl.dat"), b"fixture ICU marker").unwrap();
            path
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn argument(path: &Path) -> OsString {
        format!("{ARG}={}", path.display()).into()
    }

    #[test]
    fn complete_requires_nonempty_platform_library_icu_and_locales() {
        let temp = Temp::new();
        let path = temp.assets("assets");
        assert!(complete(&path));
        fs::write(path.join(LIBRARY), b"").unwrap();
        assert!(!complete(&path));
        fs::write(path.join(LIBRARY), b"fixture").unwrap();
        fs::remove_file(path.join("icudtl.dat")).unwrap();
        assert!(!complete(&path));
        fs::write(path.join("icudtl.dat"), b"fixture").unwrap();
        fs::remove_dir(path.join("locales")).unwrap();
        assert!(!complete(&path));
    }
    #[test]
    fn explicit_complete_hint_wins_and_missing_hint_never_falls_back() {
        let temp = Temp::new();
        let explicit = temp.assets("explicit");
        let fallback = temp.assets("fallback");
        assert_eq!(
            resolve(
                [argument(&explicit)],
                Some(fallback.clone().into_os_string()),
                Some(fallback.clone()),
                None
            ),
            Some(explicit)
        );
        assert!(resolve(
            [argument(&temp.0.join("missing"))],
            Some(fallback.clone().into_os_string()),
            Some(fallback),
            None
        )
        .is_none());
    }
    #[test]
    fn duplicate_relative_empty_and_split_hints_fail_closed() {
        let temp = Temp::new();
        let valid = temp.assets("valid");
        for args in [
            vec![argument(&valid), argument(&valid)],
            vec![format!("{ARG}=relative").into()],
            vec![format!("{ARG}=").into()],
            vec![ARG.into(), valid.clone().into_os_string()],
        ] {
            assert!(resolve(args, None, Some(valid.clone()), None).is_none());
        }
    }
    #[test]
    fn legacy_then_tauri_then_executable_priority_is_bounded() {
        let temp = Temp::new();
        let legacy = temp.assets("legacy");
        let resources = temp.assets("resources");
        let adjacent = temp.assets("bin");
        let exe = adjacent.join("nexusbro");
        assert_eq!(
            resolve(
                Vec::new(),
                Some(legacy.clone().into_os_string()),
                Some(resources.clone()),
                Some(exe.clone())
            ),
            Some(legacy)
        );
        assert_eq!(
            resolve(
                Vec::new(),
                Some(OsString::from("relative-dev-path")),
                Some(resources.clone()),
                Some(exe.clone())
            ),
            Some(resources)
        );
        assert_eq!(
            resolve(Vec::new(), None, Some(temp.0.join("missing")), Some(exe)),
            Some(adjacent)
        );
    }
    #[test]
    fn relative_executable_and_resource_candidates_are_ignored() {
        assert!(resolve(
            Vec::new(),
            Some("relative".into()),
            Some("relative".into()),
            Some("bin/nexusbro".into())
        )
        .is_none());
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn installed_linux_sibling_is_case_correct_without_environment() {
        let temp = Temp::new();
        let resources = temp.assets("usr/lib/NexusBro");
        let bin = temp.0.join("usr/bin");
        fs::create_dir_all(&bin).unwrap();
        let actual = resolve(Vec::new(), None, None, Some(bin.join("nexusbro"))).unwrap();
        assert_eq!(
            actual.canonicalize().unwrap(),
            resources.canonicalize().unwrap()
        );
    }
    #[cfg(unix)]
    #[test]
    fn non_unicode_explicit_hint_is_rejected_without_fallback() {
        use std::os::unix::ffi::OsStringExt;
        let temp = Temp::new();
        let valid = temp.assets("valid");
        let mut bytes = b"--nb-cef-resources=/".to_vec();
        bytes.push(0xff);
        assert!(resolve([OsString::from_vec(bytes)], None, Some(valid), None).is_none());
    }
}
