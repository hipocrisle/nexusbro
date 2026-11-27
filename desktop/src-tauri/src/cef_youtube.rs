//! Internal static player resources. No network broker or privileged JS API.
//! Grants are native-only and scoped to an existing browser, not a URL alone.
//! Integration must revoke on navigation, route change, close and browser lock.
use cef::*;
use rand_core::{OsRng, RngCore};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, OnceLock},
};

const SCHEME: &str = "nb-lite";
const DOMAIN: &str = "player";
const MAX_GRANTS: usize = 128;

#[derive(Clone,Default)]
struct ResourceCounts {html:u32,js:u32,css:u32,frame_refused:u32}
#[derive(Clone)]
struct Grant {
    generation: u64,
    fixture: bool,
    resources:ResourceCounts,
    document_frame:Option<String>,
    document: String,
    config: Option<zeroize::Zeroizing<String>>,
    logical_url: Option<String>,
    auxiliary: Option<serde_json::Value>,
    media: Option<Arc<crate::youtube_light_extract::ExtractedVideo>>,
}
#[derive(Clone, Default)]
pub struct PlayerDocuments(Arc<Mutex<HashMap<i32, Grant>>>);
impl PlayerDocuments {
    /// Called on CEF UI thread for the admitted, already existing Browser.
    pub fn install(&self, context: &RequestContext) -> bool {
        let mut factory = PlayerFactory::new(self.clone());
        context.register_scheme_handler_factory(
            Some(&CefString::from(SCHEME)),
            Some(&CefString::from(DOMAIN)),
            Some(&mut factory),
        ) != 0
    }
    /// The caller supplies the actual browser generation, never renderer input.
    pub fn grant(&self, browser: i32, generation: u64) -> Option<String> {
        forget_reload(browser);
        if generation == 0 {
            return None;
        }
        let mut grants = self.0.lock().ok()?;
        if grants.len() >= MAX_GRANTS && !grants.contains_key(&browser) {
            return None;
        }
        let mut nonce = [0u8; 24];
        OsRng.try_fill_bytes(&mut nonce).ok()?;
        let token: String = nonce.iter().map(|b| format!("{b:02x}")).collect();
        let document = format!("{SCHEME}://{DOMAIN}/{token}/player.html");
        grants.insert(
            browser,
            Grant {
                generation,
                fixture:false,
                resources:ResourceCounts::default(),
                document_frame:None,
                document: document.clone(),
                config: None,
                logical_url: None,
                auxiliary: None,
                media: None,
            },
        );
        Some(document)
    }
    pub fn revoke(&self, browser: i32) {
        if let Ok(mut grants) = self.0.lock() {
            grants.remove(&browser);
        }
    }
    pub fn revoke_all(&self) {
        if let Ok(mut grants) = self.0.lock() {
            grants.clear();
        }
    }
    pub fn document_matches(&self, browser: i32, generation: u64, document: &str) -> bool {
        self.0.lock().is_ok_and(|grants|grants.get(&browser).is_some_and(|grant|grant.generation==generation&&grant.document==document))
    }
    fn resource(
        &self,
        browser: i32,
        frame_id: &str,
        _frame: &str,
        request: &str,
    ) -> Option<(&'static [u8], &'static str)> {
        let mut grants = self.0.lock().ok()?;
        let grant = grants.get_mut(&browser)?;
        if frame_id.is_empty()||frame_id.len()>256||frame_id.chars().any(char::is_control){return None;}
        let prefix = grant.document.strip_suffix("player.html")?;
        if request == grant.document {
            grant.resources.html=grant.resources.html.saturating_add(1);
            // Main document navigation can still report the previous frame URL.
            return Some((include_bytes!("youtube_player/player.html"), "text/html"));
        }
        // These are immutable public source files, never a configuration or
        // media-data endpoint. The native factory already requires MAIN frame.
        // Cross-process navigation can replace its provisional Frame ID before
        // assets load (Windows). Bind private payload only at committed on_loaded.
        if grant.resources.html==0 {
            grant.resources.frame_refused=grant.resources.frame_refused.saturating_add(1);
            return None;
        }
        match request.strip_prefix(prefix)? {
            "player.js" => {grant.resources.js=grant.resources.js.saturating_add(1);Some((
                include_bytes!("youtube_player/player.js"),
                "text/javascript",
            ))},
            "player.css" => {grant.resources.css=grant.resources.css.saturating_add(1);Some((include_bytes!("youtube_player/player.css"), "text/css"))},
            _ => None,
        }
    }
}

