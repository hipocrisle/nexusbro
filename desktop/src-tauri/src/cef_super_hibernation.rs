//! Native whole-engine escrow state; private parent commands only.
use super::{sozdat_brauzer_routed, Konteyner, Sobytie};
use crate::{
    cef_cookie_transfer::Pending,
    sessiya::Sessiya,
    super_cookie_escrow::{self as escrow, CookieSnapshot, EphemeralKey},
    super_hibernation_protocol::{self as wire, Frame, Token},
};
use cef::*;
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    io::{BufRead, Write},
    sync::{
        mpsc::{Receiver, Sender},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};
use zeroize::Zeroizing;
const ANCHOR: u64 = u64::MAX;
const DEADLINE: Duration = Duration::from_secs(25);
thread_local! {static SHUTDOWN:std::cell::RefCell<Option<Token>>=Default::default();}
pub fn take_shutdown() -> Option<Token> {
    SHUTDOWN.with(|s| s.borrow_mut().take())
}
fn emit(frame: Frame) -> anyhow::Result<()> {
    let encoded = Zeroizing::new(serde_json::to_string(&frame)?);
    println!("NB-HIBERNATE {}", encoded.as_str());
    std::io::stdout().flush()?;
    Ok(())
}
fn profile_key() -> Option<String> {
    std::env::args().find_map(|s| s.strip_prefix("--nb-profile-key=").map(str::to_owned))
}
fn engine_generation() -> Option<u64> {
    std::env::args().find_map(|a| {
        a.strip_prefix("--nb-engine-generation=")
            .and_then(|v| v.parse().ok())
    })
}
fn token(p: &wire::Prepare) -> Token {
    Token {
        generation: p.scope.generation,
        attempt: p.scope.attempt,
        revision: p.revision,
    }
}
fn same(a: &Token, b: &Token) -> bool {
    a.generation == b.generation && a.attempt == b.attempt && a.revision == b.revision
}
#[derive(Default)]
struct Answer {
    reply: Option<bool>,
    seen: bool,
    bad: bool,
}
wrap_dev_tools_message_observer! {
 struct Observe{identity:i32,message:i32,state:Arc<Mutex<Answer>>,worker:bool}
 impl DevToolsMessageObserver{
  fn on_dev_tools_method_result(&self,browser:Option<&mut Browser>,message_id:i32,success:i32,_result:Option<&[u8]>){
   if message_id!=self.message{return;}if let Ok(mut s)=self.state.lock(){s.reply=Some(success!=0&&browser.is_some_and(|b|b.identifier()==self.identity&&b.is_valid()!=0));}
  }
  fn on_dev_tools_event(&self,browser:Option<&mut Browser>,method:Option<&CefString>,params:Option<&[u8]>){
   if !self.worker{return;}let name=method.map(|m|m.to_string()).unwrap_or_default();
   if !name.starts_with("ServiceWorker."){return;}let Ok(mut s)=self.state.lock()else{return;};
   if !browser.is_some_and(|b|b.identifier()==self.identity){s.bad=true;return;}
   let parsed=params.filter(|p|p.len()<=65536).and_then(|p|serde_json::from_slice::<Value>(p).ok());
   match name.as_str(){
    "ServiceWorker.workerRegistrationUpdated"=>{match parsed.as_ref().and_then(|v|v["registrations"].as_array()){Some(a)if a.is_empty()=>s.seen=true,_=>s.bad=true}},
    "ServiceWorker.workerVersionUpdated"=>{if !parsed.as_ref().and_then(|v|v["versions"].as_array()).is_some_and(|a|a.is_empty()){s.bad=true;}},
    _=>s.bad=true,
   }
  }
  fn on_dev_tools_agent_detached(&self,_browser:Option<&mut Browser>){if let Ok(mut s)=self.state.lock(){s.bad=true;s.reply=Some(false);}}
 }
}
struct Call {
    _registration: Registration,
    state: Arc<Mutex<Answer>>,
}
impl Call {
    fn start(b: &Browser, method: &str, params: Value, worker: bool) -> anyhow::Result<Self> {
        let message = crate::cdp::reserve(1).ok_or_else(|| anyhow::anyhow!("native ids"))?;
        let state = Arc::new(Mutex::new(Answer::default()));
        let reg = b
            .host()
            .and_then(|h| {
                h.add_dev_tools_message_observer(Some(&mut Observe::new(
                    b.identifier(),
                    message,
                    state.clone(),
                    worker,
                )))
            })
            .ok_or_else(|| anyhow::anyhow!("native observer"))?;
        anyhow::ensure!(
            crate::cdp::send(b, message, method, params),
            "native command"
        );
        Ok(Self {
            _registration: reg,
            state,
        })
    }
    fn result(&self) -> Option<bool> {
        self.state
            .lock()
            .map(|s| if s.bad { Some(false) } else { s.reply })
            .unwrap_or(Some(false))
    }
    fn workers(&self) -> Option<bool> {
        self.state
            .lock()
            .map(|s| {
                if s.bad || s.reply == Some(false) {
                    Some(false)
                } else if s.seen && s.reply == Some(true) {
                    Some(true)
                } else {
                    None
                }
            })
            .unwrap_or(Some(false))
    }
}
wrap_completion_callback! {struct Flushed{done:Arc<Mutex<bool>>}impl CompletionCallback{fn on_complete(&self){if let Ok(mut v)=self.done.lock(){*v=true;}}}}
#[derive(Clone, Copy, PartialEq)]
enum Phase {
    Probe,
    Freeze,
    Verify,
    Cookies,
    Prepared,
    Closing,
    NetworkDrain,
    FinalCookies,
    FinalWait,
    AnchorClose,
    Flush,
    Thaw,
    Stopped,
    Failed,
}
struct Page {
    identity: i32,
    document: String,
    request: String,
    verified: bool,
    call: Option<Call>,
}
pub struct Engine {
    request: Option<wire::Prepare>,
    key: Option<EphemeralKey>,
    phase: Phase,
    pages: HashMap<u64, Page>,
    scroll: Vec<wire::Scroll>,
    session: Option<Sessiya>,
    anchor: Option<Browser>,
    anchor_closed: bool,
    context: Option<RequestContext>,
    workers: Option<Call>,
    cookies: Option<Pending>,
    since: Instant,
    flush: Arc<Mutex<bool>>,
    refusal: Option<&'static str>,
    aborting: bool,
}
impl Default for Engine {
    fn default() -> Self {
        Self {
            request: None,
            key: None,
            phase: Phase::Probe,
            pages: HashMap::new(),
            scroll: Vec::new(),
            session: None,
            anchor: None,
            anchor_closed: false,
            context: None,
            workers: None,
            cookies: None,
            since: Instant::now(),
            flush: Arc::new(Mutex::new(false)),
            refusal: None,
            aborting: false,
        }
    }
}
impl Engine {
    pub fn active(&self) -> bool {
        self.request.is_some()
    }
    pub fn stopped(&self) -> bool {
        self.phase == Phase::Stopped
    }
    pub fn committed(&self) -> bool {
        matches!(
            self.phase,
            Phase::Closing
                | Phase::NetworkDrain
                | Phase::FinalCookies
                | Phase::FinalWait
                | Phase::AnchorClose
                | Phase::Flush
                | Phase::Stopped
                | Phase::Failed
        )
    }
    pub fn begin(
        &mut self,
        p: wire::Prepare,
        k: &Konteyner,
        s: &Sessiya,
        b: &HashMap<u64, Browser>,
        old_scroll: &HashMap<u64, (f64, f64)>,
        safe: bool,
        tx: &Sender<Sobytie>,
    ) -> anyhow::Result<()> {
        if self.active() {
            emit(Frame::Refused {
                token: token(&p),
                reason: "engine_busy".into(),
            })?;
            return Ok(());
        }
        let prepare_token = token(&p);
        let check=(||->anyhow::Result<(RequestContext,EphemeralKey,crate::tab_routes::NativeRoute,HashMap<u64,Page>)>{
   anyhow::ensure!(safe&&engine_generation()==Some(p.scope.generation)&&!b.is_empty()&&profile_key().as_deref()==Some(p.scope.profile_key.as_str()),"eligibility");
   let (context_key,context)=k.routes.borrow().hibernation_context().ok_or_else(||anyhow::anyhow!("context"))?;
   anyhow::ensure!(context_key==p.scope.context_key,"context scope");
   let key=EphemeralKey::from_base64(p.key.expose()).map_err(anyhow::Error::msg)?;
   for t in &s.vkladki{anyhow::ensure!(t.id!=ANCHOR&&!t.privatnaya&&!t.route_pending,"private or route");let d=k.routes.borrow().descriptor(&t.route).ok_or_else(||anyhow::anyhow!("route unknown"))?;anyhow::ensure!(d.key==context_key,"multiple contexts");}
   let first=s.vkladki.iter().find(|v|b.contains_key(&v.id)).ok_or_else(||anyhow::anyhow!("no page"))?;
   let route=k.routes.borrow().descriptor(&first.route).ok_or_else(||anyhow::anyhow!("route"))?;
   let mut pages=HashMap::new();
   for (id,browser) in b{let identity=browser.identifier();let document=crate::memory_guard::token(identity).ok_or_else(||anyhow::anyhow!("unknown document"))?;
    let request=format!("whole-{}-{}-{id}",p.scope.generation,p.scope.attempt);pages.insert(*id,Page{identity,document,request,verified:false,call:None});}
   Ok((context,key,route,pages))
  })();
        let (context, key, route, pages) = match check {
            Ok(v) => v,
            Err(_) => {
                emit(Frame::Refused {
                    token: prepare_token,
                    reason: "eligibility_or_scope".into(),
                })?;
                return Ok(());
            }
        };
        // No origin navigation is admitted on this exact-context carrier.
        let anchor = match sozdat_brauzer_routed(k, ANCHOR, "about:blank", tx, false, &route) {
            Ok(Some(b)) => b,
            _ => {
                emit(Frame::Refused {
                    token: prepare_token,
                    reason: "carrier_unavailable".into(),
                })?;
                return Ok(());
            }
        };
        let _ = crate::okna::sistema::pokazat(super::handle(&anchor), false);
        *self = Self::default();
        self.request = Some(p);
        self.key = Some(key);
        self.context = Some(context);
        self.pages = pages;
        self.anchor = Some(anchor);
        self.session = Some(s.clone());
        for (id, (x, y)) in old_scroll {
            if let Some(t) = s.vkladki.iter().find(|v| v.id == *id) {
                self.scroll.push(wire::Scroll {
                    id: *id,
                    route_generation: t.route_generation,
                    x: *x,
                    y: *y,
                });
            }
        }
        self.workers = self
            .anchor
            .as_ref()
            .and_then(|a| Call::start(a, "ServiceWorker.enable", json!({}), true).ok());
        if self.workers.is_none() {
            self.refuse(b, "worker_state_unknown");
            return Ok(());
        }
        for (id, page) in &self.pages {
            if !b.get(id).is_some_and(|browser| {
                crate::memory_cef::probe(browser, &page.request, &page.document)
            }) {
                self.refuse(b, "probe_unavailable");
                break;
            }
        }
        Ok(())
    }
    fn reset_deadline(&mut self) {
        self.since = Instant::now();
    }
    fn refuse(&mut self, b: &HashMap<u64, Browser>, reason: &'static str) {
        if self.committed() {
            self.phase = Phase::Failed;
            self.cookies = None;
            return;
        }
        self.refusal = Some(reason);
        self.cookies = None;
        self.phase = Phase::Thaw;
        self.reset_deadline();
        for (id, page) in &mut self.pages {
            page.call = b
                .get(id)
                .filter(|v| v.identifier() == page.identity)
                .and_then(|v| {
                    Call::start(
                        v,
                        "Page.setWebLifecycleState",
                        json!({"state":"active"}),
                        false,
                    )
                    .ok()
                });
        }
    }
    pub fn invalidate(&mut self, b: &HashMap<u64, Browser>) {
        if !self.committed() && self.phase != Phase::Thaw {
            self.refuse(b, "parent_state_changed");
        }
    }
    pub fn command(
        &mut self,
        command: &crate::sessiya::Komanda,
        k: &Konteyner,
        b: &HashMap<u64, Browser>,
        safe: bool,
    ) -> anyhow::Result<()> {
        use crate::sessiya::Komanda;
        let Some(request) = self.request.as_ref() else {
            return Ok(());
        };
        let current = token(request);
        match command {
            Komanda::HibernateCommit { token: t }
                if same(t, &current) && self.phase == Phase::Prepared =>
            {
                let valid = safe
                    && self
                        .workers
                        .as_ref()
                        .is_some_and(|w| w.workers() == Some(true))
                    && b.len() == self.pages.len()
                    && self.pages.iter().all(|(id, p)| {
                        b.get(id).is_some_and(|v| v.identifier() == p.identity)
                            && crate::memory_guard::token(p.identity).as_deref()
                                == Some(p.document.as_str())
                    });
                if !valid {
                    self.refuse(b, "commit_eligibility_changed");
                    return Ok(());
                }
                // Irreversible admission gate: no thaw/abort is permitted after
                // this succeeds. Remaining shutdown owns the profile lease.
                if !k
                    .routes
                    .borrow()
                    .quiesce_hibernation(&request.scope.context_key)
                {
                    self.refuse(b, "context_quiesce_refused");
                    return Ok(());
                }
                self.phase = Phase::Closing;
                self.reset_deadline();
                for (id, page) in &mut self.pages {
                    page.call = None;
                    if let Some(host) = b
                        .get(id)
                        .filter(|v| v.identifier() == page.identity)
                        .and_then(|v| v.host())
                    {
                        host.close_browser(0);
                    } else {
                        self.phase = Phase::Failed;
                    }
                }
            }
            Komanda::HibernateAbort { token: t } if same(t, &current) && !self.committed() => {
                self.aborting = true;
                self.refuse(b, "aborted");
            }
            Komanda::HibernateFinalAck { token: t }
                if same(t, &current) && self.phase == Phase::FinalWait =>
            {
                self.cookies = None;
                self.workers = None;
                self.phase = Phase::AnchorClose;
                self.reset_deadline();
                if let Some(host) = self.anchor.as_ref().and_then(|b| b.host()) {
                    host.close_browser(0);
                } else {
                    self.phase = Phase::Failed;
                }
            }
            Komanda::HibernatePrepare { .. }
            | Komanda::HibernateCommit { .. }
            | Komanda::HibernateAbort { .. }
            | Komanda::HibernateFinalAck { .. }
            | Komanda::GlavnoeSkryto
            | Komanda::ObnovitInterfeys
            | Komanda::PasswordsEnabled { .. } => {}
            _ => {
                self.invalidate(b);
            }
        }
        Ok(())
    }
    pub fn event(
        &mut self,
        event: &Sobytie,
        k: &Konteyner,
        s: &Sessiya,
        b: &mut HashMap<u64, Browser>,
        safe: bool,
    ) -> bool {
        if !self.active() {
            return false;
        }
        if native_close_event(event, k, b, self.anchor.as_ref()) {
            return true;
        }
        match event {
            Sobytie::MemoryMessage(id, identity, data)
                if self.pages.get(id).is_some_and(|p| p.identity == *identity) =>
            {
                let Some(page) = self.pages.get_mut(id) else {
                    return true;
                };
                if data["kind"] == "NB_MEMORY_DOCUMENT" {
                    if data["token"].as_str() != Some(page.document.as_str()) {
                        self.refuse(b, "document_changed");
                    }
                    return true;
                }
                if !matches!(self.phase, Phase::Probe | Phase::Verify)
                    || data["request"].as_str() != Some(page.request.as_str())
                    || data["token"].as_str() != Some(page.document.as_str())
                {
                    return true;
                }
                let coords = data["scroll_x"]
                    .as_f64()
                    .zip(data["scroll_y"].as_f64())
                    .filter(|(x, y)| {
                        x.is_finite() && y.is_finite() && x.abs() <= 1e8 && y.abs() <= 1e8
                    });
                if !safe
                    || data["known"] != true
                    || !data["reason"].is_null()
                    || coords.is_none()
                    || crate::memory_guard::token(*identity).as_deref()
                        != Some(page.document.as_str())
                {
                    self.refuse(b, "renderer_or_native_protected");
                    return true;
                }
                page.verified = true;
                if self.phase == Phase::Verify {
                    let (x, y) = coords.unwrap();
                    if let Some(tab) = s.vkladki.iter().find(|v| v.id == *id) {
                        self.scroll.retain(|v| v.id != *id);
                        self.scroll.push(wire::Scroll {
                            id: *id,
                            route_generation: tab.route_generation,
                            x,
                            y,
                        });
                    }
                }
                true
            }
            Sobytie::MemoryRefused(id, identity)
                if self.pages.get(id).is_some_and(|p| p.identity == *identity) =>
            {
                self.refuse(b, "beforeunload_veto");
                true
            }
            Sobytie::Zakryta(id, Some(identity)) => {
                if *id == ANCHOR
                    && self
                        .anchor
                        .as_ref()
                        .is_some_and(|a| a.identifier() == *identity)
                {
                    self.anchor_closed = true;
                    k.routes.borrow_mut().forget_browser(*identity);
                    return true;
                }
                if self.pages.get(id).is_some_and(|p| p.identity == *identity) {
                    if self.phase == Phase::Closing {
                        k.routes.borrow_mut().forget_browser(*identity);
                        b.remove(id);
                        self.pages.remove(id);
                    } else {
                        self.refuse(b, "unexpected_close");
                    }
                    return true;
                }
                false
            }
            // Every command is held while pages are frozen; an abort/thaw handshake is
            // required before input/navigation can resume. Parent queues wake intent.
            Sobytie::Komanda(_) => true,
            _ => false,
        }
    }
    pub fn tick(
        &mut self,
        s: &Sessiya,
        b: &HashMap<u64, Browser>,
        safe: bool,
    ) -> anyhow::Result<()> {
        if !self.active() {
            return Ok(());
        }
        if self.phase == Phase::Stopped || self.phase == Phase::Failed {
            return Ok(());
        }
        if self.since.elapsed() > DEADLINE {
            if self.phase == Phase::Thaw {
                self.phase = Phase::Failed;
            } else {
                self.refuse(b, "native_deadline");
            }
            return Ok(());
        }
        if !self.committed() && self.phase != Phase::Thaw {
            let membership = b.len() == self.pages.len()
                && self.pages.iter().all(|(id, p)| {
                    b.get(id).is_some_and(|v| v.identifier() == p.identity)
                        && crate::memory_guard::token(p.identity).as_deref()
                            == Some(p.document.as_str())
                });
            if !safe || !membership {
                self.refuse(b, "eligibility_changed");
                return Ok(());
            }
        }
        if self.phase != Phase::Thaw
            && self
                .workers
                .as_ref()
                .is_some_and(|w| w.workers() == Some(false))
        {
            self.refuse(b, "service_worker_present_or_unknown");
            return Ok(());
        }
        match self.phase {
            Phase::Probe
                if self.pages.values().all(|p| p.verified)
                    && self
                        .workers
                        .as_ref()
                        .is_some_and(|w| w.workers() == Some(true)) =>
            {
                self.phase = Phase::Freeze;
                self.reset_deadline();
                for (id, p) in &mut self.pages {
                    p.verified = false;
                    p.call = b.get(id).and_then(|v| {
                        Call::start(
                            v,
                            "Page.setWebLifecycleState",
                            json!({"state":"frozen"}),
                            false,
                        )
                        .ok()
                    });
                }
            }
            Phase::Freeze => {
                if self.pages.values().any(|p| {
                    p.call.as_ref().map(|c| c.result()) == Some(Some(false)) || p.call.is_none()
                }) {
                    self.refuse(b, "freeze_failed");
                } else if self
                    .pages
                    .values()
                    .all(|p| p.call.as_ref().is_some_and(|c| c.result() == Some(true)))
                {
                    self.phase = Phase::Verify;
                    self.reset_deadline();
                    for (id, p) in &mut self.pages {
                        p.request.push_str("-verify");
                        if !b
                            .get(id)
                            .is_some_and(|v| crate::memory_cef::probe(v, &p.request, &p.document))
                        {
                            self.refuse(b, "frozen_probe_failed");
                            break;
                        }
                    }
                }
            }
            Phase::Verify if self.pages.values().all(|p| p.verified) => {
                self.session = Some(s.clone());
                self.cookies = self.anchor.as_ref().and_then(|a| Pending::get(a).ok());
                self.phase = Phase::Cookies;
                self.reset_deadline();
                if self.cookies.is_none() {
                    self.refuse(b, "cookie_capture_unavailable");
                }
            }
            Phase::Cookies | Phase::FinalCookies => {
                if let Some(reply) = self.cookies.as_mut().and_then(|c| c.poll()) {
                    self.cookies = None;
                    let captured = (|| -> anyhow::Result<wire::Snapshot> {
                        let bytes = reply.map_err(anyhow::Error::msg)?;
                        let canonical =
                            CookieSnapshot::from_cdp_result(&bytes).map_err(anyhow::Error::msg)?;
                        let request = self.request.as_ref().unwrap();
                        let sealed = escrow::seal(
                            &request.scope,
                            self.key.as_ref().unwrap(),
                            &canonical.canonical_bytes(),
                        )
                        .map_err(anyhow::Error::msg)?;
                        Ok(wire::Snapshot {
                            token: token(request),
                            scope: request.scope.clone(),
                            sealed,
                            session: self.session.as_ref().unwrap().clone(),
                            scroll: self.scroll.clone(),
                        })
                    })();
                    match captured {
                        Ok(snapshot) => {
                            if self.phase == Phase::Cookies {
                                emit(Frame::Prepared { snapshot })?;
                                self.phase = Phase::Prepared;
                            } else {
                                emit(Frame::FinalSnapshot { snapshot })?;
                                self.phase = Phase::FinalWait;
                            }
                            self.reset_deadline();
                        }
                        Err(_) => self.refuse(b, "cookie_schema_or_capture_failed"),
                    }
                }
            }
            Phase::Closing if self.pages.is_empty() => {
                self.phase = Phase::NetworkDrain;
                self.reset_deadline();
                if let Some(context) = &self.context {
                    context.close_all_connections(Some(&mut Flushed::new(self.flush.clone())));
                } else {
                    self.phase = Phase::Failed;
                }
            }
            Phase::NetworkDrain if self.flush.lock().map(|v| *v).unwrap_or(false) => {
                if let Ok(mut v) = self.flush.lock() {
                    *v = false;
                }
                self.cookies = self.anchor.as_ref().and_then(|a| Pending::get(a).ok());
                self.phase = Phase::FinalCookies;
                self.reset_deadline();
                if self.cookies.is_none() {
                    self.phase = Phase::Failed;
                }
            }
            Phase::AnchorClose if self.anchor_closed => {
                self.anchor = None;
                self.phase = Phase::Flush;
                self.reset_deadline();
                let manager = self.context.as_ref().and_then(|c| c.cookie_manager(None));
                if !manager.is_some_and(|m| {
                    m.flush_store(Some(&mut Flushed::new(self.flush.clone()))) != 0
                }) {
                    self.phase = Phase::Failed;
                }
            }
            Phase::Flush if self.flush.lock().map(|f| *f).unwrap_or(false) => {
                self.cookies = None;
                self.workers = None;
                self.anchor = None;
                self.context = None;
                SHUTDOWN.with(|slot| *slot.borrow_mut() = self.request.as_ref().map(token));
                self.key = None;
                self.request = None;
                self.phase = Phase::Stopped;
            }
            Phase::Thaw => {
                if self
                    .pages
                    .values()
                    .all(|p| p.call.as_ref().is_some_and(|c| c.result() == Some(true)))
                {
                    // Close the auxiliary carrier independently; it never owns a session row.
                    if !self.anchor_closed {
                        if let Some(h) = self.anchor.as_ref().and_then(|a| a.host()) {
                            h.close_browser(0);
                        }
                        return Ok(());
                    }
                    let t = token(self.request.as_ref().unwrap());
                    if self.aborting {
                        emit(Frame::AbortAck { token: t })?;
                    } else {
                        emit(Frame::Refused {
                            token: t,
                            reason: self.refusal.unwrap_or("native_refused").into(),
                        })?;
                    }
                    *self = Self::default();
                }
            }
            _ => {}
        }
        Ok(())
    }
}

