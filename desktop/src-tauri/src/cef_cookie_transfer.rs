//! UI-thread native CDP cookie transport. No navigation, profile selection or
//! lifecycle success is inferred here. Caller owns generation/attempt admission.
use crate::super_cookie_escrow::{CookieSnapshot, MAX_BYTES};
use cef::*;
use std::{
    marker::PhantomData,
    rc::Rc as ThreadBound,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use zeroize::Zeroizing;

pub const TIMEOUT: Duration = Duration::from_secs(10);
type Reply = Result<Zeroizing<Vec<u8>>, &'static str>;
const FAILED: &str = "Native cookie transfer refused";
struct Completion {
    done: bool,
    reply: Option<Reply>,
}
impl Completion {
    fn finish(&mut self, reply: Reply) {
        if !self.done {
            self.done = true;
            self.reply = Some(reply);
        }
    }
}
wrap_dev_tools_message_observer! {
    struct CookieObserver { identity:i32, message:i32, allow_empty:bool, completion:Arc<Mutex<Completion>> }
    impl DevToolsMessageObserver {
        fn on_dev_tools_method_result(&self,browser:Option<&mut Browser>,message_id:i32,success:i32,result:Option<&[u8]>) {
            if message_id!=self.message {return;}
            let Ok(mut completion)=self.completion.lock() else {return;};
            if completion.done {return;}
            if !browser.is_some_and(|b|b.identifier()==self.identity && b.is_valid()!=0) || success==0 {
                completion.finish(Err(FAILED));return;
            }
            // Borrowed CEF memory is valid only during this callback. Never
            // parse an error payload, log it, or copy an oversized response.
            let result=match result { None if self.allow_empty=>Some(b"{}".as_slice()), other=>other };
            let Some(bytes)=result.filter(|r|!r.is_empty()&&r.len()<=MAX_BYTES) else {
                completion.finish(Err(FAILED));return;
            };
            completion.finish(Ok(Zeroizing::new(bytes.to_vec())));
        }
        fn on_dev_tools_agent_detached(&self,browser:Option<&mut Browser>) {
            if browser.is_some_and(|b|b.identifier()!=self.identity) {return;}
            if let Ok(mut completion)=self.completion.lock(){completion.finish(Err(FAILED));}
        }
    }
}
/// Keep on the creating CEF UI thread, poll from its regular maintenance tick.
/// Dropping cancels observation and zeroizes any unconsumed response.
pub struct Pending {
    registration: Option<Registration>,
    browser: Browser,
    completion: Arc<Mutex<Completion>>,
    started: Instant,
    consumed: bool,
    _ui_thread: PhantomData<ThreadBound<()>>,
}
impl Pending {
    pub fn get(browser: &Browser) -> Result<Self, String> {
        Self::start(browser, "Storage.getCookies", None)
    }
    /// Only session CookieParam objects are sent. Caller must subsequently get
    /// and call CookieSnapshot::verify_readback_at before admitting navigation.
    pub fn restore(browser: &Browser, snapshot: &CookieSnapshot) -> Result<Self, String> {
        let params = snapshot.session_params()?;
        Self::start(browser, "Storage.setCookies", Some(&params))
    }
    fn start(browser: &Browser, method: &str, cookies: Option<&[u8]>) -> Result<Self, String> {
        if currently_on(ThreadId::UI) == 0 || browser.is_valid() == 0 {
            return Err(FAILED.into());
        }
        let host = browser.host().ok_or(FAILED)?;
        let message = crate::cdp::reserve(1).ok_or(FAILED)?;
        let completion = Arc::new(Mutex::new(Completion {
            done: false,
            reply: None,
        }));
        let registration = host
            .add_dev_tools_message_observer(Some(&mut CookieObserver::new(
                browser.identifier(),
                message,
                cookies.is_some(),
                completion.clone(),
            )))
            .ok_or(FAILED)?;
        let pending = Self {
            registration: Some(registration),
            browser: browser.clone(),
            completion,
            started: Instant::now(),
            consumed: false,
            _ui_thread: PhantomData,
        };
        // Fixed method names and integer IDs need no JSON escaping. Clear JSON
        // cookies remain exclusively in zeroizing serialized buffers, no Value.
        let mut wire = Zeroizing::new(format!(
            "{{\"id\":{message},\"method\":\"{method}\",\"params\":{{"
        ));
        if let Some(cookies) = cookies {
            let array = std::str::from_utf8(cookies).map_err(|_| FAILED)?;
            if cookies.len() > MAX_BYTES || !array.starts_with('[') || !array.ends_with(']') {
                return Err(FAILED.into());
            }
            wire.push_str("\"cookies\":");
            wire.push_str(array);
        }
        wire.push_str("}}");
        if wire.len() > MAX_BYTES || host.send_dev_tools_message(Some(wire.as_bytes())) == 0 {
            return Err(FAILED.into());
        }
        Ok(pending)
    }
    /// Deadline is not proof of engine exit. Timeout only fails this request.
    pub fn expired(&self, now: Instant) -> bool {
        now.checked_duration_since(self.started)
            .is_some_and(|elapsed| elapsed >= TIMEOUT)
    }
    /// Terminal result is returned once. No CEF object leaves the UI thread.
    pub fn poll(&mut self) -> Option<Reply> {
        if self.consumed {
            return None;
        }
        // PhantomData<Rc> prevents Send/Sync; runtime check also refuses misuse
        // through untrusted thread entry points without calling other CEF APIs.
        if currently_on(ThreadId::UI) == 0 {
            return self.fail();
        }
        if self.expired(Instant::now()) || self.browser.is_valid() == 0 {
            return self.fail();
        }
        let result = match self.completion.lock() {
            Ok(mut state) => state.reply.take(),
            Err(_) => Some(Err(FAILED)),
        };
        if result.is_some() {
            self.consumed = true;
            self.registration.take();
        }
        result
    }
    fn fail(&mut self) -> Option<Reply> {
        self.consumed = true;
        self.registration.take();
        if let Ok(mut completion) = self.completion.lock() {
            completion.done = true;
            completion.reply.take();
        }
        Some(Err(FAILED))
    }
}
impl Drop for Pending {
    fn drop(&mut self) {
        self.registration.take();
        if let Ok(mut completion) = self.completion.lock() {
            completion.done = true;
            completion.reply.take();
        }
    }
}