pub fn register_scheme(registrar: &SchemeRegistrar) -> bool {
    let flags = SchemeOptions::STANDARD.get_raw()
        | SchemeOptions::SECURE.get_raw()
        | SchemeOptions::DISPLAY_ISOLATED.get_raw();
    registrar.add_custom_scheme(Some(&CefString::from(SCHEME)), flags as i32) != 0
}

wrap_scheme_handler_factory! {
    struct PlayerFactory { documents: PlayerDocuments }
    impl SchemeHandlerFactory {
        fn create(&self,browser:Option<&mut Browser>,frame:Option<&mut Frame>,_scheme_name:Option<&CefString>,request:Option<&mut Request>)->Option<ResourceHandler> {
            let (Some(browser),Some(frame),Some(request))=(browser,frame,request) else {return None;};
            if frame.is_main()==0 || CefString::from(&request.method()).to_string()!="GET" {return None;}
            let frame_url=CefString::from(&frame.url()).to_string();
            let frame_id=CefString::from(&frame.identifier()).to_string();
            let request_url=CefString::from(&request.url()).to_string();
            let (bytes,mime)=self.documents.resource(browser.identifier(),&frame_id,&frame_url,&request_url)?;
            Some(PlayerResource::new(bytes,mime,Arc::new(Mutex::new(0))))
        }
    }
}
wrap_resource_handler! {
    struct PlayerResource { bytes:&'static [u8], mime:&'static str, cursor:Arc<Mutex<usize>> }
    impl ResourceHandler {
        fn open(&self,_request:Option<&mut Request>,handle_request:Option<&mut i32>,_callback:Option<&mut Callback>)->i32 {
            if let Some(handle)=handle_request {*handle=1;} 1
        }
        fn response_headers(&self,response:Option<&mut Response>,response_length:Option<&mut i64>,_redirect:Option<&mut CefString>) {
            if let Some(response)=response {
                response.set_status(200); response.set_mime_type(Some(&CefString::from(self.mime)));
                response.set_charset(Some(&CefString::from("utf-8")));
                response.set_header_by_name(Some(&CefString::from("Cache-Control")),Some(&CefString::from("no-store")),1);
                response.set_header_by_name(Some(&CefString::from("X-Content-Type-Options")),Some(&CefString::from("nosniff")),1);
                response.set_header_by_name(Some(&CefString::from("Referrer-Policy")),Some(&CefString::from("no-referrer")),1);
            }
            if let Some(length)=response_length {*length=self.bytes.len() as i64;}
        }
        fn read(&self,data_out:*mut u8,bytes_to_read:i32,bytes_read:Option<&mut i32>,_callback:Option<&mut ResourceReadCallback>)->i32 {
            let Some(read)=bytes_read else{return 0;}; *read=0;
            if bytes_to_read<=0 || data_out.is_null(){return 0;}
            let Ok(mut cursor)=self.cursor.lock() else{return 0;};
            let count=(bytes_to_read as usize).min(self.bytes.len().saturating_sub(*cursor));
            if count==0{return 0;}
            // CEF owns a writable buffer of bytes_to_read bytes for this callback.
            unsafe {std::ptr::copy_nonoverlapping(self.bytes.as_ptr().add(*cursor),data_out,count);}
            *cursor+=count; *read=count as i32; 1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn public_assets_are_browser_nonce_bound_without_private_payload() {
        let docs = PlayerDocuments::default();
        let url = docs.grant(7, 4).unwrap();
        assert!(docs.document_matches(7, 4, &url));
        assert!(!docs.document_matches(7, 3, &url));
        assert!(!docs.document_matches(8, 4, &url));
        assert!(docs.resource(8, "frame-1", &url, &url).is_none());
        let js = url.replace("player.html", "player.js");
        assert!(docs.resource(7, "frame-1", "https://example.org/", &js).is_none());
        assert!(docs.resource(7, "frame-1", "https://example.org/", &url).is_some());
        assert!(docs.resource(7, "frame-2", &url, &js).is_some());
        assert!(docs.resource(7, "frame-1", "https://example.org/", &js).is_some());
        assert!(docs.resource(7, "frame-1", &url, &js).is_some());
        docs.0.lock().unwrap().get_mut(&7).unwrap().config=Some(zeroize::Zeroizing::new("PRIVATE_CONFIG_MUST_NOT_BE_SERVED".into()));
        let (public_js,_)=docs.resource(7,"frame-2","https://example.org/",&js).unwrap();
        assert!(!String::from_utf8_lossy(public_js).contains("PRIVATE_CONFIG_MUST_NOT_BE_SERVED"));
        assert!(docs.resource(7,"frame-1","https://example.org/",&js).is_some());
        assert!(docs.resource(7, "frame-1", &url, &format!("{js}?x=1")).is_none());
        docs.revoke(7);
        assert!(docs.resource(7, "frame-1", &url, &url).is_none());
    }
    #[test]
    fn replacement_revokes_the_old_document() {
        let docs = PlayerDocuments::default();
        let old = docs.grant(7, 4).unwrap();
        let new = docs.grant(7, 5).unwrap();
        assert_ne!(old, new);
        assert!(docs.resource(7, "frame-1", &old, &old).is_none());
        assert!(docs.document_matches(7, 5, &new));
        docs.revoke_all();
        assert!(!docs.document_matches(7, 5, &new));
    }
}

static DOCUMENTS: OnceLock<PlayerDocuments> = OnceLock::new();
pub fn documents() -> &'static PlayerDocuments {
    DOCUMENTS.get_or_init(Default::default)
}

/// Native-only prototype admission; no codec capability is inferred from metadata.
/// Real playback/seek/background proof is required before enabling the UI action.
pub fn open_webm(
    browser: &Browser,
    generation: u64,
    data: Arc<crate::youtube_light_extract::ExtractedVideo>,
    resume: Option<(
        crate::youtube_light_resume::ResumeScope,
        Option<std::path::PathBuf>,
    )>,
) -> Result<(), &'static str> {
    let facts:Vec<_>=data.formats.iter().map(|f|crate::youtube_light_download::PlaybackFacts{
        legacy_vp9:crate::youtube_light_download::supports_video(f),
        av1:crate::youtube_light_download::playback_av1(f),
        height:f.height,fps:f.fps,
    }).collect();
    let picked=crate::youtube_light_download::playback_pick(&facts);
    let video=picked.as_ref().map(|(default,_)|&data.formats[*default]);
    // The pinned yt-dlp emits formats in increasing preference order, as the
    // download selector also expects. Keep playback on its best supported Opus.
    let audio = data.formats.iter().rev().find(|f| {
        crate::youtube_light_download::supports_audio(f)
    });
    let (Some(video), Some(audio)) = (video, audio) else {
        return Err("Нет подходящих WebM дорожек для лёгкого плеера");
    };
    let context = browser
        .host()
        .and_then(|h| h.request_context())
        .ok_or("Контекст вкладки недоступен")?;
    if !documents().install(&context) {
        return Err("Не удалось подготовить внутренний плеер");
    }
    let url = documents()
        .grant(browser.identifier(), generation)
        .ok_or("Не удалось открыть внутренний плеер")?;
    {
        let mut grants = documents().0.lock().map_err(|_| "Плеер занят")?;
        grants
            .get_mut(&browser.identifier())
            .ok_or("Плеер закрыт")?
            .logical_url = Some(data.metadata.id().watch_url());
        if let Some(grant)=grants.get_mut(&browser.identifier()) {grant.media=Some(data.clone());}
    }
    let restored = if let Some((scope, root)) = resume {
        crate::cef_youtube_resume::attach(
            browser,
            url.clone(),
            scope,
            root,
            data.metadata.id().clone(),
        ).ok()
    } else {
        None
    };
    let position = restored.unwrap_or(0.0);
    let mut qualities:Vec<_>=picked.map(|(_,indices)|indices).unwrap_or_default().into_iter().map(|i|&data.formats[i]).collect();
    qualities.sort_by_key(|f| (f.height.unwrap_or(0), f.fps.unwrap_or(0.0) as u32, f.ext=="mp4"));
    let choices:Vec<_>=qualities.into_iter().map(|f|{
        let base=format!("{}p · {} к/с",f.height.unwrap_or(0),f.fps.unwrap_or(0.0).round() as u32);
        let label=if f.ext=="mp4"{format!("{base} · AV1")}else{base};
        serde_json::json!({"id":f.format_id,"label":label,"url":f.url.as_str()})
    }).collect();
    let config = serde_json::json!({"title":data.metadata.title(),"video":video.url.as_str(),"audio":audio.url.as_str(),"qualities":choices,"captions":data.captions.iter().map(|t|t.metadata()).collect::<Vec<_>>(),"position":position,"resume_available":restored.is_some(),"source":data.metadata.id().watch_url()});
    let json = zeroize::Zeroizing::new(
        serde_json::to_string(&config).map_err(|_| "Не удалось подготовить видео")?,
    );
    {
        let mut grants = documents().0.lock().map_err(|_| "Плеер занят")?;
        let grant = grants
            .get_mut(&browser.identifier())
            .ok_or("Плеер уже закрыт")?;
        grant.config = Some(json);
        grant.logical_url = Some(data.metadata.id().watch_url());
    }
    let Some(frame) = browser.main_frame() else {
        documents().revoke(browser.identifier());
        return Err("Документ вкладки недоступен");
    };
    frame.load_url(Some(&CefString::from(url.as_str())));
    Ok(())
}

