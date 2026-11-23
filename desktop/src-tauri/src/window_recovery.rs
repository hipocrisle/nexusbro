//! Disk snapshots for ordinary secondary windows. Native window/process handles,
//! browser IDs and private sessions are deliberately absent from this format.
use crate::sessiya::{durable_replace, recovery_read, Sessiya};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::Path;
use std::sync::Mutex;

static WRITES: Mutex<()> = Mutex::new(());

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SuperSession {
    pub super_id: String,
    pub session: Sessiya,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WindowSession {
    pub key: String,
    pub active_super: String,
    pub views: Vec<SuperSession>,
    #[serde(default, skip_serializing)]
    pub privatnaya: bool,
}

#[derive(Serialize, Deserialize)]
struct Document {
    schema: u32,
    windows: Vec<WindowSession>,
}

fn ordinary_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 128 && !crate::privatnost::eto_id(id)
        && id.bytes().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_'))
}

impl WindowSession {
    fn sanitized(self) -> Option<Self> {
        if self.privatnaya || !ordinary_id(&self.active_super) || self.views.len() > 256
            || self.key.is_empty() || self.key.len() > 128
            || !self.key.bytes().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_'))
        { return None; }
        let mut seen = HashSet::new();
        let views = self.views.into_iter().filter_map(|view| {
            if !ordinary_id(&view.super_id) || !seen.insert(view.super_id.clone()) { return None; }
            Some(SuperSession { super_id: view.super_id, session: view.session.validated_restore()? })
        }).collect::<Vec<_>>();
        if !views.iter().any(|v| v.super_id == self.active_super) { return None; }
        Some(Self { views, ..self })
    }
}

fn decode(bytes: &[u8]) -> Option<Vec<WindowSession>> {
    let document: Document = serde_json::from_slice(bytes).ok()?;
    if document.schema != 1 || document.windows.len() > 64 { return None; }
    let mut keys = HashSet::new();
    Some(document.windows.into_iter().filter_map(WindowSession::sanitized)
        .filter(|w| keys.insert(w.key.clone())).collect())
}

/// Root must additionally filter IDs against current Super definitions, since a
/// legacy private definition need not have a pvt-prefixed ID. No engine starts.
pub fn load(root: &Path) -> Vec<WindowSession> {
    let _guard = WRITES.lock().unwrap_or_else(|e| e.into_inner());
    read(root)
}
fn read(root: &Path) -> Vec<WindowSession> {
    for filename in ["windows-recovery.json", "windows-recovery.backup.json"] {
        if let Some(windows) = recovery_read(&root.join(filename)).ok().and_then(|b| decode(&b)) {
            return windows;
        }
    }
    Vec::new()
}

/// A complete snapshot, including an empty collection after explicit close. The
/// caller must retain not-yet-attached recovery windows until restore finishes.
/// Only ordinary windows/Supers are accepted. Never call with private metadata.
pub fn save(root: &Path, windows: Vec<WindowSession>) -> anyhow::Result<()> {
    let _guard = WRITES.lock().unwrap_or_else(|e| e.into_inner());
    if windows.len() > 64 { anyhow::bail!("Too many recovery windows"); }
    let mut keys = HashSet::new();
    let windows = windows.into_iter().filter_map(WindowSession::sanitized)
        .filter(|w| keys.insert(w.key.clone())).collect();
    let bytes = serde_json::to_vec_pretty(&Document { schema: 1, windows })?;
    if bytes.len() > 64 * 1024 * 1024 { anyhow::bail!("Recovery snapshot too large"); }
    let target = root.join("windows-recovery.json");
    let previous = recovery_read(&target).ok();
    if previous.as_deref() == Some(bytes.as_slice()) { return Ok(()); }
    if let Some(windows) = previous.as_deref().and_then(decode) {
        durable_replace(&root.join("windows-recovery.backup.json"),
            &serde_json::to_vec_pretty(&Document { schema: 1, windows })?)?;
    }
    durable_replace(&target, &bytes)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn private_windows_and_profile_ids_are_never_recoverable() {
        let window = WindowSession { key: "browser-1".into(), active_super: "normal".into(),
            views: vec![SuperSession { super_id: "normal".into(), session: Sessiya::novaya("https://public.test") }], privatnaya: true };
        assert!(window.clone().sanitized().is_none());
        let mut window = window; window.privatnaya = false;
        window.views.push(SuperSession { super_id: "pvt-private".into(), session: Sessiya::novaya("https://secret.test") });
        let result = serde_json::to_string(&window.sanitized().unwrap()).unwrap();
        assert!(!result.contains("secret.test"));
        assert!(!result.contains("pvt-private"));
    }
    #[test]
    fn distinct_windows_keep_independent_sessions_and_routes() {
        let mut windows = Vec::new();
        for n in 1..=2 {
            let mut session = Sessiya::novaya(&format!("https://window-{n}.test"));
            session.vkladki[0].route = crate::tab_routes::RouteChoice::Direct;
            session.vkladki[0].browser_id = 777;
            windows.push(WindowSession { key: format!("browser-{n}"), active_super: "normal".into(),
                views: vec![SuperSession { super_id: "normal".into(), session }], privatnaya: false });
        }
        let restored = decode(&serde_json::to_vec(&Document { schema: 1, windows }).unwrap()).unwrap();
        assert_eq!(restored.len(), 2);
        for (n, window) in restored.iter().enumerate() {
            let tab = &window.views[0].session.vkladki[0];
            assert_eq!(tab.adres, format!("https://window-{}.test", n+1));
            assert_eq!(tab.route, crate::tab_routes::RouteChoice::Direct);
            assert!(tab.otlozhena);
            assert_eq!(tab.browser_id, 0);
        }
    }
}

