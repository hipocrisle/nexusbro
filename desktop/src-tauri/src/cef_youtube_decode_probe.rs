//! Prepared GHA-only decode diagnostics, NOT physical GPU activity evidence.
//! Chromium156 primary sources: media/base/media_serializers.h (VideoDecoderConfig),
//! media/base/decoder.{h,cc}, core/inspector/inspector_media_agent.cc, and
//! content/renderer/media/inspector_media_event_handler.cc at 156.0.8078.88.
//! Owns this native CDP session's Media domain; do not overlap another native
//! Media consumer. Final current-video backend-node revalidation is point-in-time,
//! not continuous pipeline identity. No playback changes, network or private log output.
use crate::youtube_light::VideoId;
use cef::*;
use serde::Serialize;
use serde_json::{json, Value};
use std::{
    cell::RefCell,
    collections::HashSet,
    marker::PhantomData,
    rc::Rc as ThreadBound,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use zeroize::{Zeroize, Zeroizing};
const CAP: usize = 16 * 1024;
const DEADLINE: Duration = Duration::from_secs(5);
const SCRIPT: &str =
    "(()=>{const v=document.querySelectorAll('video');return v.length===1?v[0]:null})()";
type Reply = Result<Value, &'static str>;
thread_local! {static ACTIVE:RefCell<HashSet<i32>>=RefCell::new(HashSet::new());}
struct Lease(i32);
impl Lease {
    fn take(id: i32) -> Result<Self, &'static str> {
        ACTIVE.with(|set| {
            let mut set = set.borrow_mut();
            if set.len() >= 4 || !set.insert(id) {
                Err("decode_probe_busy")
            } else {
                Ok(Self(id))
            }
        })
    }
}
impl Drop for Lease {
    fn drop(&mut self) {
        ACTIVE.with(|set| {
            set.borrow_mut().remove(&self.0);
        });
    }
}
fn wipe(value: &mut Value) {
    match value {
        Value::String(v) => v.zeroize(),
        Value::Array(a) => a.iter_mut().for_each(wipe),
        Value::Object(m) => {
            for (mut k, mut v) in std::mem::take(m) {
                k.zeroize();
                wipe(&mut v);
            }
        }
        _ => {}
    }
}
struct Clear(Value);
impl Drop for Clear {
    fn drop(&mut self) {
        wipe(&mut self.0);
    }
}
fn parse(bytes: &[u8]) -> Result<Clear, &'static str> {
    if bytes.is_empty() || bytes.len() > CAP {
        return Err("decode_payload_bound");
    }
    let raw = Zeroizing::new(bytes.to_vec());
    Ok(Clear(
        serde_json::from_slice(&raw).map_err(|_| "decode_payload_invalid")?,
    ))
}
#[derive(Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum Codec {
    Vp8,
    Vp9,
    Av1,
    H264,
    Hevc,
}
#[derive(Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum Decoder {
    Ffmpeg,
    Vpx,
    Aom,
    Dav1d,
    Mojo,
    Decrypting,
    D3d11,
    Vaapi,
    Broker,
    Vda,
    V4l2,
    OutOfProcess,
    VideoToolbox,
}
fn codec(value: &str) -> Option<Codec> {
    Some(match value {
        "vp8" => Codec::Vp8,
        "vp9" => Codec::Vp9,
        "av1" => Codec::Av1,
        "h264" => Codec::H264,
        "hevc" => Codec::Hevc,
        _ => return None,
    })
}
fn decoder(value: &str) -> Option<Decoder> {
    Some(match value {
        "FFmpegVideoDecoder" => Decoder::Ffmpeg,
        "VpxVideoDecoder" => Decoder::Vpx,
        "AomVideoDecoder" => Decoder::Aom,
        "Dav1dVideoDecoder" => Decoder::Dav1d,
        "MojoVideoDecoder" => Decoder::Mojo,
        "DecryptingVideoDecoder" => Decoder::Decrypting,
        "D3D11VideoDecoder" => Decoder::D3d11,
        "VaapiVideoDecoder" => Decoder::Vaapi,
        "VideoDecoderBroker" => Decoder::Broker,
        "VDAVideoDecoder" => Decoder::Vda,
        "V4L2VideoDecoder" => Decoder::V4l2,
        "OOPVideoDecoder" => Decoder::OutOfProcess,
        "VideoToolboxVideoDecoder" => Decoder::VideoToolbox,
        _ => return None,
    })
}
#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum Mode {
    Software,
    Platform,
}
#[derive(Serialize)]
struct Evidence {
    codec: Codec,
    decoder: Decoder,
    platform_decoder: bool,
    decode_mode: Mode,
    hardware_activity_proven: bool,
    video_pipelines: u8,
}
struct Player {
    id: Zeroizing<String>,
    node: Option<i64>,
    destroyed: bool,
    decoder_unknown: bool,
    codec: Option<Codec>,
    decoder: Option<Decoder>,
    platform: Option<bool>,
    tracks: Option<usize>,
}
#[derive(Default)]
struct State {
    done: bool,
    error: Option<&'static str>,
    enabled: bool,
    object: Option<Zeroizing<String>>,
    described: bool,
    node: Option<i64>,
    final_started: bool,
    final_object: Option<Zeroizing<String>>,
    final_described: bool,
    final_node: Option<i64>,
    players: Vec<Player>,
    destroyed_total: u8,
}
impl State {
    fn player(&mut self, id: &str) -> Result<&mut Player, &'static str> {
        if id.is_empty() || id.len() > 256 {
            return Err("decode_player_id_invalid");
        }
        if let Some(index) = self.players.iter().position(|p| p.id.as_str() == id) {
            return Ok(&mut self.players[index]);
        }
        if self.players.len() >= 4 {
            // Keep the original lifetime bound, including terminal records.
            // A destroyed record cannot grant an unbounded stream of new IDs.
            return Err("decode_players_bound");
        }
        self.players.push(Player {
            id: Zeroizing::new(id.to_owned()),
            node: None,
            destroyed: false,
            decoder_unknown: false,
            codec: None,
            decoder: None,
            platform: None,
            tracks: None,
        });
        Ok(self.players.last_mut().unwrap())
    }
    fn ambiguity(&self) -> bool {
        let Some(node) = self.node else {
            return false;
        };
        self.players
            .iter()
            .filter(|p| p.node == Some(node) && !p.destroyed)
            .count()
            > 1
    }
    fn event(&mut self, method: &str, bytes: &[u8]) -> Result<(), &'static str> {
        let tree = parse(bytes)?;
        let value = &tree.0;
        if method == "Media.playerCreated" {
            let player = &value["player"];
            let id = player["playerId"].as_str().ok_or("decode_player_invalid")?;
            let p = self.player(id)?;
            if let Some(node) = player.get("domNodeId") {
                let node = node
                    .as_i64()
                    .filter(|n| *n > 0 && *n <= i32::MAX as i64)
                    .ok_or("decode_node_invalid")?;
                if p.node.is_some_and(|old| old != node) {
                    return Err("decode_node_changed");
                }
                p.node = Some(node);
            }
        } else if method == "Media.playerEventsAdded" {
            let id = value["playerId"].as_str().ok_or("decode_player_invalid")?;
            let events = value["events"]
                .as_array()
                .filter(|a| a.len() <= 64)
                .ok_or("decode_events_bound")?;
            let mut mark = false;
            {
                let p = self.player(id)?;
                for event in events {
                    // Pinned 156.0.8078.88 wire form: PlayerEvent{timestamp,
                    // value} (Media.pdl:41-45); value is the JSON string of the
                    // media log params dict (inspector_media_event_handler.cc:
                    // ToString(event.params) via base::WriteJson), whose
                    // "event" key carries the event name (media_log.cc
                    // kEventKey). Only the authoritative terminal name is
                    // consumed; the exact key is parsed, never
                    // substring-matched against URLs or error text.
                    let raw = event["value"]
                        .as_str()
                        .filter(|v| v.len() <= CAP)
                        .ok_or("decode_event_invalid")?;
                    let params = parse(raw.as_bytes())?;
                    let name = params.0["event"]
                        .as_str()
                        .filter(|n| !n.is_empty() && n.len() <= 64)
                        .ok_or("decode_event_invalid")?;
                    if name == "kWebMediaPlayerDestroyed" && !p.destroyed {
                        p.destroyed = true;
                        mark = true;
                    }
                }
            }
            if mark {
                self.destroyed_total = self.destroyed_total.saturating_add(1);
            }
        } else if method == "Media.playerPropertiesChanged" {
            let id = value["playerId"].as_str().ok_or("decode_player_invalid")?;
            let properties = value["properties"]
                .as_array()
                .filter(|a| a.len() <= 64)
                .ok_or("decode_properties_bound")?;
            let p = self.player(id)?;
            for property in properties {
                let name = property["name"].as_str().ok_or("decode_property_invalid")?;
                // Ignore unknown/private properties without copying any value.
                if !matches!(
                    name,
                    "kVideoTracks" | "kVideoDecoderName" | "kIsPlatformVideoDecoder"
                ) {
                    continue;
                }
                let Some(raw) = property.get("value").filter(|v| !v.is_null()) else {
                    match name {
                        "kVideoTracks" => {
                            p.codec = None;
                            p.tracks = None;
                        }
                        "kVideoDecoderName" => {
                            p.decoder = None;
                            p.decoder_unknown = false;
                        }
                        _ => p.platform = None,
                    }
                    continue;
                };
                let raw = raw
                    .as_str()
                    .filter(|v| v.len() <= 8192)
                    .ok_or("decode_property_invalid")?;
                match name {
                    "kVideoDecoderName" => {
                        // Неизвестное имя не обрывает probe: факт живёт в
                        // per-player флаге, ошибка — только для целевого
                        // player на этапе evidence. Сырое значение не копируется.
                        match decoder(raw) {
                            Some(kind) => {
                                p.decoder = Some(kind);
                                p.decoder_unknown = false;
                            }
                            None => {
                                p.decoder = None;
                                p.decoder_unknown = true;
                            }
                        }
                    }
                    "kIsPlatformVideoDecoder" => {
                        p.platform = Some(match raw {
                            "true" => true,
                            "false" => false,
                            _ => return Err("decode_platform_unknown"),
                        });
                    }
                    _ => {
                        let tracks = parse(raw.as_bytes())?;
                        let list = tracks
                            .0
                            .as_array()
                            .filter(|v| v.len() <= 1)
                            .ok_or("decode_tracks_ambiguous")?;
                        p.tracks = Some(list.len());
                        p.codec = if let Some(track) = list.first() {
                            Some(
                                codec(track["codec"].as_str().ok_or("decode_codec_unknown")?)
                                    .ok_or("decode_codec_unknown")?,
                            )
                        } else {
                            None
                        };
                    }
                }
            }
        }
        Ok(())
    }
    fn verified_evidence(&self) -> Result<Option<Value>, &'static str> {
        let Some(final_node) = self.final_node else {
            return Ok(None);
        };
        if self.node != Some(final_node) {
            return Err("decode_video_node_changed");
        }
        self.evidence()
    }
    fn evidence(&self) -> Result<Option<Value>, &'static str> {
        if !self.enabled {
            return Ok(None);
        }
        let Some(node) = self.node else {
            return Ok(None);
        };
        let candidates: Vec<_> = self
            .players
            .iter()
            .filter(|p| p.node == Some(node) && !p.destroyed)
            .collect();
        if candidates.len() > 1 {
            // A quality switch recreates the player on the same DOM node; the
            // old one is authoritatively destroyed via Media.playerEventsAdded
            // (possibly still in flight after the enable() history replay).
            // Wait bounded by the probe deadline instead of failing instantly;
            // poll converts a persistent ambiguity into the typed refusal.
            return Ok(None);
        }
        let Some(p) = candidates.first() else {
            return Ok(None);
        };
        if p.decoder_unknown {
            return Err("decode_decoder_unknown");
        }
        let (Some(codec), Some(decoder), Some(platform), Some(tracks)) =
            (p.codec, p.decoder, p.platform, p.tracks)
        else {
            return Ok(None);
        };
        if tracks != 1 {
            return Err("decode_video_track_missing");
        }
        let mode = if platform {
            if matches!(
                decoder,
                Decoder::Ffmpeg | Decoder::Vpx | Decoder::Aom | Decoder::Dav1d
            ) {
                return Err("decode_mode_inconsistent");
            }
            Mode::Platform
        } else if matches!(
            decoder,
            Decoder::Ffmpeg | Decoder::Vpx | Decoder::Aom | Decoder::Dav1d
        ) {
            Mode::Software
        } else {
            return Err("decode_mode_unknown");
        };
        serde_json::to_value(Evidence {
            codec,
            decoder,
            platform_decoder: platform,
            decode_mode: mode,
            hardware_activity_proven: false,
            video_pipelines: 1,
        })
        .map(Some)
        .map_err(|_| "decode_result_invalid")
    }
}
fn admitted(
    browser: &Browser,
    identity: i32,
    frame_id: &str,
    document: &str,
    expected: &VideoId,
) -> bool {
    if browser.identifier() != identity || browser.is_valid() == 0 {
        return false;
    }
    let Some(frame) = browser.main_frame() else {
        return false;
    };
    if frame.is_main() == 0
        || CefString::from(&frame.identifier()).to_string() != frame_id
        || CefString::from(&frame.url()).to_string() != document
    {
        return false;
    }
    if let Ok(url) = reqwest::Url::parse(document) {
        if url.path() == "/watch" && VideoId::from_url(document).as_ref() == Ok(expected) {
            return true;
        }
    }
    crate::cef_youtube::privacy_binding(identity, frame_id, document).is_some()
        && crate::cef_youtube::logical_url(identity, document)
            .and_then(|u| VideoId::from_url(&u).ok())
            .as_ref()
            == Some(expected)
}
wrap_dev_tools_message_observer! {
 struct Observer{identity:i32,frame_id:String,document:String,expected:VideoId,base:i32,state:Arc<Mutex<State>>}
 impl DevToolsMessageObserver{
  fn on_dev_tools_method_result(&self,browser:Option<&mut Browser>,message_id:i32,success:i32,result:Option<&[u8]>){
   if ![self.base,self.base+1,self.base+2,self.base+5,self.base+6].contains(&message_id){return;}
   let valid=browser.is_some_and(|b|admitted(b,self.identity,&self.frame_id,&self.document,&self.expected));
   let Ok(mut state)=self.state.lock()else{return};if state.done{return;}
   if !valid{state.error=Some("decode_document_changed");return;}if success==0{state.error=Some("decode_cdp_method_failed");return;}
   if message_id==self.base{state.enabled=true;return;} // Successful void methods may have no JSON result.
   let parsed=(||->Result<(),&'static str>{let tree=parse(result.ok_or("decode_cdp_result_absent")?)?;
    if message_id==self.base+1||message_id==self.base+5{
     let value=&tree.0["result"];
     if tree.0.get("exceptionDetails").is_some()||value["type"]!="object"||value["subtype"]!="node"||value["className"]!="HTMLVideoElement"{return Err("decode_video_node_missing");}
     let object=value["objectId"].as_str().filter(|v|!v.is_empty()&&v.len()<=512).ok_or("decode_object_invalid")?;if message_id==self.base+1{state.object=Some(Zeroizing::new(object.to_owned()));}else{state.final_object=Some(Zeroizing::new(object.to_owned()));}
    }else{let node=&tree.0["node"];if node["nodeName"]!="VIDEO"{return Err("decode_video_node_missing");}let id=node["backendNodeId"].as_i64().filter(|n|*n>0&&*n<=i32::MAX as i64).ok_or("decode_node_invalid")?;if message_id==self.base+2{state.node=Some(id);}else{if state.node!=Some(id){return Err("decode_video_node_changed");}state.final_node=Some(id);}}Ok(())})();
   if let Err(code)=parsed{state.error=Some(code);}
  }
  fn on_dev_tools_event(&self,browser:Option<&mut Browser>,method:Option<&CefString>,params:Option<&[u8]>){
   let Some(method)=method.map(|m|m.to_string()).filter(|m|matches!(m.as_str(),"Media.playerCreated"|"Media.playerPropertiesChanged"|"Media.playerEventsAdded"))else{return};
   let valid=browser.is_some_and(|b|admitted(b,self.identity,&self.frame_id,&self.document,&self.expected));
   let Ok(mut state)=self.state.lock()else{return};if state.done{return;}
   if !valid{state.error=Some("decode_document_changed");return;}
   if let Err(code)=params.ok_or("decode_event_absent").and_then(|bytes|state.event(&method,bytes)){state.error=Some(code);}
  }
  fn on_dev_tools_agent_detached(&self,browser:Option<&mut Browser>){if browser.is_some_and(|b|b.identifier()!=self.identity){return;}if let Ok(mut state)=self.state.lock(){if !state.done{state.error=Some("decode_agent_detached");}}}
 }
}
/// UI-thread-bound; exact per-browser exclusive Media-session lease. Drop on
/// navigation/lock/close/shutdown. Unknown/missing/ambiguous data is an error.
pub struct Probe {
    browser: Browser,
    identity: i32,
    frame: String,
    document: String,
    expected: VideoId,
    base: i32,
    group: String,
    state: Arc<Mutex<State>>,
    registration: Option<Registration>,
    started: Instant,
    consumed: bool,
    domain_started: bool,
    _lease: Lease,
    _ui: PhantomData<ThreadBound<()>>,
}
impl Probe {
    pub fn start(browser: &Browser, expected: &VideoId) -> Result<Self, &'static str> {
        if !crate::youtube_benchmark_fixture::allowed() {
            return Err("decode_fixture_only");
        }
        if currently_on(ThreadId::UI) == 0 {
            return Err("decode_wrong_thread");
        }
        let identity = browser.identifier();
        let frame = browser.main_frame().ok_or("decode_frame_missing")?;
        let id = CefString::from(&frame.identifier()).to_string();
        let doc = CefString::from(&frame.url()).to_string();
        if id.is_empty()
            || id.len() > 256
            || doc.len() > 2048
            || !admitted(browser, identity, &id, &doc, expected)
        {
            return Err("decode_document_not_admitted");
        }
        let lease = Lease::take(identity)?;
        let base = crate::cdp::reserve(7).ok_or("decode_message_bound")?;
        let state = Arc::new(Mutex::new(State::default()));
        let registration = browser
            .host()
            .and_then(|h| {
                h.add_dev_tools_message_observer(Some(&mut Observer::new(
                    identity,
                    id.clone(),
                    doc.clone(),
                    expected.clone(),
                    base,
                    state.clone(),
                )))
            })
            .ok_or("decode_observer_unavailable")?;
        let mut probe = Self {
            browser: browser.clone(),
            identity,
            frame: id,
            document: doc,
            expected: expected.clone(),
            base,
            group: format!("nb-decode-{base}"),
            state,
            registration: Some(registration),
            started: Instant::now(),
            consumed: false,
            domain_started: false,
            _lease: lease,
            _ui: PhantomData,
        };
        if !admitted(browser, identity, &probe.frame, &probe.document, expected) {
            return Err("decode_document_changed");
        }
        probe.domain_started = true;
        if !crate::cdp::send(browser, base, "Media.enable", json!({}))
            || !crate::cdp::send(
                browser,
                base + 1,
                "Runtime.evaluate",
                json!({"expression":SCRIPT,"objectGroup":probe.group,"silent":true,"returnByValue":false}),
            )
        {
            return Err("decode_dispatch_failed");
        }
        Ok(probe)
    }
    pub fn poll(&mut self) -> Option<Reply> {
        if self.consumed {
            return None;
        }
        if currently_on(ThreadId::UI) == 0 {
            return self.finish(Err("decode_wrong_thread"));
        }
        if !admitted(
            &self.browser,
            self.identity,
            &self.frame,
            &self.document,
            &self.expected,
        ) {
            return self.finish(Err("decode_document_changed"));
        }
        if self.started.elapsed() >= DEADLINE {
            let ambiguous = self.state.lock().map(|s| s.ambiguity()).unwrap_or(false);
            return self.finish(Err(if ambiguous {
                "decode_pipeline_ambiguous"
            } else {
                "decode_timeout"
            }));
        }
        let mut object = None;
        let mut final_evaluate = false;
        let mut final_object = None;
        let result = match self.state.lock() {
            Ok(mut state) => {
                if let Some(error) = state.error {
                    Some(Err(error))
                } else {
                    if !state.described {
                        if let Some(id) = state.object.as_ref() {
                            object = Some(id.clone());
                            state.described = true;
                        }
                    }
                    if !state.final_described {
                        if let Some(id) = state.final_object.as_ref() {
                            final_object = Some(id.clone());
                            state.final_described = true;
                        }
                    }
                    match state.evidence() {
                        Ok(Some(_)) => {
                            if !state.final_started {
                                state.final_started = true;
                                final_evaluate = true;
                            }
                            match state.verified_evidence() {
                                Ok(value) => value.map(Ok),
                                Err(error) => Some(Err(error)),
                            }
                        }
                        Ok(None) => None,
                        Err(error) => Some(Err(error)),
                    }
                }
            }
            Err(_) => Some(Err("decode_state_unavailable")),
        };
        if let Some(result) = result {
            return self.finish(result);
        }
        // Re-query the one CURRENT video element, then bind its backend node
        // immediately before emitting. All CEF sends happen after guard drop.
        if final_evaluate
            && !crate::cdp::send(
                &self.browser,
                self.base + 5,
                "Runtime.evaluate",
                json!({"expression":SCRIPT,"objectGroup":self.group,"silent":true,"returnByValue":false}),
            )
        {
            return self.finish(Err("decode_dispatch_failed"));
        }
        if let Some(final_object) = final_object {
            if !crate::cdp::send(
                &self.browser,
                self.base + 6,
                "DOM.describeNode",
                json!({"objectId":final_object.as_str(),"depth":0,"pierce":false}),
            ) {
                return self.finish(Err("decode_dispatch_failed"));
            }
        }
        if let Some(object) = object {
            if !crate::cdp::send(
                &self.browser,
                self.base + 2,
                "DOM.describeNode",
                json!({"objectId":object.as_str(),"depth":0,"pierce":false}),
            ) {
                return self.finish(Err("decode_dispatch_failed"));
            }
        }
        None
    }
    /// Safe typed refusal diagnostics for the owner: counts and bools only,
    /// never player ids, event strings, URLs, or page data.
    pub fn diagnostic(&self) -> Value {
        let Ok(state) = self.state.lock() else {
            return json!({"stage":"state_unavailable"});
        };
        let live = state
            .node
            .map(|node| {
                state
                    .players
                    .iter()
                    .filter(|p| p.node == Some(node) && !p.destroyed)
                    .count()
            })
            .unwrap_or(0);
        json!({
            "players_total": state.players.len(),
            "live_for_node": live,
            "destroyed_known": state.destroyed_total,
            "node_known": state.node.is_some(),
            "enabled": state.enabled,
            "final_started": state.final_started,
            "consumed": self.consumed,
        })
    }
    fn finish(&mut self, result: Reply) -> Option<Reply> {
        self.consumed = true;
        self.cleanup();
        Some(result)
    }
    fn cleanup(&mut self) {
        if let Ok(mut state) = self.state.lock() {
            state.done = true;
            state.object.take();
            state.final_object.take();
        }
        // Never call CEF under state/grant mutexes. This session is exclusively ours.
        if currently_on(ThreadId::UI) != 0 {
            if self.domain_started {
                let _ = crate::cdp::send(&self.browser, self.base + 3, "Media.disable", json!({}));
                self.domain_started = false;
            }
            let _ = crate::cdp::send(
                &self.browser,
                self.base + 4,
                "Runtime.releaseObjectGroup",
                json!({"objectGroup":self.group}),
            );
        }
        self.registration.take();
    }
}
impl Drop for Probe {
    fn drop(&mut self) {
        if !self.consumed {
            self.cleanup();
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn event(state: &mut State, name: &str, value: Value) -> Result<(), &'static str> {
        state.event(name, &serde_json::to_vec(&value).unwrap())
    }
    fn software() -> State {
        let mut s = State {
            enabled: true,
            node: Some(5),
            ..Default::default()
        };
        event(
            &mut s,
            "Media.playerCreated",
            json!({"player":{"playerId":"opaque","domNodeId":5}}),
        )
        .unwrap();
        event(&mut s,"Media.playerPropertiesChanged",json!({"playerId":"opaque","properties":[{"name":"kVideoTracks","value":"[{\"codec\":\"vp9\"}]"},{"name":"kVideoDecoderName","value":"VpxVideoDecoder"},{"name":"kIsPlatformVideoDecoder","value":"false"},{"name":"kFrameUrl","value":"https://private/?signed=never-return"}]})).unwrap();
        s
    }
    #[test]
    fn exact_dom_node_codec_and_explicit_software_not_gpu_claim() {
        let s = software();
        let result = s.evidence().unwrap().unwrap();
        assert_eq!(result["codec"], "vp9");
        assert_eq!(result["decode_mode"], "software");
        assert_eq!(result["hardware_activity_proven"], false);
        assert!(!result.to_string().contains("private"));
        assert_eq!(result.as_object().unwrap().len(), 6);
    }
    #[test]
    fn final_current_video_binding_is_required_and_changed_node_refuses() {
        let mut state = software();
        assert!(state.evidence().unwrap().is_some());
        assert!(state.verified_evidence().unwrap().is_none());
        state.final_node = Some(5);
        assert!(state.verified_evidence().unwrap().is_some());
        state.final_node = Some(6);
        assert_eq!(
            state.verified_evidence().unwrap_err(),
            "decode_video_node_changed"
        );
        state.final_node = None;
        assert!(state.verified_evidence().unwrap().is_none());
    }
    #[test]
    fn absent_or_mismatched_nodes_and_unknown_properties_never_pass() {
        let mut s = software();
        s.node = Some(99);
        assert!(s.evidence().unwrap().is_none());
        s.node = Some(5);
        s.players[0].platform = None;
        assert!(s.evidence().unwrap().is_none());
        assert!(event(&mut s,"Media.playerPropertiesChanged",json!({"playerId":"opaque","properties":[{"name":"kVideoDecoderName","value":"secret-unlisted-decoder"}]})).is_ok());
        assert_eq!(s.evidence().unwrap_err(), "decode_decoder_unknown");
    }
    #[test]
    fn dav1d_is_software_and_platform_true_is_inconsistent() {
        let mut s = State {
            enabled: true,
            node: Some(5),
            ..Default::default()
        };
        event(
            &mut s,
            "Media.playerCreated",
            json!({"player":{"playerId":"opaque","domNodeId":5}}),
        )
        .unwrap();
        event(&mut s,"Media.playerPropertiesChanged",json!({"playerId":"opaque","properties":[{"name":"kVideoTracks","value":"[{\"codec\":\"av1\"}]"},{"name":"kVideoDecoderName","value":"Dav1dVideoDecoder"},{"name":"kIsPlatformVideoDecoder","value":"false"}]})).unwrap();
        let result = s.evidence().unwrap().unwrap();
        assert_eq!(result["decoder"], "dav1d");
        assert_eq!(result["codec"], "av1");
        assert_eq!(result["decode_mode"], "software");
        assert_eq!(result["hardware_activity_proven"], false);
        s.players[0].platform = Some(true);
        assert_eq!(s.evidence().unwrap_err(), "decode_mode_inconsistent");
    }
    #[test]
    fn foreign_unknown_decoder_does_not_block_target() {
        let mut s = software();
        event(
            &mut s,
            "Media.playerCreated",
            json!({"player":{"playerId":"ad","domNodeId":99}}),
        )
        .unwrap();
        assert!(event(&mut s,"Media.playerPropertiesChanged",json!({"playerId":"ad","properties":[{"name":"kVideoDecoderName","value":"secret-unlisted-decoder"}]})).is_ok());
        let result = s.evidence().unwrap().unwrap();
        assert_eq!(result["decoder"], "vpx");
        assert_eq!(result["decode_mode"], "software");
    }
    #[test]
    fn unknown_then_known_or_null_leaves_no_stale_receipt() {
        let mut s = software();
        event(&mut s,"Media.playerPropertiesChanged",json!({"playerId":"opaque","properties":[{"name":"kVideoDecoderName","value":"secret-unlisted-decoder"}]})).unwrap();
        assert_eq!(s.evidence().unwrap_err(), "decode_decoder_unknown");
        event(&mut s,"Media.playerPropertiesChanged",json!({"playerId":"opaque","properties":[{"name":"kVideoDecoderName","value":"VpxVideoDecoder"}]})).unwrap();
        assert_eq!(s.evidence().unwrap().unwrap()["decoder"], "vpx");
        event(
            &mut s,
            "Media.playerPropertiesChanged",
            json!({"playerId":"opaque","properties":[{"name":"kVideoDecoderName","value":null}]}),
        )
        .unwrap();
        assert!(s.evidence().unwrap().is_none());
    }
    #[test]
    fn ambiguous_players_overflow_and_unsupported_tracks_refuse() {
        let mut s = software();
        event(
            &mut s,
            "Media.playerCreated",
            json!({"player":{"playerId":"second","domNodeId":5}}),
        )
        .unwrap();
        // Two LIVE players on the same node: bounded wait (Ok(None)) with the
        // ambiguity flag set; the typed refusal happens at the probe deadline.
        assert!(s.evidence().unwrap().is_none());
        assert!(s.ambiguity());
        for n in 2..4 {
            event(
                &mut s,
                "Media.playerCreated",
                json!({"player":{"playerId":format!("p{n}"),"domNodeId":99+n}}),
            )
            .unwrap();
        }
        assert!(event(
            &mut s,
            "Media.playerCreated",
            json!({"player":{"playerId":"overflow","domNodeId":9}})
        )
        .is_err());
        assert!(parse(&vec![0; CAP + 1]).is_err());
        let mut s = software();
        assert!(event(&mut s,"Media.playerPropertiesChanged",json!({"playerId":"opaque","properties":[{"name":"kVideoTracks","value":"[{\"codec\":\"vp9\"},{\"codec\":\"h264\"}]"}]})).is_err());
    }
    #[test]
    fn platform_name_is_not_physical_gpu_activity() {
        let mut s = software();
        s.players[0].platform = Some(true);
        s.players[0].decoder = Some(Decoder::D3d11);
        let result = s.evidence().unwrap().unwrap();
        assert_eq!(result["decode_mode"], "platform");
        assert_eq!(result["hardware_activity_proven"], false);
        s.players[0].platform = Some(false);
        assert!(s.evidence().is_err());
    }
    #[test]
    fn exclusive_lease_is_bounded_and_releases_without_native_work() {
        let first = Lease::take(991).unwrap();
        assert!(Lease::take(991).is_err());
        let second = Lease::take(992).unwrap();
        let third = Lease::take(993).unwrap();
        let fourth = Lease::take(994).unwrap();
        assert!(Lease::take(995).is_err());
        drop(first);
        assert!(Lease::take(995).is_ok());
        drop((second, third, fourth));
    }
    fn destroyed_event(state: &mut State, id: &str) -> Result<(), &'static str> {
        event(
            state,
            "Media.playerEventsAdded",
            json!({"playerId":id,"events":[
                {"timestamp":1.0,"value":"{\"event\":\"kPause\"}"},
                {"timestamp":2.0,"value":"{\"event\":\"kWebMediaPlayerDestroyed\"}"}
            ]}),
        )
    }
    #[test]
    fn destroyed_player_is_excluded_and_live_one_wins() {
        let mut s = software();
        event(
            &mut s,
            "Media.playerCreated",
            json!({"player":{"playerId":"old","domNodeId":5}}),
        )
        .unwrap();
        assert!(s.evidence().unwrap().is_none());
        assert!(s.ambiguity());
        destroyed_event(&mut s, "old").unwrap();
        assert_eq!(s.destroyed_total, 1);
        assert!(!s.ambiguity());
        let result = s.evidence().unwrap().unwrap();
        assert_eq!(result["decode_mode"], "software");
        assert_eq!(result["video_pipelines"], 1);
    }
    #[test]
    fn destroyed_is_sticky_and_terminal() {
        let mut s = software();
        destroyed_event(&mut s, "opaque").unwrap();
        assert!(s.players[0].destroyed);
        // Properties after the terminal event never resurrect the player.
        event(&mut s,"Media.playerPropertiesChanged",json!({"playerId":"opaque","properties":[{"name":"kVideoDecoderName","value":"VpxVideoDecoder"}]})).unwrap();
        assert!(s.players[0].destroyed);
        assert!(s.evidence().unwrap().is_none());
        // Replayed duplicate destroyed events are counted once.
        destroyed_event(&mut s, "opaque").unwrap();
        assert_eq!(s.destroyed_total, 1);
    }
    #[test]
    fn player_events_bounds_and_unknown_types() {
        let mut s = software();
        assert!(event(
            &mut s,
            "Media.playerEventsAdded",
            json!({"playerId":"opaque","events":(0..65).map(|_|json!({"timestamp":1.0,"value":"{\"event\":\"kPause\"}"})).collect::<Vec<_>>()})
        )
        .is_err());
        // value обязан быть JSON-строкой params с ключом "event" (pinned 156).
        for bad in [
            json!({"timestamp":1.0}),
            json!({"timestamp":1.0,"value":7}),
            json!({"timestamp":1.0,"value":"x".repeat(1025)}),
            json!({"timestamp":1.0,"value":"not json"}),
            json!({"timestamp":1.0,"value":"{\"other\":\"kWebMediaPlayerDestroyed\"}"}),
            json!({"timestamp":1.0,"value":"{\"event\":\"x\"".repeat(30)}),
        ] {
            assert!(
                event(
                    &mut s,
                    "Media.playerEventsAdded",
                    json!({"playerId":"opaque","events":[bad]})
                )
                .is_err(),
                "bad event must fail closed: {bad}"
            );
        }
        // Unknown будущий event игнорируется; имя ищется ТОЛЬКО в ключе
        // "event" — подстрока в другом значении не считается destroyed.
        assert!(event(
            &mut s,
            "Media.playerEventsAdded",
            json!({"playerId":"opaque","events":[
                {"timestamp":1.0,"value":"{\"event\":\"kSomeFutureEvent\"}"},
                {"timestamp":2.0,"value":"{\"event\":\"kLoad\",\"url\":\"https://x/kWebMediaPlayerDestroyed\"}"}
            ]})
        )
        .is_ok());
        assert!(!s.players[0].destroyed);
        assert_eq!(s.destroyed_total, 0);
        // Real kLoad events can contain long signed media URLs. Their private
        // contents do not impose a new 1 KiB cap or become lifecycle evidence.
        let private_url = format!(
            "https://fixture.invalid/{}kWebMediaPlayerDestroyed",
            "x".repeat(2048)
        );
        let params = json!({"event":"kLoad","url":private_url}).to_string();
        assert!(event(
            &mut s,
            "Media.playerEventsAdded",
            json!({"playerId":"opaque","events":[{"timestamp":3.0,"value":params}]})
        )
        .is_ok());
        assert!(!s.players[0].destroyed);
    }
    #[test]
    fn destroyed_players_preserve_total_observation_bound() {
        let mut s = software();
        for n in 0..3 {
            event(
                &mut s,
                "Media.playerCreated",
                json!({"player":{"playerId":format!("d{n}"),"domNodeId":50+n}}),
            )
            .unwrap();
            destroyed_event(&mut s, &format!("d{n}")).unwrap();
        }
        // Terminal records are excluded from evidence, but not from the
        // lifetime admission bound. Even destroyed-ID churn stays bounded.
        assert!(event(
            &mut s,
            "Media.playerCreated",
            json!({"player":{"playerId":"fresh","domNodeId":60}}),
        )
        .is_err());
        assert_eq!(s.players.len(), 4);
        assert_eq!(s.destroyed_total, 3);
        assert!(event(
            &mut s,
            "Media.playerCreated",
            json!({"player":{"playerId":"overflow","domNodeId":80}}),
        )
        .is_err());
    }
}