pub fn on_navigation(browser: i32, target: &str) {
    crate::cef_youtube_benchmark::cancel(browser);
    crate::cef_youtube_resume_fixture::cancel(browser);
    retire_stop_probe(browser);
    if let Ok(mut hints)=reload_hints().lock(){hints.navigated(browser,target);}
    crate::youtube_light_jobs::cancel(browser);
    let leaving = {
        let Ok(mut grants) = documents().0.lock() else {
            return;
        };
        let leaving = grants.get(&browser).is_none_or(|g| g.document != target);
        if leaving {
            grants.remove(&browser);
        }
        leaving
    };
    if leaving {
        crate::youtube_light_aux::cancel(browser);
        crate::cef_youtube_resume::closed(browser);
        crate::cef_youtube_captions::closed(browser);
        crate::cef_youtube_download::cancel(browser);
    }
}
pub fn on_loaded(browser: &Browser, frame: &Frame) {
    if frame.is_main() == 0 {
        return;
    }
    let url = CefString::from(&frame.url()).to_string();
    let frame_id=CefString::from(&frame.identifier()).to_string();
    if browser.main_frame().is_none_or(|current|CefString::from(&current.identifier()).to_string()!=frame_id){return;}
    let config = {
        let Ok(mut grants) = documents().0.lock() else {
            return;
        };
        grants
            .get_mut(&browser.identifier())
            .filter(|g| g.document == url)
            .and_then(|g| {g.document_frame=Some(frame_id);g.config.take()})
    };
    if let Some(config) = config {
        let auxiliary = {
            let mut grants = match documents().0.lock() {
                Ok(v) => v,
                Err(_) => return,
            };
            grants
                .get_mut(&browser.identifier())
                .filter(|g| g.document == url)
                .and_then(|g| g.auxiliary.take())
        };
        let auxiliary = auxiliary
            .map(|v| format!("window.NB_LIGHT && window.NB_LIGHT.setAux({v});"))
            .unwrap_or_default();
        let script = zeroize::Zeroizing::new(format!(
            "window.NB_LIGHT && window.NB_LIGHT.configure({});{}",
            config.as_str(),
            auxiliary
        ));
        // This is a JS expression passed through CEF, never interpolated HTML.
        frame.execute_java_script(Some(&CefString::from(script.as_str())), None, 0);
    }
}
pub fn stop(browser: &Browser, tab:u64, sender:&std::sync::mpsc::Sender<crate::cef_vkladki::Sobytie>) {
    let identity=browser.identifier();
    crate::cef_youtube_benchmark::cancel(identity);
    crate::cef_youtube_resume_fixture::cancel(identity);
    let binding=documents().0.lock().ok().and_then(|all|all.get(&identity).map(|g|(g.document.clone(),g.document_frame.clone())));
    // Repeated lock notifications must keep the first in-flight observation.
    // Cancellation still runs below even when extraction has no player grant.
    let stop_token=if binding.is_some(){retire_stop_probe(identity);crate::cef_youtube_stop_probe::mint(browser).ok()}else{None};
    crate::youtube_light_jobs::cancel(identity);
    crate::youtube_light_aux::cancel(identity);
    crate::cef_youtube_resume::closed(identity);
    crate::cef_youtube_captions::closed(identity);
    crate::cef_youtube_download::cancel(identity);
    let (active,hint) = documents().0.lock().ok().map(|grants| {
        let grant=grants.get(&identity);
        let hint=grant.and_then(|grant| {
            let video=grant.logical_url.as_deref().and_then(|url|crate::youtube_light::VideoId::from_url(url).ok())?;
            Some((grant.document.clone(),video))
        });
        (grant.is_some(),hint)
    }).unwrap_or((false,None));
    if let Some((document,video))=hint {if let Ok(mut hints)=reload_hints().lock(){hints.remember(identity,&document,video);}}
    documents().revoke(identity);
    if active {
        if let Some(frame) = browser.main_frame().filter(|frame|binding.as_ref().is_some_and(|(doc,id)|id.as_deref()==Some(CefString::from(&frame.identifier()).to_string().as_str())&&*doc==CefString::from(&frame.url()).to_string())) {
            frame.execute_java_script(
                Some(&CefString::from(
                    "window.NB_LIGHT && window.NB_LIGHT.dispose();",
                )),
                None,
                0,
            );
        }
    }
    if let Some(token)=stop_token {
        STOP_PROBES.with(|p|{let mut p=p.borrow_mut();if p.len()<16{p.push(StopObservation{browser:browser.clone(),identity,tab,sender:sender.clone(),token,started:std::time::Instant::now(),attempt:0,first_stopped:None,probe:None});}});
    }
}

