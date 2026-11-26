use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Mutex;

pub static ZAPIS: Mutex<()> = Mutex::new(());

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Obychnaya {
    pub id: u64,
    pub adres: String,
    pub zagolovok: String,
    #[serde(default)]
    pub zakreplena: bool,
    #[serde(default)]
    pub zagruzka: bool,
    #[serde(default)]
    pub otlozhena: bool,
    #[serde(default)]
    pub nazad: bool,
    #[serde(default)]
    pub ikonka: Option<String>,
    #[serde(default)]
    pub vpered: bool,
    #[serde(default)]
    pub privatnaya: bool,
    #[serde(default)]
    pub memory_keep:bool,
    #[serde(default)]
    pub memory_pending:bool,
    /// Local intent only; absent legacy state inherits the Super route.
    #[serde(default)]
    pub route: crate::tab_routes::RouteChoice,
    #[serde(default)]
    pub route_generation: u64,
    /// Runtime state is never restored as an applied route.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route_error: Option<String>,
    #[serde(default)]
    pub route_pending: bool,
    #[serde(default)]
    pub browser_id: i32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Sessiya {
    pub vkladki: Vec<Obychnaya>,
    pub aktivnaya: u64,
    pub sleduyushchaya: u64,
    #[serde(default)]
    pub zakrytye: Vec<Obychnaya>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "deystvie", rename_all = "snake_case")]
pub enum Komanda {
    AppShutdown { request:crate::app_shutdown_protocol::Request },
    OknoNovoe { label:String, parent:u64, x:i32, y:i32, w:u32, h:u32, privatnaya:bool, #[serde(default)] sessiya:Option<Sessiya> },
    OknoKomanda { label:String, komanda:Box<Komanda> },
    OknoZakryt { label:String },
    ObnovitInterfeys,
    #[serde(rename="youtube_light")]
    YoutubeLight { id:u64 },
    #[serde(rename="youtube_snapshot")]
    YoutubeSnapshot { id:u64 },
    YoutubeReferenceSnapshot { id:u64, video_id:String },
    YoutubeDecodeSnapshot { id:u64, video_id:String },
    #[serde(rename="youtube_fixture")]
    YoutubeFixture { id:u64 },
    YoutubeResumeFixture { id:u64 },
    YoutubeDownloadTarget {id:u64,request:String,path:Option<String>},
    SohranitStranitsu { id:u64 },
    PageSaveTarget { id:u64, request:String, path:Option<String> },
    YoutubeExitFullscreen,
    HibernatePrepare { request:crate::super_hibernation_protocol::Prepare },
    HibernateCommit { token:crate::super_hibernation_protocol::Token },
    HibernateAbort { token:crate::super_hibernation_protocol::Token },
    HibernateFinalAck { token:crate::super_hibernation_protocol::Token },
    GlavnoeSkryto,
    GlavnoePokazat,
    RouteApply {
        id: u64,
        route: crate::tab_routes::RouteChoice,
        generation: u64,
        prepared: crate::tab_routes::NativeRoute,
        expected_browser: i32,
    },
    RouteFailed { id: u64, generation: u64, expected_browser: i32, error: String },
    RoutePrepareSnapshot { snapshot: crate::tab_routes::RouteBootstrap, generation: u64 },
    Novaya { adres: String },
    NovayaPrivatnaya { adres:String },
    OpenLink { opener_id:u64, adres:String, privatnaya:bool, background:bool },
    PageContextChoose { token:String, command_id:Option<i32> },
    VosstanovitZakrytuyu,
    ZagruzkaUpravlenie{id:u32,action:String},
    Skachat{adres:String,privatnaya:bool},
    Vybrat { id: u64 },
    Zakryt { id: u64 },
    /// Explicit reload-based unload; never scheduled automatically.
    Usypit { id: u64 },
    MemoryAuto { id:u64 },
    MemorySettings { enabled:bool,minutes:u32 },
    MemoryKeep { id:u64,keep:bool },
    ZakrytDrugie { id: u64, sprava: bool },
    Zakrepit { id: u64, zakreplena: bool },
    Pereyti { id: u64, adres: String },
    Nazad { id: u64 },
    Vpered { id: u64 },
    Obnovit { id: u64 },
    /// F5 / Ctrl+F5 — перезагрузка без кеша.
    ObnovitBezKesha { id: u64 },
    Instrumenty { id: u64 },
    InstrumentyRazmer { procent: u32 },
    Nayti { id: u64, text: String, vpered: bool, dalshe: bool },
    PoiskZakryt { id: u64 },
    Perevesti { id:u64 },
    PerevodOriginal { id:u64 },
    PerevodAvto { id:u64, enabled:bool },
    FonKonteynera { rgb:u32 },
    PasswordForm { id:u64,token:String,origin:String,payload:Option<crate::password_cef::Secret> },
    PasswordsEnabled { enabled:bool },
    BrowserLocked { locked:bool, generation:u64 },
    DannyeStatistika { token:String },
    OchistitDannye { token:String,cookies:bool,cache:bool },
    PerevodOtvet { id:u64, token:String, items:Vec<(u32,String)>, done:u32, total:u32, error:Option<String> },
    Issledovat { id: u64, x: i32, y: i32 },
    Iskhodnik { id: u64 },
    Peremestit { id: u64, pered: Option<u64> },
    Sleduyushchaya { nazad: bool },
    Masshtab { procent: u32 },
}

impl Sessiya {
    pub fn novaya(adres: &str) -> Self {
        let mut s = Self {
            vkladki: Vec::new(),
            aktivnaya: 0,
            sleduyushchaya: 1,
            zakrytye: Vec::new(),
        };
        s.dobavit(adres);
        s
    }

