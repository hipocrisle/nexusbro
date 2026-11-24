//! Internal static player resources. No network broker or privileged JS API.
//! Grants are native-only and scoped to an existing browser, not a URL alone.
//! Integration must revoke on navigation, route change, close and browser lock.
use cef::*;
use rand_core::{OsRng, RngCore};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

const SCHEME: &str = "nb-lite";
const DOMAIN: &str = "player";
const MAX_GRANTS: usize = 128;

#[derive(Clone)]
struct Grant {
    generation: u64,
    document: String,
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