fn reload_hints()->&'static std::sync::Mutex<crate::youtube_light_reload::ReloadHints>{
    static HINTS:std::sync::OnceLock<std::sync::Mutex<crate::youtube_light_reload::ReloadHints>>=std::sync::OnceLock::new();
    HINTS.get_or_init(Default::default)
}
/// Clear on native browser close. This does not grant any document authority.
pub fn forget_reload(browser:i32){crate::cef_youtube_benchmark::cancel(browser);crate::cef_youtube_resume_fixture::cancel(browser);retire_stop_probe(browser);if let Ok(mut hints)=reload_hints().lock(){hints.forget(browser);}}
/// ONLY for canonical reload/extractor URL normalization, never admission.
pub fn reload_url(browser:i32,document:&str)->Option<String>{
    logical_url(browser,document).or_else(||reload_hints().lock().ok()?.video(browser,document).map(|video|video.watch_url()))
}

pub fn logical_url(browser: i32, document: &str) -> Option<String> {
    documents().0.lock().ok().and_then(|g| {
        g.get(&browser)
            .filter(|g| g.document == document)
            .and_then(|g| g.logical_url.clone())
    })
}

thread_local! {
    static PROBES:std::cell::RefCell<Vec<(u64,i32,std::sync::mpsc::Sender<crate::cef_vkladki::Sobytie>,crate::cef_youtube_probe::Probe)>>=const{std::cell::RefCell::new(Vec::new())};
}
pub fn request_probe(
    browser: &Browser,
    tab: u64,
    sender: std::sync::mpsc::Sender<crate::cef_vkladki::Sobytie>,
) -> Result<(), &'static str> {
    PROBES.with(|probes| {
        let mut probes = probes.borrow_mut();
        if probes.len() >= 16 {
            return Err("Слишком много запросов состояния плеера");
        }
        probes.push((
            tab,
            browser.identifier(),
            sender,
            crate::cef_youtube_probe::Probe::start(browser)?,
        ));
        Ok(())
    })
}
pub fn poll_probes() {
    crate::cef_youtube_benchmark::poll();
    poll_stop_probes();
    PROBES.with(|probes|{
        probes.borrow_mut().retain_mut(|(tab,browser,sender,probe)|{
            let Some(result)=probe.poll() else{return true;};
            let data=match result {
                Ok(state)=>serde_json::json!({"tip":"youtube-state","id":tab,"state":state,"resources":resource_counts(*browser)}),
                Err(code)=>serde_json::json!({"tip":"youtube-state","id":tab,"error":"Состояние плеера недоступно","code":code,"resources":resource_counts(*browser)}),
            };
            let _=sender.send(crate::cef_vkladki::Sobytie::Sluzhebnoe(data));
            false
        });
    });
}
pub fn clear_probes() {
    crate::cef_youtube_benchmark::clear();
    STOP_EPOCH.with(|e|e.set(e.get().wrapping_add(1)));
    STOP_PROBES.with(|p|p.borrow_mut().clear());
    PROBES.with(|p| p.borrow_mut().clear());
    crate::cef_youtube_resume::clear();
    crate::cef_youtube_captions::clear();
}

