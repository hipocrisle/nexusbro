//! Bounded, engine-owned local transports. Never evict a live engine's route.
//! The inherited transport is borrowed; its lifetime remains with the parent.
use crate::tab_routes::{NativeRoute, RouteChoice};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use tokio::sync::{watch, OnceCell, OwnedSemaphorePermit, Semaphore};

const ERROR: &str = "Не удалось подготовить выбранный путь; переход заблокирован";
const RETIRED: &str = "Движок уже остановлен";
const MAX_OVERRIDES: usize = 7; // plus the inherited route = eight

/// Effective configuration identity, never an observed exit IP or connection ID.
/// Framing prevents ambiguous concatenation; credentials never leave the digest.
pub fn effective_key(address: &str, node: &str) -> String {
    let mut h = Sha256::new();
    let mut part = |v: &str| {
        h.update((v.len() as u64).to_be_bytes());
        h.update(v.as_bytes());
    };
    part("nexusbro-effective-route-v1");
    if crate::vpn::eto_podpiska(address.trim()) {
        part("vpn");
        part(address.trim());
        part(node.trim());
    } else {
        match crate::proxy::razobrat_vyhod(address) {
            Ok(crate::proxy::Vyhod::Pryamo) => part("direct"),
            Ok(crate::proxy::Vyhod::Socks5 { adres, vhod }) => {
                part("socks5");
                part(&adres);
                credentials(&mut part, vhod);
            }
            Ok(crate::proxy::Vyhod::Veb { adres, vhod }) => {
                part("http");
                part(&adres);
                credentials(&mut part, vhod);
            }
            Err(_) => {
                part("invalid");
                part(address.trim());
                part(node.trim());
            }
        }
    }
    format!("{:x}", h.finalize())
}
fn credentials(part: &mut impl FnMut(&str), value: Option<(String, String)>) {
    match value {
        Some((u, p)) => {
            part("auth");
            part(&u);
            part(&p);
        }
        None => part("anonymous"),
    }
}
fn port(route: &NativeRoute) -> Option<u16> {
    route
        .validate()
        .then(|| route.proxy.strip_prefix("http://127.0.0.1:")?.parse().ok())
        .flatten()
}

#[derive(Default)]
pub struct Registry {
    state: Mutex<State>,
}
#[derive(Default)]
struct State {
    engines: HashMap<(String, u64), Arc<Engine>>,
    retired: HashMap<String, u64>,
    stopped: bool,
}
struct Engine {
    inherit: NativeRoute,
    slots: Mutex<HashMap<String, Arc<OnceCell<Result<Arc<Runtime>, String>>>>>,
    cancel: watch::Sender<bool>,
}
struct Runtime {
    route: NativeRoute,
    resources: Mutex<Resources>,
}
#[derive(Default)]
struct Resources {
    proxy: Option<crate::proxy::Rabotayushchij>,
    tunnel: Option<std::process::Child>,
    directory: Option<PathBuf>,
    permit: Option<OwnedSemaphorePermit>,
}
impl Resources {
    fn stop(&mut self) {
        if let Some(p) = self.proxy.take() {
            p.ostanovit();
        }
        if let Some(mut c) = self.tunnel.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
        if let Some(p) = self.directory.take() {
            let _ = std::fs::remove_dir_all(p);
        }
        self.permit.take();
    }
}
impl Drop for Resources {
    fn drop(&mut self) {
        self.stop();
    }
}
impl Engine {
    fn stop(&self) {
        self.cancel.send_replace(true);
        let slots = std::mem::take(&mut *self.slots.lock().unwrap());
        for cell in slots.values() {
            if let Some(Ok(runtime)) = cell.get() {
                runtime.resources.lock().unwrap().stop();
            }
        }
    }
}
impl Drop for Registry {
    fn drop(&mut self) {
        self.stop_all();
    }
}