    pub fn dobavit(&mut self, adres: &str) -> u64 {
        let id = self.sleduyushchaya;
        self.sleduyushchaya += 1;
        self.vkladki.push(Obychnaya {
            id,
            adres: adres.into(),
            zagolovok: "Новая вкладка".into(),
            zakreplena: false,
            zagruzka: adres!="about:blank",
            otlozhena: false,
            nazad: false,
            ikonka: None,
            vpered: false,
            privatnaya: false,
            memory_keep:false,
            memory_pending:false,
            route: crate::tab_routes::RouteChoice::Inherit,
            route_generation: 0,
            route_error: None,
            route_pending: false,
            browser_id: 0,
        });
        self.aktivnaya = id;
        id
    }

    pub fn peremestit(&mut self, id: u64, pered: Option<u64>) {
        if pered == Some(id) { return }
        let Some(from) = self.vkladki.iter().position(|v| v.id == id) else { return };
        let v = self.vkladki.remove(from);
        let to = pered.and_then(|n| self.vkladki.iter().position(|v| v.id == n)).unwrap_or(self.vkladki.len());
        self.vkladki.insert(to, v);
        self.vkladki.sort_by_key(|v| !v.zakreplena);
    }

    /// Strip transient/private data before any recovery store may serialize it.
    pub(crate) fn public_snapshot(&self) -> Self {
        let mut public = self.clone();
        public.vkladki.retain(|v| !v.privatnaya);
        public.zakrytye.retain(|v| !v.privatnaya);
        if public.vkladki.is_empty() {
            // An ordinary window containing only private pages must not revive
            // an older non-private session from the backup on the next start.
            public = Self::novaya("about:blank");
        }
        if !public.vkladki.iter().any(|v| v.id == public.aktivnaya) {
            public.aktivnaya = public.vkladki[0].id;
        }
        for v in public.vkladki.iter_mut().chain(public.zakrytye.iter_mut()) {
            v.memory_pending = false;
            v.route_pending = false;
            v.route_error = None;
            v.route_generation = 0;
            v.browser_id = 0;
            v.zagruzka = false;
            v.otlozhena = true;
            v.nazad = false;
            v.vpered = false;
        }
        public
    }