/// Protocol/media fixture only. Does not replace real anonymous extraction.
/// Fixed GoogleVideo host is mapped to an owned HTTPS server by the CI proxy.
fn open_fixture_inner(browser:&Browser,generation:u64,resume:Option<(crate::youtube_light_resume::ResumeScope,Option<std::path::PathBuf>)>,route:Option<crate::tab_routes::NativeRoute>) -> Result<(), &'static str> {
    if std::env::var("GITHUB_ACTIONS").as_deref() != Ok("true")
        || std::env::var_os("NEXUSBRO_TEST_DATA").is_none()
    {
        return Err("Диагностика доступна только в CI");
    }
    let id = crate::youtube_light::VideoId::parse("BaW_jenozKc")
        .map_err(|_| "Диагностика недоступна")?;
    let json = serde_json::json!({"id":id.as_str(),"title":"NexusBro controlled media fixture","uploader":"NexusBro CI","duration":120,"is_live":false,"formats":[
        {"format_id":"fixture-video","ext":"webm","vcodec":"vp9","acodec":"none","width":320,"height":180,"fps":12,"protocol":"https","url":"https://fixture.googlevideo.com/videoplayback?track=video"},
        {"format_id":"fixture-audio","ext":"webm","vcodec":"none","acodec":"opus","protocol":"https","url":"https://fixture.googlevideo.com/videoplayback?track=audio"}
    ]});
    let mut json=json;
    if let Ok(sentinel)=std::env::var("NEXUSBRO_TEST_YOUTUBE_MEDIA_SENTINEL"){
        if sentinel.len()!=48||!sentinel.bytes().all(|b|b.is_ascii_digit()||(b'a'..=b'f').contains(&b)){return Err("Диагностический маркер отклонён");}
        for format in json["formats"].as_array_mut().ok_or("Диагностика недоступна")?{
            let source=format["url"].as_str().ok_or("Диагностика недоступна")?;
            format["url"]=serde_json::json!(format!("{source}&nb_privacy={sentinel}"));
        }
    }
    if std::env::var("NEXUSBRO_TEST_YOUTUBE_FEATURES").as_deref()==Ok("1"){
        json["formats"][0]["format_id"]=serde_json::json!("fixture-video-low");
        let low=json["formats"][0]["url"].as_str().ok_or("Диагностика недоступна")?.to_owned();
        json["formats"][0]["url"]=serde_json::json!(format!("{low}&quality=low"));
        let high=json["formats"][0]["url"].as_str().ok_or("Диагностика недоступна")?.replace("quality=low","quality=high");
        json["formats"].as_array_mut().ok_or("Диагностика недоступна")?.push(serde_json::json!({"format_id":"fixture-video-high","ext":"webm","vcodec":"vp9","acodec":"none","width":640,"height":360,"fps":12,"protocol":"https","url":high}));
        json["subtitles"]=serde_json::json!({"de":[{"url":"https://www.youtube.com/api/timedtext?v=BaW_jenozKc&fmt=vtt&lang=de","ext":"vtt","name":"Deutsch"}]});
    }
    let bytes = serde_json::to_vec(&json).map_err(|_| "Диагностика недоступна")?;
    let parsed = crate::youtube_light_extract::parse(&bytes, &id)
        .map_err(|_| "Диагностические дорожки отклонены")?;
    let data=Arc::new(parsed);
    open_webm(browser,generation,data.clone(),resume)?;
    let identity=browser.identifier();
    if let Ok(mut grants)=documents().0.lock(){if let Some(g)=grants.get_mut(&identity){g.fixture=true;}}
    if let (Some(route),Some(document))=(route,current_document(identity)){
        crate::cef_youtube_captions::attach(identity,generation,document,data.captions.clone(),route);
    }
    Ok(())
}
pub fn open_fixture(browser:&Browser)->Result<(),&'static str>{open_fixture_inner(browser,1,None,None)}
pub fn open_fixture_routed(browser:&Browser,route:crate::tab_routes::NativeRoute)->Result<(),&'static str>{open_fixture_inner(browser,1,None,Some(route))}
pub fn open_fixture_with_resume(browser:&Browser,generation:u64,resume:(crate::youtube_light_resume::ResumeScope,Option<std::path::PathBuf>))->Result<(),&'static str>{open_fixture_inner(browser,generation,Some(resume),None)}


