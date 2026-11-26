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
    initial_target_failed: AtomicBool,
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
            initial_target_failed: AtomicBool::new(false),
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
    /// Only the explicitly requested initial owned blank may lack a URL/method
    /// in CEF's first callbacks. This never identifies an HTTP bootstrap blank.
    pub fn owned_blank_navigation(&self,url:&str)->bool{
        self.approved()&&!self.seen_real.load(Ordering::Acquire)
            &&self.target.lock().unwrap().as_str()=="about:blank"
            &&(url.is_empty()||url=="about:blank")
    }
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
    /// Terminal network failure ends the initial loading wait, not the initial
    /// blank/history suppression. Called only after a native latest-URL error.
    pub fn mark_terminal_failure(&self,browser:i32,failed_url:&str,epoch:u64)->Option<String>{
        if self.closed.load(Ordering::Acquire)||self.pending.load(Ordering::Acquire)
            ||crate::memory_guard::native_error_epoch(browser,failed_url)!=Some(epoch){return None;}
        let logical_target=self.target.lock().unwrap().clone();
        self.initial_target_failed.store(true,Ordering::Release);Some(logical_target)
    }
    pub fn waiting_for_target(&self) -> bool {
        !self.initial_target_failed.load(Ordering::Acquire)
            && !self.seen_real.load(Ordering::Acquire)
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
        if let Some(frame)=browser.main_frame(){
            let current=CefString::from(&frame.url()).to_string();
            eprintln!("NEXUSBRO_MEMORY_BOOTSTRAP stage=owned_blank browser={} initial_empty={} initial_blank={}",browser.identifier(),current.is_empty(),current=="about:blank");
            if setup.owned_blank_navigation(&current){
                // Some CEF initial-empty callbacks omit the GET method entirely.
                // Seed only this owned navigation, never unknown web documents.
                crate::memory_guard::navigation_to(browser.identifier(),true,"about:blank");
                frame.execute_java_script(Some(&CefString::from("location.replace(\"about:blank\")")),None,0);
            }
        }
    }
    if !target.is_empty() && target != "about:blank" {
        if let Some(frame) = browser.main_frame() {
            // Local files must be opened by the native host. JavaScript from
            // the opaque bootstrap blank is correctly forbidden to navigate
            // to file://; do not weaken Chromium's file-access protections.
            if reqwest::Url::parse(&target).ok().is_some_and(|u|u.scheme()=="file") {
                frame.load_url(Some(&CefString::from(target.as_str())));
            } else if CefString::from(&frame.url()).to_string() == "about:blank" {
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
fn registration_params(setup:&Bootstrap)->serde_json::Value{
    let owned_blank={setup.target.lock().unwrap().as_str()=="about:blank"};
    serde_json::json!({
        "source":crate::cef_memory_renderer::new_document_script(&setup.name,&setup.nonce,owned_blank),
        // finish() performs a fresh history-neutral navigation AFTER this ACK.
        // Capturing the initial empty document as well creates a second native
        // token whose delayed IPC can consume that fresh navigation's GET epoch.
        "runImmediately":false
    })
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
                let params=registration_params(&self.setup);
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

#[cfg(test)]
mod owned_blank_tests {
    use super::*;
    #[test]
    fn only_approved_initial_owned_blank_can_use_empty_native_metadata(){
        let owned=Bootstrap::new("about:blank").unwrap();
        assert!(!owned.owned_blank_navigation(""));
        owned.approved.store(true,Ordering::Release);
        assert!(owned.owned_blank_navigation(""));assert!(owned.owned_blank_navigation("about:blank"));
        assert!(!owned.owned_blank_navigation("https://site.test/"));
        owned.skip_address("https://site.test/");assert!(!owned.owned_blank_navigation("about:blank"));
        let web=Bootstrap::new("https://site.test/").unwrap();web.approved.store(true,Ordering::Release);
        assert!(!web.owned_blank_navigation(""));assert!(!web.owned_blank_navigation("about:blank"));assert!(web.rejects_initial_document("about:blank"));
        let closed=Bootstrap::new("about:blank").unwrap();closed.approved.store(true,Ordering::Release);closed.closed.store(true,Ordering::Release);
        assert!(!closed.owned_blank_navigation("about:blank"));
    }
    #[test]
    fn registration_never_captures_the_old_initial_document(){
        for target in ["about:blank","https://site.test/","about:blank#fragment",""]{
            let setup=Bootstrap::new(target).unwrap();
            let params=registration_params(&setup);
            assert_eq!(params["runImmediately"],false);
            let source=params["source"].as_str().unwrap();
            assert!(source.contains(if target=="about:blank"{"const nativeOwnedBlank=true;"}else{"const nativeOwnedBlank=false;"}));
        }
    }

    #[test]
    fn terminal_error_releases_loading_wait_without_arming_initial_blank(){
        let browser=908;let target="https://logical.test/";let failed="https://redirected.test/";
        let setup=Bootstrap::new(target).unwrap();
        crate::memory_guard::navigation_to(browser,true,failed);
        assert!(crate::memory_guard::native_load_error(browser,failed));
        let epoch=crate::memory_guard::native_error_epoch(browser,failed).unwrap();
        assert!(setup.waiting_for_target());assert!(setup.mark_terminal_failure(browser,failed,epoch).is_none());
        setup.approved.store(true,Ordering::Release);
        assert!(setup.mark_terminal_failure(browser,failed,epoch).is_none()); // registration pending
        setup.pending.store(false,Ordering::Release);
        assert!(setup.mark_terminal_failure(browser,"https://different.test/",epoch).is_none());
        assert!(setup.mark_terminal_failure(browser,failed,epoch+1).is_none());assert!(setup.waiting_for_target());
        assert_eq!(setup.mark_terminal_failure(browser,failed,epoch).as_deref(),Some(target));
        assert!(!setup.waiting_for_target());assert!(!setup.seen_real.load(Ordering::Acquire));
        assert!(setup.skip_address("about:blank"));assert!(setup.rejects_initial_document("about:blank"));
        assert!(!setup.owned_blank_navigation("about:blank"));
        crate::memory_guard::navigation_to(browser,true,failed); // same-URL retry invalidates old event
        assert!(setup.mark_terminal_failure(browser,failed,epoch).is_none());
        assert!(crate::memory_guard::native_load_error(browser,failed));
        let next=crate::memory_guard::native_error_epoch(browser,failed).unwrap();
        setup.closed.store(true,Ordering::Release);assert!(setup.mark_terminal_failure(browser,failed,next).is_none());
        crate::memory_guard::closed(browser);
    }

    #[test]
    fn unknown_safety_can_stop_loading_without_granting_document_approval(){
        let browser=909;let url="https://failed.test/";let setup=Bootstrap::new(url).unwrap();
        crate::memory_guard::navigation_to(browser,true,url);assert!(crate::memory_guard::native_load_error(browser,url));
        let epoch=crate::memory_guard::native_error_epoch(browser,url).unwrap();
        setup.pending.store(false,Ordering::Release); // finish(approved=false), safety remains unknown
        assert!(!setup.approved());assert!(crate::memory_guard::reason(browser).is_some());
        assert!(setup.mark_terminal_failure(browser,url,epoch).is_some());assert!(!setup.waiting_for_target());
        assert!(!setup.approved());assert!(crate::memory_guard::reason(browser).is_some());
        assert!(setup.rejects_initial_document("about:blank"));assert!(setup.skip_address("about:blank"));
        crate::memory_guard::closed(browser);
    }

}