    pub(crate) fn validated_restore(self) -> Option<Self> {
        const MAX_ID: u64 = (1_u64 << 53) - 1;
        if self.vkladki.len() > 10_000 || self.zakrytye.len() > 10_000 { return None; }
        let mut s = self.public_snapshot();
        let ids: std::collections::HashSet<_> = s.vkladki.iter().map(|v| v.id).collect();
        if ids.len() != s.vkladki.len()
            || s.vkladki.iter().chain(s.zakrytye.iter()).any(|v| v.id == 0 || v.id >= MAX_ID - 1)
        { return None; }
        s.sleduyushchaya = s.vkladki.iter().chain(s.zakrytye.iter()).map(|v| v.id).max()? + 1;
        s.vkladki.sort_by_key(|v| !v.zakreplena);
        Some(s)
    }

    pub fn zagruzit(profil: &Path) -> Self {
        if profil.file_name().and_then(|s| s.to_str()).is_some_and(crate::privatnost::eto_id) {
            return Self::novaya("about:blank");
        }
        let primary = profil.join("obychnye.json");
        if let Some(s) = read_session(&primary) { return s; }
        // Preserve the first malformed snapshot for local diagnosis; never use
        // an uncommitted .tmp file as recovery authority.
        if let Ok(bytes) = recovery_read(&primary) {
            let forensic = profil.join("obychnye.povrezhdena.json");
            // Unparseable fragments cannot be reliably stripped of private
            // URLs. Keep forensic copies only when privacy can be inspected.
            if !forensic.exists() {
                if let Ok(source) = serde_json::from_slice::<Self>(&bytes) {
                    let safe = if source.vkladki.iter().chain(source.zakrytye.iter()).any(|v|v.privatnaya) {
                        serde_json::to_vec_pretty(&source.public_snapshot()).ok()
                    } else { Some(bytes) };
                    if let Some(safe) = safe { let _ = durable_replace(&forensic, &safe); }
                }
            }
        }
        read_session(&profil.join("obychnye.backup.json"))
            .unwrap_or_else(|| Self::novaya("about:blank"))
    }

    pub fn sohranit(&self, profil: &Path) -> anyhow::Result<()> {
        if profil.file_name().and_then(|s| s.to_str()).is_some_and(crate::privatnost::eto_id) { return Ok(()); }
        let public = self.public_snapshot().validated_restore()
            .ok_or_else(|| anyhow::anyhow!("Invalid session snapshot"))?;
        let bytes = serde_json::to_vec_pretty(&public)?;
        if bytes.len() > 64 * 1024 * 1024 { anyhow::bail!("Recovery snapshot too large"); }
        let target = profil.join("obychnye.json");
        let old_bytes = recovery_read(&target).ok();
        // Identical events are common (loading/focus): no extra sync I/O.
        if old_bytes.as_deref() == Some(bytes.as_slice()) { return Ok(()); }
        std::fs::create_dir_all(profil)?;
        if let Some(previous) = old_bytes.as_deref()
            .and_then(|bytes| serde_json::from_slice::<Self>(bytes).ok())
            .and_then(Self::validated_restore) {
            // Backup only validated/sanitized content. A corrupt primary must
            // never replace the last recoverable snapshot.
            durable_replace(&profil.join("obychnye.backup.json"), &serde_json::to_vec_pretty(&previous)?)?;
        }
        durable_replace(&target, &bytes)?;
        Ok(())
    }

}

/// Bounded read shared by the session and additional-window recovery stores.
pub(crate) fn recovery_read(path: &Path) -> std::io::Result<Vec<u8>> {
    use std::io::Read;
    let mut bytes = Vec::new();
    std::fs::File::open(path)?.take(64 * 1024 * 1024 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > 64 * 1024 * 1024 {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "Recovery snapshot too large"));
    }
    Ok(bytes)
}
fn read_session(path: &Path) -> Option<Sessiya> {
    serde_json::from_slice::<Sessiya>(&recovery_read(path).ok()?).ok()?.validated_restore()
}