pub fn current_document(browser: i32) -> Option<String> {
    documents()
        .0
        .lock()
        .ok()
        .and_then(|g| g.get(&browser).map(|g| g.document.clone()))
}
pub fn apply_aux(browser: &Browser, ready: crate::youtube_light_aux::Ready) {
    let data = serde_json::json!({
        "segments":ready.segments.map(|v|v.into_iter().map(|s|[s.start,s.end]).collect::<Vec<_>>()),
        "likes":ready.dislikes.map(|d|d.likes),"dislikes":ready.dislikes.map(|d|d.dislikes),
        "segment_error":ready.segment_error,"rating_error":ready.rating_error
    });
    let Some(frame) = browser.main_frame() else {
        return;
    };
    let actual = CefString::from(&frame.url()).to_string();
    let apply = {
        let Ok(mut grants) = documents().0.lock() else {
            return;
        };
        let Some(grant) = grants
            .get_mut(&browser.identifier())
            .filter(|g| g.generation == ready.operation && g.document == ready.document)
        else {
            return;
        };
        if grant.config.is_some() {
            grant.auxiliary = Some(data.clone());
            false
        } else {
            actual == grant.document
        }
    };
    if apply {
        frame.execute_java_script(
            Some(&CefString::from(
                format!("window.NB_LIGHT && window.NB_LIGHT.setAux({data});").as_str(),
            )),
            None,
            0,
        );
    }
}