fn snapshot(choice: &RouteChoice) -> Result<(String, String), String> {
    match choice {
        RouteChoice::Direct => Ok((String::new(), String::new())),
        RouteChoice::Connection { id } => {
            let settings = crate::nastrojki::zagruzit_sync()
                .map_err(|_| ERROR.to_string())?
                .ok_or_else(|| ERROR.to_string())?;
            choice
                .resolve(settings.vyhody.iter().map(|v| v.id.as_str()))
                .map_err(|_| ERROR.to_string())?;
            let v = settings
                .vyhody
                .iter()
                .find(|v| v.id == id.as_str())
                .ok_or_else(|| ERROR.to_string())?;
            Ok((v.adres.clone(), v.proxy_uzel.clone()))
        }
        _ => Err(ERROR.into()),
    }
}
impl Registry {
    pub async fn prepare(
        &self,
        super_id: &str,
        engine: u64,
        settings: &crate::Nastrojki,
        choice: &RouteChoice,
        inherit: &NativeRoute,
    ) -> Result<NativeRoute, String> {
        if !inherit.validate() || super_id != settings.id {
            return Err(ERROR.into());
        }
        let owner = {
            let mut state = self.state.lock().unwrap();
            if state.stopped || state.retired.get(super_id).is_some_and(|n| engine <= *n) {
                return Err(RETIRED.into());
            }
            let owner = state
                .engines
                .entry((super_id.into(), engine))
                .or_insert_with(|| {
                    let (cancel, _) = watch::channel(false);
                    Arc::new(Engine {
                        inherit: inherit.clone(),
                        slots: Mutex::new(HashMap::new()),
                        cancel,
                    })
                })
                .clone();
            if owner.inherit.key != inherit.key || owner.inherit.proxy != inherit.proxy {
                return Err(ERROR.into());
            }
            owner
        };
        if matches!(choice, RouteChoice::Inherit) {
            return Ok(inherit.clone());
        }
        let (address, node) = snapshot(choice)?;
        let key = effective_key(&address, &node);
        if key == inherit.key {
            return Ok(inherit.clone());
        }
        let slot = {
            let mut slots = owner.slots.lock().unwrap();
            if !slots.contains_key(&key) && slots.len() >= MAX_OVERRIDES {
                return Err("Достигнут предел путей этого движка".into());
            }
            slots
                .entry(key.clone())
                .or_insert_with(|| Arc::new(OnceCell::new()))
                .clone()
        };
        let mut cancel = owner.cancel.subscribe();
        if *cancel.borrow() {
            return Err(RETIRED.into());
        }
        let future = slot.get_or_init(|| async {
            tokio::time::timeout(
                std::time::Duration::from_secs(30),
                build(
                    super_id,
                    engine,
                    &key,
                    &address,
                    &node,
                    settings.rezat_reklamu,
                ),
            )
            .await
            .map_err(|_| ERROR.to_string())
            .and_then(|v| v)
        });
        let result = tokio::select! {
            _ = cancel.changed() => Err(RETIRED.into()),
            result = future => result.clone(),
        };
        let still_owned = {
            let state = self.state.lock().unwrap();
            !state.stopped
                && state
                    .engines
                    .get(&(super_id.into(), engine))
                    .is_some_and(|v| Arc::ptr_eq(v, &owner))
        };
        if !still_owned || *cancel.borrow() {
            return Err(RETIRED.into());
        }
        if result.is_err() {
            let mut slots = owner.slots.lock().unwrap();
            if slots.get(&key).is_some_and(|v| Arc::ptr_eq(v, &slot)) {
                slots.remove(&key);
            }
        }
        let runtime = result?;
        if snapshot(choice)? != (address, node) {
            return Err(ERROR.into());
        }
        {
            let mut resources = runtime.resources.lock().unwrap();
            if resources.proxy.is_none() {
                return Err(ERROR.into());
            }
            if let Some(child) = resources.tunnel.as_mut() {
                if !matches!(child.try_wait(), Ok(None)) {
                    return Err(ERROR.into());
                }
            }
        }
        Ok(runtime.route.clone())
    }
    /// Only after CEF is dead and translation tasks have been cancelled.
    pub fn release_engine(&self, super_id: &str, engine: u64) {
        let owner = {
            let mut state = self.state.lock().unwrap();
            state
                .retired
                .entry(super_id.into())
                .and_modify(|n| *n = (*n).max(engine))
                .or_insert(engine);
            state.engines.remove(&(super_id.into(), engine))
        };
        if let Some(owner) = owner {
            owner.stop();
        }
    }
    pub fn stop_all(&self) {
        let engines = {
            let mut state = self.state.lock().unwrap();
            state.stopped = true;
            std::mem::take(&mut state.engines)
        };
        for owner in engines.values() {
            owner.stop();
        }
    }
    /// Unknown/stopped keys are denied, never resolved through the default.
    pub fn port_for_key(&self, super_id: &str, engine: u64, key: &str) -> Option<u16> {
        let owner = self
            .state
            .lock()
            .unwrap()
            .engines
            .get(&(super_id.into(), engine))?
            .clone();
        if *owner.cancel.borrow() {
            return None;
        }
        if key == owner.inherit.key {
            return port(&owner.inherit);
        }
        let slots = owner.slots.lock().unwrap();
        let runtime = slots.get(key)?.get()?.as_ref().ok()?;
        let mut resources = runtime.resources.lock().unwrap();
        if resources.proxy.is_none() {
            return None;
        }
        if let Some(child) = resources.tunnel.as_mut() {
            if !matches!(child.try_wait(), Ok(None)) {
                return None;
            }
        }
        port(&runtime.route)
    }
}
async fn build(
    super_id: &str,
    engine: u64,
    key: &str,
    address: &str,
    node: &str,
    adblock: bool,
) -> Result<Arc<Runtime>, String> {
    static STARTS: OnceLock<Semaphore> = OnceLock::new();
    static CHILDREN: OnceLock<Arc<Semaphore>> = OnceLock::new();
    static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let _start = STARTS
        .get_or_init(|| Semaphore::new(2))
        .acquire()
        .await
        .map_err(|_| ERROR.to_string())?;
    let mut resources = Resources::default();
    let exit = if crate::vpn::eto_podpiska(address.trim()) {
        resources.permit = Some(
            CHILDREN
                .get_or_init(|| Arc::new(Semaphore::new(16)))
                .clone()
                .try_acquire_owned()
                .map_err(|_| "Достигнут предел VPN-путей".to_string())?,
        );
        let link =
            if address.trim().starts_with("http://") || address.trim().starts_with("https://") {
                let nodes = crate::vpn::skachat_podpisku(address.trim())
                    .await
                    .map_err(|_| ERROR.to_string())?;
                crate::vpn::vybrat_uzel(&nodes, node)
                    .map_err(|_| ERROR.to_string())?
                    .ssylka
                    .clone()
            } else {
                address.trim().to_string()
            };
        let sequence = SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let directory = crate::brauzer::papka_profilya(super_id)
            .join("route-runtime")
            .join(format!("{engine}-{key}-{sequence}"));
        std::fs::create_dir_all(&directory).map_err(|_| ERROR.to_string())?;
        resources.directory = Some(directory.clone());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))
                .map_err(|_| ERROR.to_string())?;
        }
        let (child, port) = crate::vpn::podnyat_tunnel(&directory, &link)
            .await
            .map_err(|_| ERROR.to_string())?;
        resources.tunnel = Some(child);
        crate::proxy::Vyhod::Socks5 {
            adres: format!("127.0.0.1:{port}"),
            vhod: None,
        }
    } else {
        crate::proxy::razobrat_vyhod(address).map_err(|_| ERROR.to_string())?
    };
    let proxy = crate::proxy::podnyat(crate::proxy::Nastrojka {
        vyhod: exit,
        rezat_reklamu: adblock,
    })
    .await
    .map_err(|_| ERROR.to_string())?;
    let route = NativeRoute {
        key: key.into(),
        proxy: format!("http://127.0.0.1:{}", proxy.port),
    };
    resources.proxy = Some(proxy);
    Ok(Arc::new(Runtime {
        route,
        resources: Mutex::new(resources),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn identity_is_framed_and_normalized() {
        assert_eq!(effective_key("", "ignored"), effective_key("  ", ""));
        assert_eq!(
            effective_key("socks://u:p@localhost:1/", "ignored"),
            effective_key("socks5://u:p@localhost:1", "")
        );
        assert_ne!(
            effective_key("socks5://u:p@localhost:1", ""),
            effective_key("socks5://u:q@localhost:1", "")
        );
        assert_ne!(
            effective_key("vless://one", "a"),
            effective_key("vless://one", "b")
        );
        let key = effective_key("", "");
        assert!(NativeRoute {
            key,
            proxy: "http://127.0.0.1:1".into()
        }
        .validate());
    }
    #[test]
    fn unknown_transport_never_falls_back() {
        let registry = Registry::default();
        assert_eq!(
            registry.port_for_key("super", 1, &effective_key("", "")),
            None
        );
        registry.release_engine("super", 1);
        registry.stop_all();
        assert_eq!(registry.port_for_key("super", 1, "unknown"), None);
    }
}
