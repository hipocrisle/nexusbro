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

#[derive(Clone)]
struct Grant {
    generation: u64,
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
        self.0
            .lock()
            .ok()
            .and_then(|g| g.get(&browser).cloned())
            .is_some_and(|g| g.generation == generation && g.document == document)
    }
    fn resource(
        &self,
        browser: i32,
        frame: &str,
        request: &str,
    ) -> Option<(&'static [u8], &'static str)> {
        let grants = self.0.lock().ok()?;
        let grant = grants.get(&browser)?;
        let prefix = grant.document.strip_suffix("player.html")?;
        if request == grant.document {
            // Main document navigation can still report the previous frame URL.
            return Some((include_bytes!("youtube_player/player.html"), "text/html"));
        }
        if frame != grant.document {
            return None;
        }
        match request.strip_prefix(prefix)? {
            "player.js" => Some((
                include_bytes!("youtube_player/player.js"),
                "text/javascript",
            )),
            "player.css" => Some((include_bytes!("youtube_player/player.css"), "text/css")),
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
            let request_url=CefString::from(&request.url()).to_string();
            let (bytes,mime)=self.documents.resource(browser.identifier(),&frame_url,&request_url)?;
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
    fn document_grant_is_not_a_cross_browser_or_subresource_capability() {
        let docs = PlayerDocuments::default();
        let url = docs.grant(7, 4).unwrap();
        assert!(docs.document_matches(7, 4, &url));
        assert!(!docs.document_matches(7, 3, &url));
        assert!(!docs.document_matches(8, 4, &url));
        assert!(docs.resource(8, &url, &url).is_none());
        let js = url.replace("player.html", "player.js");
        assert!(docs.resource(7, "https://example.org/", &js).is_none());
        assert!(docs.resource(7, &url, &js).is_some());
        assert!(docs.resource(7, &url, &format!("{js}?x=1")).is_none());
        docs.revoke(7);
        assert!(docs.resource(7, &url, &url).is_none());
    }
    #[test]
    fn replacement_revokes_the_old_document() {
        let docs = PlayerDocuments::default();
        let old = docs.grant(7, 4).unwrap();
        let new = docs.grant(7, 5).unwrap();
        assert_ne!(old, new);
        assert!(docs.resource(7, &old, &old).is_none());
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
    use crate::youtube_light_extract::MediaKind;
    let video = data
        .formats
        .iter()
        .filter(|f| {
            f.ext == "webm"
                && f.kind == MediaKind::VideoOnly
                && f.vcodec
                    .as_deref()
                    .is_some_and(|c| c.starts_with("vp9") || c.starts_with("vp09"))
                && f.height.is_some_and(|h| h <= 1080)
        })
        .max_by_key(|f| f.height);
    let audio = data.formats.iter().find(|f| {
        f.ext == "webm" && f.kind == MediaKind::AudioOnly && f.acodec.as_deref() == Some("opus")
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
    let mut qualities: Vec<_> = data
        .formats
        .iter()
        .filter(|f| {
            f.ext == "webm"
                && f.kind == MediaKind::VideoOnly
                && f.height.is_some_and(|h| h <= 4320)
                && f.vcodec
                    .as_deref()
                    .is_some_and(|c| c.starts_with("vp9") || c.starts_with("vp09"))
        })
        .collect();
    qualities.sort_by_key(|f| (f.height.unwrap_or(0), f.fps.unwrap_or(0.0) as u32));
    let choices:Vec<_>=qualities.into_iter().map(|f|serde_json::json!({"id":f.format_id,"label":format!("{}p · {} к/с",f.height.unwrap_or(0),f.fps.unwrap_or(0.0).round() as u32),"url":f.url.as_str()})).collect();
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
    let config = {
        let Ok(mut grants) = documents().0.lock() else {
            return;
        };
        grants
            .get_mut(&browser.identifier())
            .filter(|g| g.document == url)
            .and_then(|g| g.config.take())
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
pub fn stop(browser: &Browser) {
    crate::youtube_light_jobs::cancel(browser.identifier());
    crate::youtube_light_aux::cancel(browser.identifier());
    crate::cef_youtube_resume::closed(browser.identifier());
    crate::cef_youtube_captions::closed(browser.identifier());
    crate::cef_youtube_download::cancel(browser.identifier());
    let active = documents()
        .0
        .lock()
        .ok()
        .is_some_and(|g| g.contains_key(&browser.identifier()));
    documents().revoke(browser.identifier());
    if active {
        if let Some(frame) = browser.main_frame() {
            frame.execute_java_script(
                Some(&CefString::from(
                    "window.NB_LIGHT && window.NB_LIGHT.dispose();",
                )),
                None,
                0,
            );
        }
    }
}

pub fn logical_url(browser: i32, document: &str) -> Option<String> {
    documents().0.lock().ok().and_then(|g| {
        g.get(&browser)
            .filter(|g| g.document == document)
            .and_then(|g| g.logical_url.clone())
    })
}

thread_local! {
    static PROBES:std::cell::RefCell<Vec<(u64,std::sync::mpsc::Sender<crate::cef_vkladki::Sobytie>,crate::cef_youtube_probe::Probe)>>=const{std::cell::RefCell::new(Vec::new())};
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
            sender,
            crate::cef_youtube_probe::Probe::start(browser)?,
        ));
        Ok(())
    })
}
pub fn poll_probes() {
    PROBES.with(|probes|{
        probes.borrow_mut().retain_mut(|(tab,sender,probe)|{
            let Some(result)=probe.poll() else{return true;};
            let data=match result {
                Ok(state)=>serde_json::json!({"tip":"youtube-state","id":tab,"state":state}),
                Err(code)=>serde_json::json!({"tip":"youtube-state","id":tab,"error":"Состояние плеера недоступно","code":code}),
            };
            let _=sender.send(crate::cef_vkladki::Sobytie::Sluzhebnoe(data));
            false
        });
    });
}
pub fn clear_probes() {
    PROBES.with(|p| p.borrow_mut().clear());
    crate::cef_youtube_resume::clear();
    crate::cef_youtube_captions::clear();
}

/// Protocol/media fixture only. Does not replace real anonymous extraction.
/// Fixed GoogleVideo host is mapped to an owned HTTPS server by the CI proxy.
pub fn open_fixture(browser: &Browser) -> Result<(), &'static str> {
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
    let bytes = serde_json::to_vec(&json).map_err(|_| "Диагностика недоступна")?;
    let parsed = crate::youtube_light_extract::parse(&bytes, &id)
        .map_err(|_| "Диагностические дорожки отклонены")?;
    open_webm(browser, 1, Arc::new(parsed), None)
}

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
    let actual=browser.main_frame().map(|f|CefString::from(&f.url()).to_string())?;
    let identity=browser.identifier();
    let grants=documents().0.lock().ok()?;
    let grant=grants.get(&identity).filter(|g|g.document==actual)?;
    Some((grant.generation,grant.document.clone(),grant.media.clone()?))
}