/// Native-only retained extractor result. No signed URL crosses parent IPC.
pub fn media(browser:&Browser)->Option<(u64,String,Arc<crate::youtube_light_extract::ExtractedVideo>)> {
    let frame=browser.main_frame()?;
    let actual=CefString::from(&frame.url()).to_string();
    let frame_id=CefString::from(&frame.identifier()).to_string();
    let identity=browser.identifier();
    let grants=documents().0.lock().ok()?;
    let grant=grants.get(&identity).filter(|g|g.document==actual&&g.document_frame.as_deref()==Some(frame_id.as_str()))?;
    Some((grant.generation,grant.document.clone(),grant.media.clone()?))
}

fn resource_counts(browser:i32)->serde_json::Value {
 documents().0.lock().ok().and_then(|all|all.get(&browser).map(|g|serde_json::json!({"html":g.resources.html,"js":g.resources.js,"css":g.resources.css,"frame_refused":g.resources.frame_refused}))).unwrap_or(serde_json::Value::Null)
}

/// Fixture-only observation capability: no playback or data access authority.
pub fn fixture_stop_identity(browser:i32,frame:&str,document:&str)->Option<u64>{
    documents().0.lock().ok()?.get(&browser).filter(|g|g.fixture&&g.document==document&&g.document_frame.as_deref()==Some(frame)).map(|g|g.generation)
}
struct StopObservation{
    browser:Browser,identity:i32,tab:u64,sender:std::sync::mpsc::Sender<crate::cef_vkladki::Sobytie>,
    token:crate::cef_youtube_stop_probe::FixtureStopToken,started:std::time::Instant,attempt:usize,
    first_stopped:Option<(std::time::Instant,f64,f64)>,
    probe:Option<crate::cef_youtube_stop_probe::Probe>,
}
thread_local!{static STOP_EPOCH:std::cell::Cell<u64>=const{std::cell::Cell::new(0)};static STOP_PROBES:std::cell::RefCell<Vec<StopObservation>>=const{std::cell::RefCell::new(Vec::new())};}
fn retire_stop_probe(browser:i32){STOP_EPOCH.with(|e|e.set(e.get().wrapping_add(1)));STOP_PROBES.with(|p|p.borrow_mut().retain(|v|v.identity!=browser));}
fn poll_stop_probes(){
    let epoch=STOP_EPOCH.with(|e|e.get());
    let mut pending=STOP_PROBES.with(|p|std::mem::take(&mut *p.borrow_mut()));
    pending.retain_mut(|v|{
        if STOP_EPOCH.with(|e|e.get())!=epoch{return false;}
        let elapsed=v.started.elapsed();
        let sender=v.sender.clone();let tab=v.tab;let observations=v.attempt;
        let emit=|complete:bool,state:Option<serde_json::Value>,code:&str,stable_ms:u64|{
            if STOP_EPOCH.with(|e|e.get())!=epoch{return;}
            let _=sender.send(crate::cef_vkladki::Sobytie::Sluzhebnoe(serde_json::json!({"tip":"youtube-stop-state","id":tab,"complete":complete,"state":state,"code":code,"observations":observations,"stable_ms":stable_ms})));
        };
        if elapsed>=std::time::Duration::from_secs(5){emit(false,None,"stop_observation_timeout",0);return false;}
        if let Some(probe)=v.probe.as_mut(){
            let Some(result)=probe.poll() else{return true;};
            v.probe=None;
            match result{
                Ok(state)=>{
                    let empty=["disposed","video_paused","audio_paused","video_source_attribute_absent","audio_source_attribute_absent","video_source_object_absent","audio_source_object_absent","video_source_children_absent","audio_source_children_absent"].iter().all(|key|state[*key].as_bool()==Some(true))
                        &&["video_network_state","audio_network_state","video_ready_state","audio_ready_state"].iter().all(|key|state[*key].as_u64()==Some(0));
                    let position=state["position"].as_f64().unwrap_or(f64::NAN);let audio=state["audio_position"].as_f64().unwrap_or(f64::NAN);
                    if empty{
                        if let Some((first,old_position,old_audio))=v.first_stopped{
                            let stable_ms=first.elapsed().as_millis() as u64;
                            if (position-old_position).abs()<=0.15&&(audio-old_audio).abs()<=0.15{
                                if stable_ms>=200{emit(true,Some(state),"stopped",stable_ms);return false;}
                            }else{v.first_stopped=Some((std::time::Instant::now(),position,audio));}
                        }else{v.first_stopped=Some((std::time::Instant::now(),position,audio));}
                    }else{v.first_stopped=None;}
                    if v.attempt>=4{emit(false,Some(state),if empty{"stop_unstable"}else{"stop_incomplete"},0);return false;}
                },
                Err(code)=>{emit(false,None,code,0);return false;}
            }
        }
        if STOP_EPOCH.with(|e|e.get())!=epoch{return false;}
        let delays=[0,250,1000,2000];
        if v.attempt<delays.len()&&elapsed.as_millis()>=delays[v.attempt]{
            match crate::cef_youtube_stop_probe::Probe::start(&v.browser,&v.token){Ok(probe)=>{v.probe=Some(probe);v.attempt+=1;},Err(code)=>{emit(false,None,code,0);return false;}}
        }
        true
    });
    if STOP_EPOCH.with(|e|e.get())==epoch{STOP_PROBES.with(|p|p.borrow_mut().extend(pending));}
}