/// Secondary CEF views allocate fresh IDs when recreated. Only the initiating
/// command may translate by row, and only while URL/route/privacy still match.
/// Never silently select a different row if a stale search/result has changed.
pub fn remap_command(before: &Sessiya, after: &Sessiya, mut command: crate::sessiya::Komanda) -> Result<crate::sessiya::Komanda, String> {
    use crate::sessiya::Komanda;
    let changed = before.vkladki.iter().map(|v|v.id).collect::<Vec<_>>()
        != after.vkladki.iter().map(|v|v.id).collect::<Vec<_>>();
    if !changed { return Ok(command); }
    let target = |id: &mut u64| -> Result<(), String> {
        let index = before.vkladki.iter().position(|v|v.id==*id).ok_or("Вкладка уже закрыта")?;
        let old = &before.vkladki[index];
        let new = after.vkladki.get(index).filter(|new| new.adres==old.adres && new.route==old.route && new.privatnaya==old.privatnaya)
            .ok_or("Список вкладок изменился. Повторите действие.")?;
        *id = new.id; Ok(())
    };
    match &mut command {
        Komanda::Vybrat{id}|Komanda::Zakryt{id}|Komanda::Usypit{id}|Komanda::MemoryAuto{id}
        |Komanda::MemoryKeep{id,..}|Komanda::ZakrytDrugie{id,..}|Komanda::Zakrepit{id,..}
        |Komanda::Pereyti{id,..}|Komanda::Nazad{id}|Komanda::Vpered{id}|Komanda::Obnovit{id}
        |Komanda::ObnovitBezKesha{id}|Komanda::Instrumenty{id}|Komanda::Nayti{id,..}
        |Komanda::PoiskZakryt{id}|Komanda::Perevesti{id}|Komanda::PerevodOriginal{id}
        |Komanda::PerevodAvto{id,..}|Komanda::Issledovat{id,..}|Komanda::Iskhodnik{id} => target(id)?,
        Komanda::OpenLink{opener_id,..} => target(opener_id)?,
        Komanda::Peremestit{id,pered} => { target(id)?; if let Some(id)=pered { target(id)?; } },
        Komanda::RouteApply{..}|Komanda::RouteFailed{..}|Komanda::PasswordForm{..}|Komanda::PerevodOtvet{..}
        |Komanda::OknoKomanda{..} => return Err("Операция относится к прежнему движку".into()),
        _ => {},
    }
    Ok(command)
}

/// Initial renderer effects (theme and interface refresh) are not user intent.
/// Keep a positive list so new maintenance commands cannot wake every window.
pub fn user_command_can_wake(command: &crate::sessiya::Komanda) -> bool {
    use crate::sessiya::Komanda;
    matches!(command, Komanda::Novaya{..}|Komanda::NovayaPrivatnaya{..}|Komanda::OpenLink{..}
        |Komanda::VosstanovitZakrytuyu|Komanda::Skachat{..}|Komanda::Vybrat{..}|Komanda::Zakryt{..}
        |Komanda::ZakrytDrugie{..}|Komanda::Zakrepit{..}|Komanda::Pereyti{..}|Komanda::Nazad{..}
        |Komanda::Vpered{..}|Komanda::Obnovit{..}|Komanda::ObnovitBezKesha{..}|Komanda::Instrumenty{..}
        |Komanda::Nayti{..}|Komanda::PoiskZakryt{..}|Komanda::Perevesti{..}|Komanda::PerevodOriginal{..}
        |Komanda::PerevodAvto{..}|Komanda::Issledovat{..}|Komanda::Iskhodnik{..}
        |Komanda::Peremestit{..}|Komanda::Sleduyushchaya{..}|Komanda::Masshtab{..})
}
