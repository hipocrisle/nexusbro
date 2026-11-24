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
        "audio_position",
        "video_width",
        "video_height",
        "hidden",
        "video_paused",
        "audio_paused",
        "caption_selected",
        "caption_cues",
        "caption_active",
        "caption_showing",
        "fullscreen",
    ];
    if source.len() != FIELDS.len() || source.keys().any(|k| !FIELDS.contains(&k.as_str())) {
        return Err(FAILED);
    }
    let mut clean = serde_json::Map::new();
    for field in ["configured", "disposed", "playing", "separate", "hidden", "video_paused", "audio_paused", "caption_showing", "fullscreen"] {
        clean.insert(
            field.into(),
            Value::Bool(source.get(field).and_then(Value::as_bool).ok_or(FAILED)?),
        );
    }
    let selected=source.get("caption_selected").and_then(Value::as_i64).filter(|n|(-1..=255).contains(n)).ok_or(FAILED)?;
    clean.insert("caption_selected".into(),json!(selected));
    for field in ["caption_cues","caption_active"]{
        let count=source.get(field).and_then(Value::as_u64).filter(|n|*n<=10000).ok_or(FAILED)?;
        clean.insert(field.into(),json!(count));
    }
    for field in ["position", "duration", "skew", "audio_position"] {
        let number = source
            .get(field)
            .and_then(Value::as_f64)
            .filter(|n| n.is_finite() && (0.0..=MAX_SECONDS).contains(n))
            .ok_or(FAILED)?;
        clean.insert(field.into(), json!(number));
    }
    for field in ["video_width", "video_height"] {
        let number = source
            .get(field)
            .and_then(Value::as_u64)
            .filter(|n| *n <= 16384)
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
/// Diagnostic only, invoked after strict decode refuses a result. Never returns
/// protocol content, field names, exception text or remote string values.
fn diagnose(bytes: &[u8]) -> &'static str {
    if bytes.len() > MAX_BYTES {
        return "snapshot_oversized";
    }
    let raw = Zeroizing::new(bytes.to_vec());
    let Ok(value) = serde_json::from_slice::<Value>(&raw) else {
        return "snapshot_invalid_json";
    };
    let tree = Clear(value);
    let Some(envelope) = tree.0.as_object() else {
        return "snapshot_not_object";
    };
    if envelope.contains_key("exceptionDetails") {
        return "snapshot_exception";
    }
    let Some(result) = envelope.get("result") else {
        return "snapshot_result_absent";
    };
    let Some(result) = result.as_object() else {
        return "snapshot_not_object";
    };
    if result.get("type").and_then(Value::as_str) == Some("undefined") {
        return "snapshot_undefined";
    }
    if result.get("type").and_then(Value::as_str) != Some("object") {
        return "snapshot_not_object";
    }
    let Some(value) = result.get("value") else {
        return "snapshot_value_absent";
    };
    let Some(source) = value.as_object() else {
        return "snapshot_not_object";
    };
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
        "audio_position",
        "video_width",
        "video_height",
        "hidden",
        "video_paused",
        "audio_paused",
        "caption_selected",
        "caption_cues",
        "caption_active",
        "caption_showing",
        "fullscreen",
    ];
    if source.len() != FIELDS.len() || source.keys().any(|k| !FIELDS.contains(&k.as_str())) {
        return "snapshot_schema_mismatch";
    }
    "snapshot_field_invalid"
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
   let matches=browser.is_some_and(|b|document_matches(b,self.identity,&self.frame_id,&self.document));
   let Ok(mut state)=self.state.lock()else{return;};if state.done{return;}
   state.done=true;
   state.reply=Some(if success==0{Err("cdp_method_failed")}else if !matches{Err("probe_document_changed")}else{result.ok_or("cdp_result_absent").and_then(|v|decode(v).map_err(|_|diagnose(v)))});
  }
  fn on_dev_tools_agent_detached(&self,browser:Option<&mut Browser>){
   if browser.is_some_and(|b|b.identifier()!=self.identity){return;}
   if let Ok(mut state)=self.state.lock(){if !state.done{state.done=true;state.reply=Some(Err("cdp_agent_detached"));}}
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
        if currently_on(ThreadId::UI) == 0 {
            return Err("probe_wrong_thread");
        }
        if browser.is_valid() == 0 {
            return Err("probe_browser_invalid");
        }
        let identity = browser.identifier();
        let frame = browser.main_frame().ok_or("probe_frame_absent")?;
        let document = CefString::from(&frame.url()).to_string();
        let frame_id = CefString::from(&frame.identifier()).to_string();
        if !document_matches(browser, identity, &frame_id, &document) {
            return Err("probe_document_not_admitted");
        }
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
        json!({"configured":true,"disposed":false,"caption_selected":-1,"caption_cues":0,"caption_active":0,"caption_showing":false,"fullscreen":false,"hidden":false,"video_paused":false,"audio_paused":false,"playing":true,"separate":true,"position":2.25,"duration":600,"skew":0.02,"decoded":66,"dropped":1,"audio_position":2.23,"video_width":1920,"video_height":1080})
    }
    fn envelope(value: Value) -> Vec<u8> {
        serde_json::to_vec(&json!({"result":{"type":"object","value":value}})).unwrap()
    }
    #[test]
    fn reconstructs_only_bounded_native_player_fields() {
        let bytes = envelope(data());
        let value = decode(&bytes).unwrap();
        assert_eq!(value.as_object().unwrap().len(), 20);
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
            ("video_height", json!(16385)),
            ("audio_position", json!(-1)),
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
    #[test]
    fn diagnostics_classify_only_fixed_shapes_without_echoing_content() {
        let cases: &[(&[u8], &str)] = &[
            (b"", "snapshot_invalid_json"),
            (b"null", "snapshot_not_object"),
            (
                br#"{"secret":"https://private/?token=secret"}"#,
                "snapshot_result_absent",
            ),
            (
                br#"{"exceptionDetails":{"text":"secret"}}"#,
                "snapshot_exception",
            ),
            (br#"{"result":{"type":"undefined"}}"#, "snapshot_undefined"),
            (
                br#"{"result":{"type":"string","value":"secret"}}"#,
                "snapshot_not_object",
            ),
            (
                br#"{"result":{"type":"object","description":"secret"}}"#,
                "snapshot_value_absent",
            ),
            (
                br#"{"result":{"type":"object","value":{"secret":"secret"}}}"#,
                "snapshot_schema_mismatch",
            ),
        ];
        for (bytes, expected) in cases {
            assert!(decode(bytes).is_err());
            assert_eq!(diagnose(bytes), *expected);
            assert!(!diagnose(bytes).contains("secret"));
        }
        assert_eq!(diagnose(&vec![b'x'; MAX_BYTES + 1]), "snapshot_oversized");
        let mut invalid = data();
        invalid["position"] = json!(-1);
        assert!(decode(&envelope(invalid.clone())).is_err());
        assert_eq!(diagnose(&envelope(invalid)), "snapshot_field_invalid");
    }
}
