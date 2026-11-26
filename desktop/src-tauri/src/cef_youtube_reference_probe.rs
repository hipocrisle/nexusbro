//! GHA-only read-only observation of one ordinary public YouTube watch player.
//! Requires the public player content ID and known ad markers to be clear.
//! These observations are not universal ad absence, codec, GPU or route proof.
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
const SCRIPT: &str = "(()=>{const list=document.querySelectorAll('video');if(list.length!==1)return null;const player=document.querySelector('#movie_player.html5-video-player');if(!player||player.classList.contains('ad-showing')||player.classList.contains('ad-interrupting'))return null;const data=typeof player.getVideoData==='function'?player.getVideoData():null;if(!data||data.video_id!==new URL(location.href).searchParams.get('v'))return null;const v=list[0],q=v.getVideoPlaybackQuality();const norm=t=>(t||'').replace(/\s+/g,' ').trim();const vis=el=>{const st=getComputedStyle(el);return st.display!=='none'&&st.visibility!=='hidden';};const rectOf=el=>{if(!el||!vis(el))return null;const r=el.getBoundingClientRect();if(!(r.width>0&&r.height>0))return null;const cx=r.x+r.width/2,cy=r.y+r.height/2;if(cx<0||cy<0||cx>=innerWidth||cy>=innerHeight)return null;const hit=document.elementFromPoint(cx,cy);if(!(hit===el||el.contains(hit)))return null;return {x:r.x,y:r.y,width:r.width,height:r.height};};const one=sel=>{const l=document.querySelectorAll(sel);return l.length===1?l[0]:null;};const byLabel=(sel,test)=>{const m=Array.from(document.querySelectorAll(sel)).filter(e=>test(norm(e.textContent)));return m.length===1?m[0]:null;};const autohide=player.classList.contains('ytp-autohide');const reject=byLabel('button,[role=button]',t=>t==='Reject all'||t==='Alle ablehnen'||t==='\u041e\u0442\u043a\u043b\u043e\u043d\u0438\u0442\u044c \u0432\u0441\u0435');const controls={viewport:{width:innerWidth,height:innerHeight},reject_all_present:!!reject,play:autohide?null:rectOf(one('.ytp-play-button')),settings:autohide?null:rectOf(one('.ytp-settings-button')),quality:rectOf(byLabel('[role=menuitem]',t=>t==='Quality'||t==='\u041a\u0430\u0447\u0435\u0441\u0442\u0432\u043e'||t==='Qualit\u00e4t')),p720:rectOf(byLabel('[role=menuitem]',t=>t.indexOf('720p')===0)),reject_all:rectOf(reject)};return {volume:v.volume,muted:v.muted,default_sink:v.sinkId==='',playing:!v.paused&&!v.ended,paused:v.paused,ended:v.ended,seeking:v.seeking,ready_state:v.readyState,position:v.currentTime,duration:Number.isFinite(v.duration)?v.duration:0,video_width:v.videoWidth,video_height:v.videoHeight,decoded:q.totalVideoFrames,dropped:q.droppedVideoFrames,playback_rate:v.playbackRate,video_count:list.length,controls:controls};})()";
fn watch_id(document: &str) -> Option<crate::youtube_light::VideoId> {
    if document.len() > crate::youtube_light::MAX_INPUT_URL_BYTES {
        return None;
    }
    let url = reqwest::Url::parse(document).ok()?;
    if url.path() != "/watch" {
        return None;
    }
    crate::youtube_light::VideoId::from_url(document).ok()
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
        "volume",
        "muted",
        "default_sink",
        "playing",
        "paused",
        "ended",
        "seeking",
        "ready_state",
        "position",
        "duration",
        "video_width",
        "video_height",
        "decoded",
        "dropped",
        "playback_rate",
        "video_count",
    ];
    let has_controls = source.contains_key("controls");
    let expected_len = FIELDS.len() + usize::from(has_controls);
    if source.len() != expected_len
        || source
            .keys()
            .any(|k| !FIELDS.contains(&k.as_str()) && k != "controls")
    {
        return Err(FAILED);
    }
    let mut clean = serde_json::Map::new();
    for field in ["playing", "paused", "ended", "seeking", "muted", "default_sink"] {
        clean.insert(field.into(), json!(source[field].as_bool().ok_or(FAILED)?));
    }
    for (field, maximum) in [
        ("volume", 1.0),
        ("position", MAX_SECONDS),
        ("duration", MAX_SECONDS),
        ("playback_rate", 16.0),
    ] {
        let value = source[field]
            .as_f64()
            .filter(|n| n.is_finite() && (0.0..=maximum).contains(n))
            .ok_or(FAILED)?;
        clean.insert(field.into(), json!(value));
    }
    for (field, maximum) in [
        ("ready_state", 4),
        ("video_width", 16384),
        ("video_height", 16384),
        ("decoded", MAX_COUNTER),
        ("dropped", MAX_COUNTER),
        ("video_count", 1),
    ] {
        let value = source[field]
            .as_u64()
            .filter(|n| *n <= maximum)
            .ok_or(FAILED)?;
        if field == "video_count" && value != 1 {
            return Err(FAILED);
        }
        clean.insert(field.into(), json!(value));
    }
    if source["playing"].as_bool() == Some(true)
        && (source["paused"].as_bool() == Some(true) || source["ended"].as_bool() == Some(true))
    {
        return Err(FAILED);
    }
    // Optional controls: отсутствие (старый fixture) — честный null; при
    // наличии — строгий санитайзер, любое отклонение => FAILED.
    let controls = match source.get("controls") {
        None => Value::Null,
        Some(value) => sanitize_controls(value)?,
    };
    clean.insert("controls".into(), controls);
    Ok(Value::Object(clean))
}

