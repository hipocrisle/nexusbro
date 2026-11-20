//! Per-Super contexts. A route is admitted only after its proxy preference is set.
//! The legacy Default directory stays bound to its first effective route key.
#[path = "route_profile_paths.rs"]
mod profile_paths;
use crate::tab_routes::{NativeRoute, RouteBootstrap, RouteChoice};
use cef::*;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU8, Ordering},
        Arc,
    },
};

wrap_resource_request_handler! {
    struct Admission { state: Arc<AtomicU8> }
    impl ResourceRequestHandler {
        fn on_before_resource_load(&self,_browser:Option<&mut Browser>,_frame:Option<&mut Frame>,_request:Option<&mut Request>,_callback:Option<&mut Callback>)->ReturnValue {
            if self.state.load(Ordering::Acquire)==1 {ReturnValue::CONTINUE} else {ReturnValue::CANCEL}
        }
    }
}
wrap_request_context_handler! {
    struct Initialized { state:Arc<AtomicU8>, proxy:String,allow_non_proxied_udp:bool }
    impl RequestContextHandler {
        fn on_request_context_initialized(&self,context:Option<&mut RequestContext>){
            let Some(context)=context else{self.state.store(2,Ordering::Release);return;};
            if self.proxy.is_empty(){self.state.store(if disable_prediction(context){3}else{2},Ordering::Release);return;}
            let good=configure(context,&self.proxy,self.allow_non_proxied_udp);
            self.state.store(if good{1}else{2},Ordering::Release);
        }
        fn resource_request_handler(&self,_browser:Option<&mut Browser>,_frame:Option<&mut Frame>,_request:Option<&mut Request>,_is_navigation:i32,_is_download:i32,_request_initiator:Option<&CefString>,_disable_default_handling:Option<&mut i32>)->Option<ResourceRequestHandler>{
            // Includes requests without a Browser (e.g. persisted service workers).
            // This gate proves admission ordering; SW behaviour still needs runtime acceptance.
            Some(Admission::new(self.state.clone()))
        }
    }
}
fn disable_prediction(context: &RequestContext) -> bool {
    let key = CefString::from("net.network_prediction_options");
    if context.can_set_preference(Some(&key)) == 0 {
        eprintln!("route-context: profile prediction policy is unavailable; admission denied");
        return false;
    }
    let Some(mut value) = value_create() else { return false; };
    if value.set_int(2) == 0 { return false; }
    let mut error = CefString::from("route prediction policy");
    let accepted = context.set_preference(Some(&key), Some(&mut value), Some(&mut error)) != 0;
    if !accepted { eprintln!("route-context: profile prediction policy was rejected; admission denied"); }
    accepted
}
fn configure(context: &RequestContext, proxy: &str, allow_non_proxied_udp: bool) -> bool {
    if !disable_prediction(context) { return false; }
    let policy = CefString::from("webrtc.ip_handling_policy");
    if context.can_set_preference(Some(&policy)) == 0 {
        eprintln!("route-context: WebRTC policy preference is unavailable; admission denied");
        return false;
    }
    let Some(mut rtc) = value_create() else {
        return false;
    };
    if rtc.set_string(Some(&CefString::from(if allow_non_proxied_udp {
        "default"
    } else {
        "disable_non_proxied_udp"
    }))) == 0
    {
        return false;
    }
    let mut rtc_error = CefString::from("route rtc policy");
    if context.set_preference(Some(&policy), Some(&mut rtc), Some(&mut rtc_error)) == 0 {
        eprintln!("route-context: WebRTC policy preference was rejected; admission denied");
        return false;
    }

    let key = CefString::from("proxy");
    if context.can_set_preference(Some(&key)) == 0 {
        eprintln!("route-context: proxy preference is locked; admission denied");
        return false;
    }
    let Some(mut dict) = dictionary_value_create() else {
        return false;
    };
    if dict.set_string(
        Some(&CefString::from("mode")),
        Some(&CefString::from("fixed_servers")),
    ) == 0
        || dict.set_string(
            Some(&CefString::from("server")),
            Some(&CefString::from(proxy)),
        ) == 0
        || dict.set_string(
            Some(&CefString::from("bypass_list")),
            Some(&CefString::from("<-loopback>")),
        ) == 0
    {
        return false;
    }
    let Some(mut value) = value_create() else {
        return false;
    };
    if value.set_dictionary(Some(&mut dict)) == 0 {
        return false;
    }
    // cef-rs requires allocated output storage, not CefString::default().
    let mut error = CefString::from("route preference");
    let accepted = context.set_preference(Some(&key), Some(&mut value), Some(&mut error)) != 0;
    if !accepted { eprintln!("route-context: proxy preference was rejected; admission denied"); }
    accepted
}
struct Entry {
    context: RequestContext,
    state: Arc<AtomicU8>,
    proxy: String,
    allow_non_proxied_udp: bool,
}
pub struct Registry {
    root: PathBuf,
    private: bool,
    legacy_key: String,
    bootstrap: RouteBootstrap,
    entries: HashMap<(String, bool), Entry>,
    browser_routes: HashMap<i32, String>,
}
impl Registry {
    pub fn new(root: &Path, bootstrap: RouteBootstrap, private: bool) -> anyhow::Result<Self> {
        anyhow::ensure!(
            bootstrap.inherit.validate() && bootstrap.bindings.len() <= 128,
            "Недопустимый начальный маршрут"
        );
        anyhow::ensure!(
            bootstrap
                .bindings
                .iter()
                .all(|b| b.prepared.as_ref().is_none_or(NativeRoute::validate)),
            "Недопустимая привязка маршрута"
        );
        let legacy_key = if private {
            bootstrap.inherit.key.clone()
        } else {
            let path = root.join("route-legacy.json");
            if !path.exists() {
                use std::io::Write;
                match std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&path)
                {
                    Ok(mut file) => {
                        file.write_all(bootstrap.inherit.key.as_bytes())?;
                        file.sync_all()?;
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                    Err(e) => return Err(e.into()),
                }
            }
            let bytes = std::fs::read(&path)?;
            anyhow::ensure!(bytes.len() == 64, "Повреждена привязка старого профиля");
            let value = String::from_utf8(bytes)?;
            anyhow::ensure!(
                NativeRoute {
                    key: value.clone(),
                    proxy: bootstrap.inherit.proxy.clone(),
                    allow_non_proxied_udp: false
                }
                .validate(),
                "Повреждена привязка старого профиля"
            );
            value
        };
        Ok(Self {
            root: root.into(),
            private,
            legacy_key,
            bootstrap,
            entries: HashMap::new(),
            browser_routes: HashMap::new(),
        })
    }
    pub fn snapshot(&mut self, snapshot: RouteBootstrap) -> anyhow::Result<()> {
        anyhow::ensure!(
            snapshot.inherit.validate()
                && snapshot.bindings.len() <= 128
                && snapshot
                    .bindings
                    .iter()
                    .all(|b| b.prepared.as_ref().is_none_or(NativeRoute::validate)),
            "Некорректный снимок маршрутов"
        );
        self.bootstrap = snapshot;
        Ok(())
    }
    pub fn bind(&mut self, choice: RouteChoice, prepared: NativeRoute) {
        if choice == RouteChoice::Inherit {
            self.bootstrap.inherit = prepared;
            return;
        }
        self.bootstrap.bindings.retain(|b| b.route != choice);
        self.bootstrap
            .bindings
            .push(crate::tab_routes::NativeRouteBinding {
                route: choice,
                prepared: Some(prepared),
                error: None,
            });
    }
    pub fn record_browser(&mut self, browser: i32, key: String) {
        self.browser_routes.insert(browser, key);
    }
    pub fn forget_browser(&mut self, browser: i32) {
        self.browser_routes.remove(&browser);
    }
    pub fn key_for_browser(&self, browser: i32) -> Option<String> {
        self.browser_routes.get(&browser).cloned()
    }
    pub fn descriptor(&self, choice: &RouteChoice) -> Option<NativeRoute> {
        if *choice == RouteChoice::Inherit {
            return Some(self.bootstrap.inherit.clone());
        }
        self.bootstrap
            .bindings
            .iter()
            .find(|b| &b.route == choice && b.error.is_none())
            .and_then(|b| b.prepared.clone())
    }
    pub fn prepare(
        &mut self,
        route: &NativeRoute,
        private: bool,
    ) -> anyhow::Result<Option<RequestContext>> {
        anyhow::ensure!(route.validate(), "Недопустимый маршрут");
        let key = (route.key.clone(), private);
        if !self.entries.contains_key(&key) {
            anyhow::ensure!(
                self.entries.len() < 256,
                "Достигнут лимит профилей маршрутов"
            );
            let mut settings = RequestContextSettings::default();
            if !private {
                let path=profile_paths::create_profile(&self.root,&route.key,&self.legacy_key)?;
                settings.cache_path = CefString::from(path.to_string_lossy().as_ref());
            }
            let state = Arc::new(AtomicU8::new(0));
            let context = request_context_create_context(
                Some(&settings),
                Some(&mut Initialized::new(
                    state.clone(),
                    route.proxy.clone(),
                    route.allow_non_proxied_udp,
                )),
            )
            .ok_or_else(|| anyhow::anyhow!("Не создан профиль маршрута"))?;
            self.entries.insert(
                key.clone(),
                Entry {
                    context,
                    state,
                    proxy: route.proxy.clone(),
                    allow_non_proxied_udp: route.allow_non_proxied_udp,
                },
            );
        }
        let entry = self.entries.get_mut(&key).unwrap();
        if entry.proxy.is_empty() {
            if entry.state.load(Ordering::Acquire) == 0 {
                return Ok(None);
            }
            anyhow::ensure!(
                entry.state.load(Ordering::Acquire) == 3,
                "Профиль недоступен"
            );
            if configure(&entry.context, &route.proxy, route.allow_non_proxied_udp) {
                entry.proxy = route.proxy.clone();
                entry.allow_non_proxied_udp = route.allow_non_proxied_udp;
                entry.state.store(1, Ordering::Release);
            } else {
                entry.state.store(2, Ordering::Release);
            }
        }
        anyhow::ensure!(
            entry.proxy == route.proxy
                && entry.allow_non_proxied_udp == route.allow_non_proxied_udp,
            "Транспорт активного профиля изменился"
        );
        match entry.state.load(Ordering::Acquire) {
            1 => Ok(Some(entry.context.clone())),
            2 => anyhow::bail!("CEF отклонил безопасную настройку маршрута"),
            _ => Ok(None),
        }
    }
    /// Opens only owned disk profiles without admitting network or starting VPN.
    /// The hard limit bounds malformed/foreign directories; errors never report a partial clear.
    pub fn all_owned(&mut self) -> anyhow::Result<Option<Vec<RequestContext>>> {
        if self.private {
            if self
                .entries
                .values()
                .any(|e| e.state.load(Ordering::Acquire) == 0)
            {
                return Ok(None);
            }
            return Ok(Some(
                self.entries.values().map(|e| e.context.clone()).collect(),
            ));
        }
        let owned=profile_paths::owned_profiles(&self.root,&self.legacy_key)?;
        let additional=owned.iter().filter(|(key,_)|!self.entries.contains_key(&(key.clone(),false))).count();
        anyhow::ensure!(self.entries.len().checked_add(additional).is_some_and(|count|count<=256),"Достигнут лимит профилей маршрутов");
        for (key, path) in owned {
            if self.entries.contains_key(&(key.clone(), false)) {
                continue;
            }
            profile_paths::validate_existing(&self.root,&path)?;
            let state = Arc::new(AtomicU8::new(0));
            let mut settings = RequestContextSettings::default();
            settings.cache_path = CefString::from(path.to_string_lossy().as_ref());
            let context = request_context_create_context(
                Some(&settings),
                Some(&mut Initialized::new(state.clone(), String::new(), false)),
            )
            .ok_or_else(|| anyhow::anyhow!("Не открыт профиль данных"))?;
            self.entries.insert(
                (key, false),
                Entry {
                    context,
                    state,
                    proxy: String::new(),
                    allow_non_proxied_udp: false,
                },
            );
        }
        if self
            .entries
            .values()
            .any(|e| e.state.load(Ordering::Acquire) == 0)
        {
            return Ok(None);
        }
        anyhow::ensure!(
            !self
                .entries
                .values()
                .any(|e| e.state.load(Ordering::Acquire) == 2),
            "Не все профили данных доступны"
        );
        Ok(Some(
            self.entries.values().map(|e| e.context.clone()).collect(),
        ))
    }
    pub fn has_private(&self) -> bool {
        self.entries.keys().any(|(_, p)| *p)
    }
    pub fn clear_private(&mut self) {
        for ((_, private), entry) in &self.entries {
            if *private {
                entry.state.store(2, Ordering::Release);
            }
        }
        self.entries.retain(|(_, p), _| !*p);
    }
}