/// Flush a unique same-directory temporary file before replacing the committed
/// snapshot. A crash leaves either the old or new document, never half JSON.
pub(crate) fn durable_replace(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let parent = path.parent().ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "Missing parent"))?;
    std::fs::create_dir_all(parent)?;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)] {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let (temporary, mut file) = loop {
        let candidate = parent.join(format!(".recovery-{}-{}.tmp", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        match options.open(&candidate) {
            Ok(file) => break (candidate, file),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    };
    struct Cleanup(std::path::PathBuf);
    impl Drop for Cleanup { fn drop(&mut self) { let _ = std::fs::remove_file(&self.0); } }
    let _cleanup = Cleanup(temporary.clone());
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(&temporary, path)?;
    #[cfg(unix)] std::fs::File::open(parent)?.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Papka(std::path::PathBuf);
    impl Papka {
        fn novaya() -> Self {
            let p = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("target/session-tests")
                .join(format!(
                    "{}-{}",
                    std::process::id(),
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_nanos()
                ));
            std::fs::create_dir_all(&p).unwrap();
            Self(p)
        }
    }
    impl Drop for Papka {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn vosstanovlenie_starogo_formata_i_schetchika() {
        let p = Papka::novaya();
        std::fs::write(p.0.join("obychnye.json"),r#"{"vkladki":[{"id":11,"adres":"about:blank","zagolovok":"One","zagruzka":true},{"id":22,"adres":"about:blank","zagolovok":"Two","zakreplena":true}],"aktivnaya":999,"sleduyushchaya":0}"#).unwrap();
        let mut s = Sessiya::zagruzit(&p.0);
        assert_eq!(s.aktivnaya, 11);
        assert!(s.vkladki.iter().all(|v| v.route == crate::tab_routes::RouteChoice::Inherit));
        assert_eq!(s.vkladki[0].id, 22);
        assert!(!s.vkladki[1].zagruzka);
        assert_eq!(s.dobavit("about:blank"), 23);
        s.sohranit(&p.0).unwrap();
        let s = Sessiya::zagruzit(&p.0);
        assert_eq!(s.vkladki.len(), 3);
        assert_eq!(s.aktivnaya, 23);
    }

    #[test]
    fn corrupt_primary_recovers_previous_committed_snapshot_without_temporary_promotion() {
        let p = Papka::novaya();
        let first = Sessiya::novaya("https://first.test");
        first.sohranit(&p.0).unwrap();
        let second = Sessiya::novaya("https://second.test");
        second.sohranit(&p.0).unwrap();
        std::fs::write(p.0.join("obychnye.json"), b"{truncated").unwrap();
        std::fs::write(p.0.join("obychnye.tmp"), serde_json::to_vec(&second).unwrap()).unwrap();
        let restored = Sessiya::zagruzit(&p.0);
        assert_eq!(restored.vkladki[0].adres, "https://first.test");
        assert!(restored.vkladki[0].otlozhena);
        restored.sohranit(&p.0).unwrap();
        assert_eq!(Sessiya::zagruzit(&p.0).vkladki[0].adres, "https://first.test");
        assert_eq!(read_session(&p.0.join("obychnye.backup.json")).unwrap().vkladki[0].adres, "https://first.test");
        assert!(!p.0.join("obychnye.povrezhdena.json").exists());
    }

    #[test]
    fn private_rows_never_survive_primary_or_backup_recovery() {
        let p = Papka::novaya();
        let mut session = Sessiya::novaya("https://normal.test");
        session.dobavit("https://private.test");
        session.vkladki[1].privatnaya = true;
        session.zakrytye.push(session.vkladki[1].clone());
        std::fs::write(p.0.join("obychnye.json"), serde_json::to_vec(&session).unwrap()).unwrap();
        let restored = Sessiya::zagruzit(&p.0);
        assert_eq!(restored.vkladki.len(), 1);
        assert!(restored.zakrytye.is_empty());
        assert_eq!(restored.aktivnaya, 1);
        restored.sohranit(&p.0).unwrap();
        for name in ["obychnye.json", "obychnye.backup.json"] {
            assert!(!std::fs::read_to_string(p.0.join(name)).unwrap().contains("private.test"));
        }
    }

    #[test]
    fn all_private_current_session_does_not_restore_older_ordinary_pages() {
        let p = Papka::novaya();
        Sessiya::novaya("https://old.test").sohranit(&p.0).unwrap();
        let mut private = Sessiya::novaya("https://private.test");
        private.vkladki[0].privatnaya = true;
        private.sohranit(&p.0).unwrap();
        let restored = Sessiya::zagruzit(&p.0);
        assert_eq!(restored.vkladki.len(), 1);
        assert_eq!(restored.vkladki[0].adres, "about:blank");
    }

    #[test]
    fn route_roundtrip_preserves_intent_but_not_pending_runtime_state() {
        let p = Papka::novaya();
        let mut s = Sessiya::novaya("https://example.test");
        s.vkladki[0].route = crate::tab_routes::RouteChoice::Connection {
            id: crate::tab_routes::ConnectionId::parse("connection-1").unwrap(),
        };
        s.vkladki[0].route_generation = 7;
        s.vkladki[0].route_pending = true;
        s.vkladki[0].route_error = Some("Temporary failure".into());
        s.zakrytye.push(s.vkladki[0].clone());
        s.sohranit(&p.0).unwrap();
        let restored = Sessiya::zagruzit(&p.0);
        for tab in restored.vkladki.iter().chain(restored.zakrytye.iter()) {
            assert_eq!(tab.route, s.vkladki[0].route);
            assert_eq!(tab.route_generation, 0);
            assert!(!tab.route_pending);
            assert!(tab.route_error.is_none());
        }
    }

    #[test]
    fn unknown_route_survives_restore_as_blocked_without_resetting_session() {
        let p = Papka::novaya();
        let mut value = serde_json::to_value(Sessiya::novaya("https://example.test")).unwrap();
        value["vkladki"][0]["route"] = serde_json::json!({"kind":"future-route","secret":"not-persisted"});
        std::fs::write(p.0.join("obychnye.json"), serde_json::to_vec(&value).unwrap()).unwrap();
        let s = Sessiya::zagruzit(&p.0);
        assert_eq!(s.vkladki[0].adres, "https://example.test");
        assert!(matches!(s.vkladki[0].route, crate::tab_routes::RouteChoice::Blocked { .. }));
        s.sohranit(&p.0).unwrap();
        let persisted = std::fs::read_to_string(p.0.join("obychnye.json")).unwrap();
        assert!(!persisted.contains("not-persisted"));
        assert!(!persisted.contains("future-route"));
    }

    #[test]
    fn povrezhdennye_id_ne_padayut_i_ne_stirayut_ishodnik() {
        let p = Papka::novaya();
        let json = r#"{"vkladki":[{"id":1,"adres":"about:blank","zagolovok":"One"},{"id":1,"adres":"about:blank","zagolovok":"Two"}],"aktivnaya":1,"sleduyushchaya":0}"#;
        std::fs::write(p.0.join("obychnye.json"), json).unwrap();
        let s = Sessiya::zagruzit(&p.0);
        assert_eq!(s.vkladki.len(), 1);
        assert_eq!(s.vkladki[0].adres, "about:blank");
        s.sohranit(&p.0).unwrap();
        assert_eq!(
            std::fs::read_to_string(p.0.join("obychnye.povrezhdena.json")).unwrap(),
            json
        );
        Sessiya::zagruzit(&p.0).sohranit(&p.0).unwrap();
        assert_eq!(
            std::fs::read_to_string(p.0.join("obychnye.povrezhdena.json")).unwrap(),
            json
        );
    }
}

#[cfg(test)]
mod order_tests {
    use super::*;
    #[test]
    fn perenos_sohranyaet_aktivnuyu_i_gruppu_zakreplennyh() {
        let mut s = Sessiya::novaya("https://a.test");
        let a=s.aktivnaya; let b=s.dobavit("https://b.test"); let c=s.dobavit("https://c.test");
        s.vkladki.iter_mut().find(|v|v.id==a).unwrap().zakreplena=true;
        s.peremestit(c,Some(a));
        assert_eq!(s.vkladki[0].id,a); assert_eq!(s.vkladki[1].id,c); assert_eq!(s.aktivnaya,c);
        s.peremestit(a,None); assert_eq!(s.vkladki[0].id,a);
        s.peremestit(b,Some(c)); assert_eq!(s.vkladki[1].id,b);
    }
}