fn rect(value: &Value, vw: f64, vh: f64) -> Result<Value, &'static str> {
    if value.is_null() {
        return Ok(Value::Null);
    }
    let map = value.as_object().ok_or(FAILED)?;
    if map.len() != 4 || map.keys().any(|k| !["x", "y", "width", "height"].contains(&k.as_str())) {
        return Err(FAILED);
    }
    let number = |key: &str| -> Result<f64, &'static str> {
        map.get(key)
            .and_then(Value::as_f64)
            .filter(|n| n.is_finite())
            .ok_or(FAILED)
    };
    let (x, y, w, h) = (number("x")?, number("y")?, number("width")?, number("height")?);
    if x < 0.0 || y < 0.0 || w <= 0.0 || h <= 0.0 || x + w > vw || y + h > vh {
        return Err(FAILED);
    }
    Ok(json!({"x":x,"y":y,"width":w,"height":h}))
}

fn sanitize_controls(value: &Value) -> Result<Value, &'static str> {
    const KEYS: [&str; 7] = [
        "viewport",
        "reject_all_present",
        "play",
        "settings",
        "quality",
        "p720",
        "reject_all",
    ];
    let map = value.as_object().ok_or(FAILED)?;
    if map.len() != KEYS.len() || map.keys().any(|k| !KEYS.contains(&k.as_str())) {
        return Err(FAILED);
    }
    let viewport = map.get("viewport").and_then(Value::as_object).ok_or(FAILED)?;
    if viewport.len() != 2 || viewport.keys().any(|k| !["width", "height"].contains(&k.as_str())) {
        return Err(FAILED);
    }
    let dimension = |key: &str| -> Result<f64, &'static str> {
        viewport
            .get(key)
            .and_then(Value::as_f64)
            .filter(|n| n.is_finite() && (1.0..=16384.0).contains(n))
            .ok_or(FAILED)
    };
    let (vw, vh) = (dimension("width")?, dimension("height")?);
    let present = map
        .get("reject_all_present")
        .and_then(Value::as_bool)
        .ok_or(FAILED)?;
    let mut clean = serde_json::Map::new();
    clean.insert("viewport".into(), json!({"width":vw,"height":vh}));
    clean.insert("reject_all_present".into(), json!(present));
    for key in ["play", "settings", "quality", "p720", "reject_all"] {
        let rect = rect(map.get(key).ok_or(FAILED)?, vw, vh)?;
        clean.insert(key.into(), rect);
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
        && watch_id(document).is_some()
}
wrap_dev_tools_message_observer! {
 struct Observer{identity:i32,frame_id:String,document:String,message:i32,state:Arc<Mutex<Completion>>}
 impl DevToolsMessageObserver{
  fn on_dev_tools_method_result(&self,browser:Option<&mut Browser>,message_id:i32,success:i32,result:Option<&[u8]>){
   if message_id!=self.message{return;}
   let matches=browser.is_some_and(|b|document_matches(b,self.identity,&self.frame_id,&self.document));
   let Ok(mut state)=self.state.lock()else{return;};if state.done{return;}
   state.done=true;
   state.reply=Some(if success==0{Err("cdp_method_failed")}else if !matches{Err("probe_document_changed")}else{result.ok_or("cdp_result_absent").and_then(|v|decode(v))});
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
    pub fn start(
        browser: &Browser,
        expected: &crate::youtube_light::VideoId,
    ) -> Result<Self, &'static str> {
        if !crate::youtube_benchmark_fixture::allowed()
        {
            return Err("reference_fixture_only");
        }
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
        if document.len() > crate::youtube_light::MAX_INPUT_URL_BYTES
            || frame_id.is_empty()
            || frame_id.len() > 256
            || watch_id(&document).as_ref() != Some(expected)
        {
            return Err("reference_document_not_admitted");
        }
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
        if !document_matches(browser, identity, &pending.frame_id, &pending.document) {
            return Err("reference_document_changed");
        }
        if !crate::cdp::send(
            browser,
            message,
            "Runtime.evaluate",
            json!({"expression":SCRIPT,"returnByValue":true,"silent":true}),
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
    fn sample() -> Value {
        json!({"volume":1.0,"muted":false,"default_sink":true,"playing":true,"paused":false,"ended":false,"seeking":false,"ready_state":4,"position":30.5,"duration":600,"video_width":1280,"video_height":720,"decoded":900,"dropped":2,"playback_rate":1,"video_count":1})
    }
    fn wire(value: Value) -> Vec<u8> {
        serde_json::to_vec(&json!({"result":{"type":"object","value":value}})).unwrap()
    }
    #[test]
    fn returns_exact_numeric_public_player_schema() {
        let clean = decode(&wire(sample())).unwrap();
        // 16 прежних полей + честный controls:null для старого fixture.
        assert_eq!(clean.as_object().unwrap().len(), 17);
        assert_eq!(clean["decoded"], 900);
        assert!(clean["controls"].is_null());
    }
    fn controls_fixture() -> Value {
        json!({"viewport":{"width":854,"height":480},"reject_all_present":true,
            "play":{"x":12.0,"y":430.0,"width":48.0,"height":48.0},
            "settings":{"x":700.0,"y":430.0,"width":48.0,"height":48.0},
            "quality":null,"p720":null,
            "reject_all":{"x":300.0,"y":240.0,"width":120.0,"height":36.0}})
    }
    #[test]
    fn controls_valid_fixed_rects_pass_and_missing_stay_null() {
        let mut data = sample();
        data["controls"] = controls_fixture();
        let clean = decode(&wire(data)).unwrap();
        let controls = &clean["controls"];
        assert_eq!(controls["reject_all_present"], true);
        assert_eq!(controls["play"]["width"], 48.0);
        assert!(controls["quality"].is_null() && controls["p720"].is_null());
        assert_eq!(controls.as_object().unwrap().len(), 7);
    }
    #[test]
    fn controls_reject_left_keys_bad_bool_bad_geometry_and_overflow() {
        for mutate in [
            Box::new(|c: &mut Value| c["extra"] = json!(1)) as Box<dyn Fn(&mut Value)>,
            Box::new(|c: &mut Value| c["reject_all_present"] = json!("yes")),
            Box::new(|c: &mut Value| c["viewport"]["width"] = json!(0)),
            Box::new(|c: &mut Value| c["viewport"]["height"] = json!(20000)),
            Box::new(|c: &mut Value| c["play"]["width"] = json!(0)),
            Box::new(|c: &mut Value| c["play"]["x"] = json!(-1)),
            Box::new(|c: &mut Value| c["play"]["x"] = json!(850.0)), // x+w > vw
            Box::new(|c: &mut Value| c["reject_all"]["y"] = json!(470.0)), // y+h > vh
            Box::new(|c: &mut Value| c["quality"] = json!({"x":1,"y":1,"width":10,"height":10,"text":"720p"})),
            Box::new(|c: &mut Value| c["p720"] = json!("720p")),
        ] {
            let mut data = sample();
            let mut controls = controls_fixture();
            mutate(&mut controls);
            data["controls"] = controls;
            assert_eq!(decode(&wire(data)).unwrap_err(), FAILED);
        }
        // NaN-подобное через 1e999 в координате
        let mut data = sample();
        data["controls"] = controls_fixture();
        let raw = serde_json::to_string(&json!({"result":{"type":"object","value":data}}))
            .unwrap()
            .replace(""x":12.0", ""x":1e999");
        assert!(decode(raw.as_bytes()).is_err());
    }
    #[test]
    fn refuses_extras_urls_nonfinite_and_wrong_types() {
        for (field, value) in [
            ("url", json!("https://private/?sig=secret")),
            ("position", json!("https://private")),
            ("position", json!(-1)),
            ("duration", json!(604801)),
            ("video_count", json!(0)),
            ("video_count", json!(2)),
            ("decoded", json!(9007199254740992u64)),
            ("ready_state", json!(5)),
            ("paused", json!(true)),
            ("playback_rate", json!(17)),
        ] {
            let mut data = sample();
            data[field] = value;
            assert_eq!(decode(&wire(data)).unwrap_err(), FAILED);
        }
        assert!(decode(br#"{"result":{"type":"object","value":{"position":1e999}}}"#).is_err());
        assert!(decode(&vec![0; MAX_BYTES + 1]).is_err());
        assert!(decode(&serde_json::to_vec(&json!({"result":{"type":"object","value":sample()},"exceptionDetails":{"text":"secret"}})).unwrap()).is_err());
    }
    #[test]
    fn only_public_watch_ids_not_other_paths_or_ambiguous_queries() {
        assert_eq!(
            watch_id("https://www.youtube.com/watch?v=BaW_jenozKc")
                .unwrap()
                .as_str(),
            "BaW_jenozKc"
        );
        for url in [
            "https://www.youtube.com/shorts/BaW_jenozKc",
            "https://www.youtube.com.evil/watch?v=BaW_jenozKc",
            "https://www.youtube.com/watch?v=BaW_jenozKc&v=abcdefghijk",
            "http://www.youtube.com/watch?v=BaW_jenozKc",
        ] {
            assert!(watch_id(url).is_none());
        }
    }
}