/// Immutable committed native binding. Callers read CEF identities BEFORE
/// entering this accessor; no CEF method is called while the grant mutex is held.
pub fn privacy_binding(browser:i32,frame:&str,document:&str)->Option<u64>{
    if browser<=0||frame.is_empty()||frame.len()>256{return None;}
    let grants=documents().0.lock().ok()?;
    let grant=grants.get(&browser).filter(|g|g.document==document&&g.document_frame.as_deref()==Some(frame)&&g.media.is_some())?;
    Some(grant.generation)
}
/// Request URL remains RAM-only. Admission is exact against the validated native
/// extractor result, not merely a googlevideo host or an origin supplied by JS.
pub fn privacy_media_url(browser:i32,frame:&str,operation:u64,request_url:&str)->bool{
    if request_url.len()>16384||request_url.bytes().any(|b|b.is_ascii_control()) {return false;}
    let Ok(url)=reqwest::Url::parse(request_url)else{return false;};
    if url.scheme()!="https"||!url.username().is_empty()||url.password().is_some()||url.port().is_some()
        ||url.fragment().is_some()||url.path()!="/videoplayback"
        ||!url.host_str().is_some_and(|host|host.ends_with(".googlevideo.com")&&host.len()>".googlevideo.com".len()){return false;}
    let Ok(grants)=documents().0.lock()else{return false;};
    grants.get(&browser).filter(|g|g.generation==operation&&g.document_frame.as_deref()==Some(frame))
        .and_then(|g|g.media.as_ref()).is_some_and(|media|media.formats.iter().any(|format|format.url.as_str()==url.as_str()))
}
