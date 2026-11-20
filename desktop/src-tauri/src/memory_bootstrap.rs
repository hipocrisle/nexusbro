//! Register the early observer after Chromium conditional DOM features, before
//! any real page navigation. Missing/late bootstrap never authorizes unloading.
use cef::*;
use rand_core::{OsRng, RngCore};
use std::{
    cell::RefCell,
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};
thread_local! {static REGISTRATIONS:RefCell<HashMap<i32,Registration>>=Default::default();}
pub struct Bootstrap {
    pending: AtomicBool,
    approved: AtomicBool,
    closed: AtomicBool,
    seen_real: AtomicBool,
    initial_blank_pending: AtomicBool,
    target: Mutex<String>,
    name: String,
    nonce: String,
}
impl Bootstrap {
    pub fn new(target: &str) -> anyhow::Result<Arc<Self>> {
        let mut bytes = [0u8; 32];
        OsRng
            .try_fill_bytes(&mut bytes)
            .map_err(|_| anyhow::anyhow!("Не удалось подготовить защиту вкладки"))?;
        let hex = |bytes: &[u8]| bytes.iter().map(|v| format!("{v:02x}")).collect::<String>();
        Ok(Arc::new(Self {
            pending: AtomicBool::new(true),
            approved: AtomicBool::new(false),
            closed: AtomicBool::new(false),
            seen_real: AtomicBool::new(false),
            initial_blank_pending: AtomicBool::new(true),
            target: Mutex::new(target.into()),
            name: format!("__nb_memory_{}", hex(&bytes[..16])),
            nonce: hex(&bytes[16..]),
        }))
    }
    pub fn extra(&self) -> anyhow::Result<DictionaryValue> {
        let value = dictionary_value_create()
            .ok_or_else(|| anyhow::anyhow!("Не удалось подготовить параметры вкладки"))?;
        value.set_string(
            Some(&CefString::from("nb_memory_name")),
            Some(&CefString::from(self.name.as_str())),
        );
        value.set_string(
            Some(&CefString::from("nb_memory_nonce")),
            Some(&CefString::from(self.nonce.as_str())),
        );
        Ok(value)
    }
    pub fn approved(&self) -> bool {
        self.approved.load(Ordering::Acquire) && !self.closed.load(Ordering::Acquire)
    }
    /// User navigation before setup supersedes the initial saved URL. No page
    /// can run before its script-on-new-document has been registered.
    pub fn rejects_initial_document(&self, url: &str) -> bool {
        (url.is_empty() || url == "about:blank")
            && self.target.lock().unwrap().as_str() != "about:blank"
    }
    /// Permit only the browser's first internal blank navigation. It starts the
    /// renderer needed to acknowledge CDP registration, while real URLs wait.
    pub fn allow_initial_blank(&self, url: &str) -> bool {
        url == "about:blank" && self.initial_blank_pending.swap(false, Ordering::AcqRel)
    }
    pub fn defer(&self, url: &str) -> bool {
        if self.pending.load(Ordering::Acquire) && !url.is_empty() {
            *self.target.lock().unwrap() = url.into();
            true
        } else {
            false
        }
    }
    pub fn skip_address(&self, url: &str) -> bool {
        if !url.is_empty() && url != "about:blank" {
            self.seen_real.store(true, Ordering::Release);
            return false;
        }
        !self.seen_real.load(Ordering::Acquire)
            && self.target.lock().unwrap().as_str() != "about:blank"
    }
    pub fn waiting_for_target(&self) -> bool {
        !self.seen_real.load(Ordering::Acquire)
            && self.target.lock().unwrap().as_str() != "about:blank"
    }
    pub fn close(&self, identity: i32) {
        self.closed.store(true, Ordering::Release);
        self.pending.store(false, Ordering::Release);
        self.approved.store(false, Ordering::Release);
        REGISTRATIONS.with(|r| {
            r.borrow_mut().remove(&identity);
        });
    }
}
fn finish(browser: &Browser, setup: &Bootstrap, approved: bool) {
    if setup.closed.load(Ordering::Acquire) || !setup.pending.swap(false, Ordering::AcqRel) {
        return;
    }
    setup.approved.store(approved, Ordering::Release);
    eprintln!(
        "NEXUSBRO_MEMORY_BOOTSTRAP browser={} registered={approved} safety={}",
        browser.identifier(),
        if approved { "early" } else { "keep" }
    );
    if browser.is_valid() == 0 {
        return;
    }
    let target = setup.target.lock().unwrap().clone();
    if approved&&target=="about:blank"{
        // The first internal blank predates script registration. Recreate only
        // the explicitly requested owned blank, so it gets the same early
        // observer and full media/edit protections as a normal web document.
        if let Some(frame)=browser.main_frame(){if CefString::from(&frame.url()).to_string()=="about:blank"{frame.load_url(Some(&CefString::from("about:blank")));}}
    }
    if !target.is_empty() && target != "about:blank" {
        if let Some(frame) = browser.main_frame() {
            if CefString::from(&frame.url()).to_string() == "about:blank" {
                // The blank exists only to start Chromium, not as a user history
                // entry. Replace it rather than append a second navigation.
                let script = format!("location.replace({});", serde_json::json!(target));
                frame.execute_java_script(Some(&CefString::from(script.as_str())), None, 0);
            } else {
                frame.load_url(Some(&CefString::from(target.as_str())));
            }
        }
    }
}
wrap_dev_tools_message_observer! {
    struct BootstrapObserver{setup:Arc<Bootstrap>,message:i32,enable:i32}
    impl DevToolsMessageObserver{
        fn on_dev_tools_method_result(&self,browser:Option<&mut Browser>,message_id:i32,success:i32,result:Option<&[u8]>){
            if message_id!=self.message && message_id!=self.enable{return;}
            let Some(browser)=browser else{return;};
            if !self.setup.pending.load(Ordering::Acquire){return;}
            if message_id==self.enable{
                eprintln!("NEXUSBRO_MEMORY_BOOTSTRAP stage=page_enable browser={} success={}",browser.identifier(),success!=0);
                if success==0{finish(browser,&self.setup,false);return;}
                let params=serde_json::json!({
                    "source":crate::cef_memory_renderer::new_document_script(&self.setup.name,&self.setup.nonce),
                    "runImmediately":self.setup.target.lock().unwrap().as_str()=="about:blank"
                });
                if !crate::cdp::send(browser,self.message,"Page.addScriptToEvaluateOnNewDocument",params){finish(browser,&self.setup,false);}
                return;
            }
            let registered=success!=0&&result.filter(|s|s.len()<4096).and_then(|s|serde_json::from_slice::<serde_json::Value>(s).ok()).is_some_and(|v|v["identifier"].as_str().is_some_and(|s|!s.is_empty()));
            eprintln!("NEXUSBRO_MEMORY_BOOTSTRAP stage=method_result browser={} success={} identifier_present={} error_code={}",browser.identifier(),success!=0,registered,result.filter(|s|s.len()<4096).and_then(|s|serde_json::from_slice::<serde_json::Value>(s).ok()).and_then(|v|v["code"].as_i64()).unwrap_or(0));
            finish(browser,&self.setup,registered);
        }
    }
}
wrap_task! {
    struct BootstrapTimeout{browser:Browser,setup:Arc<Bootstrap>}
    impl Task{fn execute(&self){if self.setup.pending.load(Ordering::Acquire){eprintln!("NEXUSBRO_MEMORY_BOOTSTRAP stage=deadline browser={}",self.browser.identifier());}finish(&self.browser,&self.setup,false);}}
}
pub fn start(browser: &Browser, setup: Arc<Bootstrap>) {
    let Some(enable) = crate::cdp::reserve(2) else {
        finish(browser, &setup, false);
        return;
    };
    let message = enable + 1;
    let Some(host) = browser.host() else {
        eprintln!(
            "NEXUSBRO_MEMORY_BOOTSTRAP stage=host_missing browser={}",
            browser.identifier()
        );
        finish(browser, &setup, false);
        return;
    };
    let Some(registration) = host.add_dev_tools_message_observer(Some(
        &mut BootstrapObserver::new(setup.clone(), message, enable),
    )) else {
        finish(browser, &setup, false);
        return;
    };
    eprintln!(
        "NEXUSBRO_MEMORY_BOOTSTRAP stage=observer_ready browser={}",
        browser.identifier()
    );
    REGISTRATIONS.with(|r| {
        r.borrow_mut().insert(browser.identifier(), registration);
    });
    if !crate::cdp::send(browser, enable, "Page.enable", serde_json::json!({})) {
        eprintln!(
            "NEXUSBRO_MEMORY_BOOTSTRAP stage=command_rejected browser={}",
            browser.identifier()
        );
        finish(browser, &setup, false);
        return;
    }
    if post_delayed_task(
        ThreadId::UI,
        Some(&mut BootstrapTimeout::new(browser.clone(), setup.clone())),
        8000,
    ) != 1
    {
        eprintln!(
            "NEXUSBRO_MEMORY_BOOTSTRAP stage=timer_rejected browser={}",
            browser.identifier()
        );
        finish(browser, &setup, false);
    }
}
