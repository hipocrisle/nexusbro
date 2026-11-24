//! Read-only native observation of an already admitted internal player.
//! Numeric media observations are not themselves playback/route acceptance.
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
const MAX_COUNTER: u64 = 9_007_199_254_740_991;
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
        "configured",
        "disposed",
        "playing",
        "separate",
        "position",
        "duration",
        "skew",
        "decoded",
        "dropped",
    ];
    if source.len() != FIELDS.len() || source.keys().any(|k| !FIELDS.contains(&k.as_str())) {
        return Err(FAILED);
    }
    let mut clean = serde_json::Map::new();
    for field in ["configured", "disposed", "playing", "separate"] {
        clean.insert(
            field.into(),
            Value::Bool(source.get(field).and_then(Value::as_bool).ok_or(FAILED)?),
        );
    }
    for field in ["position", "duration", "skew"] {
        let number = source
            .get(field)
            .and_then(Value::as_f64)
            .filter(|n| n.is_finite() && (0.0..=MAX_SECONDS).contains(n))
            .ok_or(FAILED)?;
        clean.insert(field.into(), json!(number));
    }
    for field in ["decoded", "dropped"] {
        let number = source
            .get(field)
            .and_then(Value::as_u64)
            .filter(|n| *n <= MAX_COUNTER)
            .ok_or(FAILED)?;
        clean.insert(field.into(), json!(number));
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
        && crate::cef_youtube::logical_url(identity, document).is_some()
}
wrap_dev_tools_message_observer! {
 struct Observer{identity:i32,frame_id:String,document:String,message:i32,state:Arc<Mutex<Completion>>}
 impl DevToolsMessageObserver{
  fn on_dev_tools_method_result(&self,browser:Option<&mut Browser>,message_id:i32,success:i32,result:Option<&[u8]>){
   if message_id!=self.message{return;}
   let Ok(mut state)=self.state.lock()else{return;};if state.done{return;}
   state.done=true;
   state.reply=Some(if success==0||!browser.is_some_and(|b|document_matches(b,self.identity,&self.frame_id,&self.document)){Err(FAILED)}else{result.ok_or(FAILED).and_then(decode)});
  }
  fn on_dev_tools_agent_detached(&self,browser:Option<&mut Browser>){
   if browser.is_some_and(|b|b.identifier()!=self.identity){return;}
   if let Ok(mut state)=self.state.lock(){if !state.done{state.done=true;state.reply=Some(Err(FAILED));}}
  }
 }
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
    pub fn start(browser: &Browser) -> Result<Self, &'static str> {
        if currently_on(ThreadId::UI) == 0 || browser.is_valid() == 0 {
            return Err(FAILED);
        }
        let identity = browser.identifier();
        let frame = browser.main_frame().ok_or(FAILED)?;
        let document = CefString::from(&frame.url()).to_string();
        let frame_id = CefString::from(&frame.identifier()).to_string();
        if !document_matches(browser, identity, &frame_id, &document) {
            return Err(FAILED);
        }
        let message = crate::cdp::reserve(1).ok_or(FAILED)?;
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
            .ok_or(FAILED)?;
        let pending = Self {
            registration: Some(registration),
            browser: browser.clone(),
            identity,
            frame_id,
            document,
            state,
            started: Instant::now(),
            consumed: false,
            _ui: PhantomData,
        };
        if !crate::cdp::send(
            browser,
            message,
            "Runtime.evaluate",
            json!({"expression":"window.NB_LIGHT && window.NB_LIGHT.snapshot()","returnByValue":true,"silent":true}),
        ) {
            return Err(FAILED);
        }
        Ok(pending)
    }
    pub fn poll(&mut self) -> Option<Reply> {
        if self.consumed {
            return None;
        }
        if currently_on(ThreadId::UI) == 0
            || self.started.elapsed() >= TIMEOUT
            || !document_matches(&self.browser, self.identity, &self.frame_id, &self.document)
        {
            return self.fail();
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
    fn fail(&mut self) -> Option<Reply> {
        self.consumed = true;
        self.registration.take();
        if let Ok(mut state) = self.state.lock() {
            state.done = true;
            state.reply.take();
        }
        Some(Err(FAILED))
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
        json!({"configured":true,"disposed":false,"playing":true,"separate":true,"position":2.25,"duration":600,"skew":0.02,"decoded":66,"dropped":1})
    }
    fn envelope(value: Value) -> Vec<u8> {
        serde_json::to_vec(&json!({"result":{"type":"object","value":value}})).unwrap()
    }
    #[test]
    fn reconstructs_only_bounded_native_player_fields() {
        let bytes = envelope(data());
        let value = decode(&bytes).unwrap();
        assert_eq!(value.as_object().unwrap().len(), 9);
        assert_eq!(value["decoded"], 66);
        assert_eq!(value["playing"], true);
        let mut result: Value = serde_json::from_slice(&bytes).unwrap();
        result["result"]["description"] = json!("https://secret/path?token=private");
        result["unrelated"] = json!("never-return");
        let clean = decode(&serde_json::to_vec(&result).unwrap()).unwrap();
        let wire = serde_json::to_string(&clean).unwrap();
        assert!(!wire.contains("secret"));
        assert!(!wire.contains("token"));
        assert!(!wire.contains("unrelated"));
    }
    #[test]
    fn malformed_exception_extra_fields_and_forged_numbers_refuse() {
        for (field, value) in [
            ("playing", json!(1)),
            ("configured", json!("true")),
            ("position", json!(-1)),
            ("duration", json!(604801)),
            ("skew", Value::Null),
            ("decoded", json!(9007199254740992u64)),
            ("dropped", json!(1.25)),
            ("secret_url", json!("never-reflect")),
        ] {
            let mut v = data();
            v[field] = value;
            assert_eq!(decode(&envelope(v)).unwrap_err(), FAILED);
        }
        let mut missing = data();
        missing.as_object_mut().unwrap().remove("position");
        assert!(decode(&envelope(missing)).is_err());
        let exception = json!({"result":{"type":"object","value":data()},"exceptionDetails":{"text":"do-not-copy"}});
        assert_eq!(
            decode(&serde_json::to_vec(&exception).unwrap()).unwrap_err(),
            FAILED
        );
        assert!(decode(&vec![b' '; MAX_BYTES + 1]).is_err());
        assert!(decode(b"null").is_err());
        assert!(decode(br#"{"result":{"type":"undefined"}}"#).is_err());
        assert!(decode(br#"{"result":{"type":"object","value":{"position":1e999}}}"#).is_err());
    }
}
