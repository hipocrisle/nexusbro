//! GHA fixture-only observation after player authority revocation.
//! Tokens never confer media access or authority to an arbitrary document.
use cef::*;
use serde_json::{json, Value};
use std::{
    marker::PhantomData,
    rc::Rc as ThreadBound,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use zeroize::{Zeroize, Zeroizing};
const FAILED: &str = "Native player observation refused";
const MAX_BYTES: usize = 4096;
const MAX_SECONDS: f64 = 7.0 * 24.0 * 60.0 * 60.0;
const TIMEOUT: Duration = Duration::from_secs(5);
type Reply = Result<Value, &'static str>;
fn wipe(v: &mut Value) {
    match v {
        Value::String(s) => s.zeroize(),
        Value::Array(a) => {
            for v in a {
                wipe(v)
            }
        }
        Value::Object(m) => {
            for (_, v) in m.iter_mut() {
                wipe(v)
            }
        }
        _ => {}
    }
}
struct Clear(Value);
impl Drop for Clear {
    fn drop(&mut self) {
        wipe(&mut self.0)
    }
}
fn decode(bytes: &[u8]) -> Reply {
    if bytes.is_empty() || bytes.len() > MAX_BYTES {
        return Err(FAILED);
    }
    let raw = Zeroizing::new(bytes.to_vec());
    let tree = Clear(serde_json::from_slice(&raw).map_err(|_| FAILED)?);
    let envelope = tree.0.as_object().ok_or(FAILED)?;
    if envelope.contains_key("exceptionDetails") {
        return Err(FAILED);
    }
    let result = envelope
        .get("result")
        .and_then(Value::as_object)
        .ok_or(FAILED)?;
    if result.get("type").and_then(Value::as_str) != Some("object") {
        return Err(FAILED);
    }
    let source = result
        .get("value")
        .and_then(Value::as_object)
        .ok_or(FAILED)?;
    const FIELDS: &[&str] = &[
        "disposed",
        "video_paused",
        "audio_paused",
        "video_source_empty",
        "audio_source_empty",
        "position",
        "audio_position",
    ];
    if source.len() != FIELDS.len() || source.keys().any(|k| !FIELDS.contains(&k.as_str())) {
        return Err(FAILED);
    }
    let mut clean = serde_json::Map::new();
    for field in &FIELDS[..5] {
        clean.insert(
            (*field).into(),
            Value::Bool(source.get(*field).and_then(Value::as_bool).ok_or(FAILED)?),
        );
    }
    for field in &FIELDS[5..] {
        let n = source
            .get(*field)
            .and_then(Value::as_f64)
            .filter(|n| n.is_finite() && (0.0..=MAX_SECONDS).contains(n))
            .ok_or(FAILED)?;
        clean.insert((*field).into(), json!(n));
    }
    Ok(Value::Object(clean))
}
struct Completion {
    done: bool,
    reply: Option<Reply>,
}
fn document_matches(browser: &Browser, identity: i32, frame_id: &str, document: &str) -> bool {
    if browser.identifier() != identity || browser.is_valid() == 0 {
        return false;
    }
    let Some(frame) = browser.main_frame() else {
        return false;
    };
    frame.is_main() != 0
        && CefString::from(&frame.identifier()).to_string() == frame_id
        && CefString::from(&frame.url()).to_string() == document
}
wrap_dev_tools_message_observer! {
 struct Observer{identity:i32,frame_id:String,document:String,message:i32,state:Arc<Mutex<Completion>>}
 impl DevToolsMessageObserver{
  fn on_dev_tools_method_result(&self,browser:Option<&mut Browser>,message_id:i32,success:i32,result:Option<&[u8]>){
   if message_id!=self.message{return;}
   let Ok(mut state)=self.state.lock()else{return;};if state.done{return;}
   state.done=true;
   state.reply=Some(if success==0{Err("cdp_method_failed")}else if !browser.is_some_and(|b|document_matches(b,self.identity,&self.frame_id,&self.document)){Err("probe_document_changed")}else{result.ok_or("cdp_result_absent").and_then(|v|decode(v))});
  }
  fn on_dev_tools_agent_detached(&self,browser:Option<&mut Browser>){
   if browser.is_some_and(|b|b.identifier()!=self.identity){return;}
   if let Ok(mut state)=self.state.lock(){if !state.done{state.done=true;state.reply=Some(Err("cdp_agent_detached"));}}
  }
 }
}
/// Native-only fixture capability. No serde, Clone, or unchecked constructor.
/// Valid only on its originating CEF UI thread for five seconds.
pub struct FixtureStopToken {
    identity: i32,
    frame_id: String,
    document: String,
    _operation: u64,
    minted: Instant,
    _ui: PhantomData<ThreadBound<()>>,
}
pub fn mint(browser: &Browser) -> Result<FixtureStopToken, &'static str> {
    if currently_on(ThreadId::UI) == 0 {
        return Err("stop_wrong_thread");
    }
    if std::env::var("GITHUB_ACTIONS").as_deref() != Ok("true")
        || std::env::var_os("NEXUSBRO_TEST_DATA").is_none()
    {
        return Err("stop_fixture_only");
    }
    let identity = browser.identifier();
    let frame = browser.main_frame().ok_or("stop_frame_absent")?;
    let frame_id = CefString::from(&frame.identifier()).to_string();
    let document = CefString::from(&frame.url()).to_string();
    if !document_matches(browser, identity, &frame_id, &document) {
        return Err("stop_document_changed");
    }
    let operation = crate::cef_youtube::fixture_stop_identity(identity, &document)
        .ok_or("stop_fixture_not_admitted")?;
    Ok(FixtureStopToken {
        identity,
        frame_id,
        document,
        _operation: operation,
        minted: Instant::now(),
        _ui: PhantomData,
    })
}
/// Keep and poll on the creating CEF UI thread. Drop before CEF shutdown.
pub struct Probe {
    registration: Option<Registration>,
    browser: Browser,
    identity: i32,
    frame_id: String,
    document: String,
    state: Arc<Mutex<Completion>>,
    started: Instant,
    consumed: bool,
    _ui: PhantomData<ThreadBound<()>>,
}
impl Probe {
    pub fn start(browser: &Browser, token: &FixtureStopToken) -> Result<Self, &'static str> {
        if currently_on(ThreadId::UI) == 0 {
            return Err("stop_wrong_thread");
        }
        if token.minted.elapsed() >= TIMEOUT {
            return Err("stop_token_expired");
        }
        if !document_matches(browser, token.identity, &token.frame_id, &token.document) {
            return Err("stop_document_changed");
        }
        let identity = token.identity;
        let frame_id = token.frame_id.clone();
        let document = token.document.clone();
        let message = crate::cdp::reserve(1).ok_or("probe_message_capacity")?;
        let state = Arc::new(Mutex::new(Completion {
            done: false,
            reply: None,
        }));
        let registration = browser
            .host()
            .and_then(|host| {
                host.add_dev_tools_message_observer(Some(&mut Observer::new(
                    identity,
                    frame_id.clone(),
                    document.clone(),
                    message,
                    state.clone(),
                )))
            })
            .ok_or("probe_observer_unavailable")?;
        let pending = Self {
            registration: Some(registration),
            browser: browser.clone(),
            identity,
            frame_id,
            document,
            state,
            started: token.minted,
            consumed: false,
            _ui: PhantomData,
        };
        if !crate::cdp::send(
            browser,
            message,
            "Runtime.evaluate",
            json!({"expression":"(()=>{const v=document.getElementById('video'),a=document.getElementById('audio');if(!window.NB_LIGHT||!v||!a)return null;return {disposed:window.NB_LIGHT.snapshot().disposed,video_paused:v.paused,audio_paused:a.paused,video_source_empty:!v.hasAttribute('src')&&!v.currentSrc,audio_source_empty:!a.hasAttribute('src')&&!a.currentSrc,position:v.currentTime||0,audio_position:a.currentTime||0};})()","returnByValue":true,"silent":true}),
        ) {
            return Err("probe_dispatch_failed");
        }
        Ok(pending)
    }
    pub fn poll(&mut self) -> Option<Reply> {
        if self.consumed {
            return None;
        }
        if currently_on(ThreadId::UI) == 0 {
            return self.fail("probe_wrong_thread");
        }
        if self.started.elapsed() >= TIMEOUT {
            return self.fail("probe_timeout");
        }
        if !document_matches(&self.browser, self.identity, &self.frame_id, &self.document) {
            return self.fail("probe_document_changed");
        }
        let reply = match self.state.lock() {
            Ok(mut state) => state.reply.take(),
            Err(_) => Some(Err(FAILED)),
        };
        if reply.is_some() {
            self.consumed = true;
            self.registration.take();
        }
        reply
    }
    fn fail(&mut self, code: &'static str) -> Option<Reply> {
        self.consumed = true;
        self.registration.take();
        if let Ok(mut state) = self.state.lock() {
            state.done = true;
            state.reply.take();
        }
        Some(Err(code))
    }
}
impl Drop for Probe {
    fn drop(&mut self) {
        self.registration.take();
        if let Ok(mut state) = self.state.lock() {
            state.done = true;
            state.reply.take();
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn data() -> Value {
        json!({"disposed":true,"video_paused":true,"audio_paused":true,"video_source_empty":true,"audio_source_empty":true,"position":0,"audio_position":0})
    }
    fn frame(v: Value) -> Vec<u8> {
        serde_json::to_vec(&json!({"result":{"type":"object","value":v}})).unwrap()
    }
    #[test]
    fn bounded_stop_fields_only() {
        let value = decode(&frame(data())).unwrap();
        assert_eq!(value.as_object().unwrap().len(), 7);
        for (key, value) in [
            ("disposed", json!(1)),
            ("position", json!(-1)),
            ("audio_position", json!(604801)),
            ("secret", json!("never-return")),
        ] {
            let mut v = data();
            v[key] = value;
            assert!(decode(&frame(v)).is_err());
        }
        let mut v = data();
        v.as_object_mut().unwrap().remove("disposed");
        assert!(decode(&frame(v)).is_err());
        assert!(decode(&vec![0; 4097]).is_err());
        assert!(decode(br#"{"result":{"type":"undefined"}}"#).is_err());
        assert!(decode(br#"{"exceptionDetails":{"text":"secret"}}"#).is_err());
    }
    #[test]
    fn observation_does_not_invent_success() {
        let mut v = data();
        v["disposed"] = json!(false);
        v["audio_paused"] = json!(false);
        let result = decode(&frame(v)).unwrap();
        assert_eq!(result["disposed"], false);
        assert_eq!(result["audio_paused"], false);
    }
}
