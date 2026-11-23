//! Isolated disk recovery verification: real session/store code, no CEF/Tauri.
#[path = "../../../desktop/src-tauri/src/tab_routes.rs"]
pub mod tab_routes;
#[path = "../../../desktop/src-tauri/src/sessiya.rs"]
pub mod sessiya;
#[path = "../../../desktop/src-tauri/src/window_recovery.rs"]
pub mod window_recovery;
// These two unrelated command-payload dependencies are not used by recovery.
pub mod password_cef {
    #[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
    pub struct Secret(String);
}
pub mod privatnost {
    pub fn eto_id(id: &str) -> bool { id.starts_with("pvt-") }
}

#[cfg(test)]
mod disk_tests {
    use super::*;
    use std::path::PathBuf;
    struct Temporary(PathBuf);
    impl Temporary {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!("nexusbro-recovery-{}-{}", std::process::id(),
                std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
            std::fs::create_dir_all(&root).unwrap(); Self(root)
        }
    }
    impl Drop for Temporary { fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); } }
    fn window(url: &str) -> window_recovery::WindowSession {
        window_recovery::WindowSession { key: "browser-1".into(), active_super: "normal".into(), privatnaya: false,
            views: vec![window_recovery::SuperSession { super_id: "normal".into(), session: sessiya::Sessiya::novaya(url) }] }
    }
    #[test]
    fn disk_window_corruption_uses_committed_backup_not_temp() {
        let p = Temporary::new();
        window_recovery::save(&p.0, vec![window("https://first.test")]).unwrap();
        window_recovery::save(&p.0, vec![window("https://second.test")]).unwrap();
        std::fs::write(p.0.join("windows-recovery.json"), b"{broken").unwrap();
        let loaded = window_recovery::load(&p.0);
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].views[0].session.vkladki[0].adres, "https://first.test");
        assert!(loaded[0].views[0].session.vkladki[0].otlozhena);
        window_recovery::save(&p.0, loaded).unwrap();
        assert_eq!(window_recovery::load(&p.0)[0].views[0].session.vkladki[0].adres, "https://first.test");
    }
    #[test]
    fn explicit_window_close_commits_empty_instead_of_reviving_backup() {
        let p = Temporary::new();
        window_recovery::save(&p.0, vec![window("https://first.test")]).unwrap();
        window_recovery::save(&p.0, Vec::new()).unwrap();
        assert!(window_recovery::load(&p.0).is_empty());
    }
    #[test]
    fn failed_atomic_publication_preserves_existing_target() {
        let p = Temporary::new();
        let target = p.0.join("target");
        std::fs::create_dir(&target).unwrap();
        std::fs::write(target.join("sentinel"), b"original").unwrap();
        assert!(sessiya::durable_replace(&target, b"new").is_err());
        assert_eq!(std::fs::read(target.join("sentinel")).unwrap(), b"original");
        assert_eq!(std::fs::read_dir(&p.0).unwrap().count(), 1, "failed temp is cleaned up");
    }
    #[test]
    fn initiating_selection_remaps_only_unchanged_route_and_url() {
        let before = sessiya::Sessiya::novaya("https://same.test");
        let mut after = before.clone();
        after.vkladki[0].id = 1_000_000;
        match window_recovery::remap_command(&before, &after, sessiya::Komanda::Vybrat { id: 1 }).unwrap() {
            sessiya::Komanda::Vybrat { id } => assert_eq!(id, 1_000_000),
            _ => panic!("wrong command"),
        }
        after.vkladki[0].route = tab_routes::RouteChoice::Direct;
        assert!(window_recovery::remap_command(&before, &after, sessiya::Komanda::Vybrat { id: 1 }).is_err());
        after.vkladki[0].route = before.vkladki[0].route.clone();
        after.vkladki[0].adres = "https://different.test".into();
        assert!(window_recovery::remap_command(&before, &after, sessiya::Komanda::Vybrat { id: 1 }).is_err());
    }
    #[test]
    fn recovery_refuses_stale_internal_engine_receipt() {
        let before = sessiya::Sessiya::novaya("https://same.test");
        let mut after = before.clone(); after.vkladki[0].id = 1_000_000;
        assert!(window_recovery::remap_command(&before, &after, sessiya::Komanda::RouteFailed {
            id: 1, generation: 7, expected_browser: 99, error: "stale".into(),
        }).is_err());
    }

    #[test]
    fn renderer_mount_and_maintenance_do_not_wake_recovered_windows() {
        use sessiya::Komanda;
        for command in [Komanda::FonKonteynera { rgb: 0x123456 }, Komanda::ObnovitInterfeys,
            Komanda::MemorySettings { enabled: true, minutes: 30 }, Komanda::BrowserLocked { locked: false, generation: 1 }] {
            assert!(!window_recovery::user_command_can_wake(&command));
        }
        assert!(window_recovery::user_command_can_wake(&Komanda::Vybrat { id: 1 }));
    }

}