fn native_close_event(
    event: &Sobytie,
    k: &Konteyner,
    browsers: &HashMap<u64, Browser>,
    anchor: Option<&Browser>,
) -> bool {
    #[cfg(target_os = "windows")]
    if let Sobytie::ZakrytOkno(id, identity) = event {
        let browser = if *id == ANCHOR {
            anchor
        } else {
            browsers.get(id)
        };
        if let Some(b) = browser.filter(|b| b.identifier() == *identity) {
            use windows::Win32::{
                Foundation::HWND,
                System::Threading::{GetCurrentProcessId, GetCurrentThreadId},
                UI::WindowsAndMessaging::*,
            };
            unsafe {
                let h = HWND(super::handle(b) as usize as _);
                let mut pid = 0;
                let thread = GetWindowThreadProcessId(h, Some(&mut pid));
                if pid == GetCurrentProcessId()
                    && thread == GetCurrentThreadId()
                    && GetParent(h).ok().map(|p| p.0 as usize as u32) == Some(k.okno)
                {
                    let _ = DestroyWindow(h);
                }
            }
        }
        return true;
    }
    let _ = (event, k, browsers, anchor);
    false
}
fn wait_cookie(mut pending: Pending) -> anyhow::Result<Zeroizing<Vec<u8>>> {
    loop {
        do_message_loop_work();
        if let Some(result) = pending.poll() {
            return result.map_err(anyhow::Error::msg);
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}
fn now() -> anyhow::Result<u64> {
    Ok(std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs())
}
/// Runs before ordinary Browser creation and before starting the stdin relay.
pub fn restore_startup(
    k: &Konteyner,
    s: &mut Sessiya,
    tx: &Sender<Sobytie>,
    rx: &Receiver<Sobytie>,
) -> anyhow::Result<HashMap<u64, (f64, f64)>> {
    use std::io::Read;
    let generation = std::env::args()
        .find_map(|a| {
            a.strip_prefix("--nb-engine-generation=")
                .and_then(|v| v.parse::<u64>().ok())
        })
        .ok_or_else(|| anyhow::anyhow!("restore generation"))?;
    let result = (|| -> anyhow::Result<HashMap<u64, (f64, f64)>> {
        let mut line = Zeroizing::new(String::new());
        let stdin = std::io::stdin();
        let mut reader = stdin.lock().take(8 * 1024 * 1024 + 1);
        reader.read_line(&mut line)?;
        anyhow::ensure!(
            line.len() <= 8 * 1024 * 1024 && line.ends_with('\n'),
            "restore frame bound"
        );
        let restore: wire::Restore = serde_json::from_str(&line)?;
        let snapshot = restore.snapshot;
        anyhow::ensure!(
            profile_key().as_deref() == Some(snapshot.scope.profile_key.as_str())
                && snapshot.scope.generation == snapshot.token.generation
                && snapshot.scope.attempt == snapshot.token.attempt,
            "restore scope"
        );
        let key = EphemeralKey::from_base64(restore.key.expose()).map_err(anyhow::Error::msg)?;
        let clear =
            escrow::open(&snapshot.scope, &key, &snapshot.sealed).map_err(anyhow::Error::msg)?;
        let cookies = CookieSnapshot::from_cdp_result(&clear).map_err(anyhow::Error::msg)?;
        anyhow::ensure!(
            !snapshot.session.vkladki.is_empty() && snapshot.session.vkladki.len() <= 4096,
            "restore session bound"
        );
        let first = snapshot.session.vkladki.first().unwrap();
        let route = k
            .routes
            .borrow()
            .descriptor(&first.route)
            .ok_or_else(|| anyhow::anyhow!("restore route unavailable"))?;
        anyhow::ensure!(
            route.key == snapshot.scope.context_key,
            "restore context mismatch"
        );
        for tab in &snapshot.session.vkladki {
            anyhow::ensure!(
                tab.id != ANCHOR && !tab.privatnaya && !tab.route_pending,
                "restore private or route"
            );
            let d = k
                .routes
                .borrow()
                .descriptor(&tab.route)
                .ok_or_else(|| anyhow::anyhow!("restore route unknown"))?;
            anyhow::ensure!(
                d.key == snapshot.scope.context_key,
                "restore multiple context"
            );
        }
        let since = Instant::now();
        loop {
            if k.routes.borrow_mut().prepare(&route, false)?.is_some() {
                break;
            }
            anyhow::ensure!(since.elapsed() < DEADLINE, "restore context deadline");
            do_message_loop_work();
            std::thread::sleep(Duration::from_millis(2));
        }
        let (context_key, context) = k
            .routes
            .borrow()
            .hibernation_context()
            .ok_or_else(|| anyhow::anyhow!("restore singleton"))?;
        anyhow::ensure!(context_key == snapshot.scope.context_key, "restore context");
        let anchor = sozdat_brauzer_routed(k, ANCHOR, "about:blank", tx, false, &route)?
            .ok_or_else(|| anyhow::anyhow!("restore carrier"))?;
        let _ = crate::okna::sistema::pokazat(super::handle(&anchor), false);
        let workers = Call::start(&anchor, "ServiceWorker.enable", json!({}), true)?;
        let start = Instant::now();
        loop {
            do_message_loop_work();
            match workers.workers() {
                Some(true) => break,
                Some(false) => anyhow::bail!("restore workers"),
                None => {}
            }
            anyhow::ensure!(start.elapsed() < DEADLINE, "restore worker deadline");
            std::thread::sleep(Duration::from_millis(2));
        }
        wait_cookie(Pending::restore(&anchor, &cookies).map_err(anyhow::Error::msg)?)?;
        let readback = wait_cookie(Pending::get(&anchor).map_err(anyhow::Error::msg)?)?;
        cookies
            .verify_readback_at(&readback, now()?)
            .map_err(anyhow::Error::msg)?;
        anyhow::ensure!(workers.workers() == Some(true), "restore worker changed");
        let done = Arc::new(Mutex::new(false));
        let manager = context
            .cookie_manager(None)
            .ok_or_else(|| anyhow::anyhow!("restore manager"))?;
        anyhow::ensure!(
            manager.flush_store(Some(&mut Flushed::new(done.clone()))) != 0,
            "restore flush refused"
        );
        let start = Instant::now();
        while !done.lock().map(|v| *v).unwrap_or(false) {
            anyhow::ensure!(start.elapsed() < DEADLINE, "restore flush timeout");
            do_message_loop_work();
            std::thread::sleep(Duration::from_millis(2));
        }
        let mut scroll = HashMap::new();
        for item in snapshot.scroll {
            let tab = snapshot
                .session
                .vkladki
                .iter()
                .find(|v| v.id == item.id && v.route_generation == item.route_generation)
                .ok_or_else(|| anyhow::anyhow!("restore scroll identity"))?;
            anyhow::ensure!(
                item.x.is_finite()
                    && item.y.is_finite()
                    && item.x.abs() <= 1e8
                    && item.y.abs() <= 1e8
                    && !scroll.contains_key(&tab.id),
                "restore scroll bound"
            );
            scroll.insert(tab.id, (item.x, item.y));
        }
        drop(workers);
        anchor
            .host()
            .ok_or_else(|| anyhow::anyhow!("restore carrier lost"))?
            .close_browser(0);
        let start = Instant::now();
        loop {
            do_message_loop_work();
            let mut closed = false;
            for event in rx.try_iter() {
                native_close_event(&event, k, &HashMap::new(), Some(&anchor));
                if matches!(event,Sobytie::Zakryta(ANCHOR,Some(identity))if identity==anchor.identifier())
                {
                    closed = true;
                }
            }
            if closed {
                break;
            }
            anyhow::ensure!(start.elapsed() < DEADLINE, "restore carrier close deadline");
            std::thread::sleep(Duration::from_millis(2));
        }
        k.routes.borrow_mut().forget_browser(anchor.identifier());
        drop(anchor);
        drop(manager);
        drop(context);
        *s = snapshot.session;
        emit(Frame::Restored { generation })?;
        Ok(scroll)
    })();
    if result.is_err() {
        let _ = emit(Frame::RestoreRefused { generation });
    }
    // Do not leak serde/crypto/URL diagnostics (including the raw restore line).
    result.map_err(|_| anyhow::anyhow!("Native restoration refused"))
}
