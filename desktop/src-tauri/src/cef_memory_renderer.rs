//! Renderer-owned early safety observation. No page-console or global binding
//! can manufacture an approval. Browser-side code must still validate the
//! native sender, Browser identity, document token and pending request epoch.
use cef::*;
use rand_core::{OsRng, RngCore};
use serde_json::{json, Value};
use std::{
    cell::RefCell,
    collections::HashMap,
    sync::atomic::{AtomicU64, Ordering},
};

#[derive(Clone)]
struct Document {
    token: String,
    context: V8Context,
    probe: Option<V8Value>,
    lifetime: u64,
    attempts: u8,
}
#[derive(Clone)]
struct Seed {
    owner: Browser,
    name: String,
    nonce: String,
    lifetime: u64,
}
thread_local! {
    static SEEDS: RefCell<HashMap<i32, Seed>> = Default::default();
    static DOCUMENTS: RefCell<HashMap<(i32, String), Document>> = Default::default();
}
static NEXT: AtomicU64 = AtomicU64::new(1);
static DIAGNOSTICS: AtomicU64 = AtomicU64::new(0);
fn diagnostic(message: std::fmt::Arguments<'_>) {
    if DIAGNOSTICS.fetch_add(1, Ordering::Relaxed) < 64 {
        eprintln!("NEXUSBRO_MEMORY_RENDERER {message}");
    }
}
fn reason_code(reason: &Value) -> &'static str {
    match reason.as_str() {
        None => "none",
        Some(
            "Unsupported native hook"
            | "Native hooks unavailable"
            | "Early native instrumentation unavailable"
            | "Before-unload hook unavailable"
            | "Before-unload state unavailable",
        ) => "js_install_unavailable",
        Some("Session storage state unavailable") => "js_storage_unavailable",
        Some("Service worker state unavailable") => "js_service_worker_unavailable",
        Some("Dynamic or uninspectable document") => "js_dynamic_budget",
        Some("Document state unavailable or changed") => "js_probe_unavailable_or_tampered",
        Some("Trusted page interaction or editing") => "js_trusted_interaction",
        Some(
            "Media, connection or worker API used"
            | "Media playback requested"
            | "Speech playback requested"
            | "Media capture requested"
            | "Screen capture requested",
        ) => "js_media_or_worker",
        Some(
            "Form, media, frame, canvas or shadow DOM"
            | "Frame, canvas or media element created"
            | "Shadow DOM used"
            | "Canvas used"
            | "Offscreen canvas used",
        ) => "js_dom_protected",
        Some("Before-unload listener registered" | "Before-unload handler installed") => {
            "js_unload_protected"
        }
        Some(
            "Session storage is not empty"
            | "Service worker controls page"
            | "Service worker registered"
            | "Editable document",
        ) => "js_state_protected",
        Some(_) => "other_internal_reason",
    }
}

fn key(browser: &Browser, frame: &Frame) -> (i32, String) {
    (
        browser.identifier(),
        CefString::from(&frame.identifier()).to_string(),
    )
}
fn new_token() -> Option<String> {
    let mut nonce = [0u8; 16];
    OsRng.try_fill_bytes(&mut nonce).ok()?;
    let nonce: String = nonce.iter().map(|b| format!("{b:02x}")).collect();
    Some(format!(
        "{}-{}-{nonce}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ))
}
fn send(frame: &Frame, name: &str, value: Value) {
    let Some(mut message) = process_message_create(Some(&CefString::from(name))) else {
        return;
    };
    let Some(arguments) = message.argument_list() else {
        return;
    };
    if arguments.set_string(0, Some(&CefString::from(value.to_string().as_str()))) == 0 {
        return;
    }
    frame.send_process_message(ProcessId::BROWSER, Some(&mut message));
}
fn unknown(reason: &str) -> Value {
    json!({"known":false,"reason":reason,"scroll_x":0,"scroll_y":0})
}
/// Registered before the first real navigation; never install this after page JS.
pub fn new_document_script(name: &str, nonce: &str, owned_blank:bool) -> String {
    let script=include_str!("../scripts/memory_safety.js").replace("/*NB_NATIVE_OWNED_BLANK*/false",if owned_blank{"true"}else{"false"});
    format!("(function(){{if(window!==top)return;const key={};const capture=window[key];delete window[key];if(typeof capture!=='function')return;capture({},(\n{}\n));}})();",
        json!(name), json!(nonce), script)
}
fn same_nonce(actual: &str, expected: &str) -> bool {
    if actual.len() != expected.len() {
        return false;
    }
    actual
        .bytes()
        .zip(expected.bytes())
        .fold(0u8, |diff, (a, b)| diff | (a ^ b))
        == 0
}
wrap_v8_handler! {
    struct Capture {
        document_key: (i32,String), token: String, lifetime: u64,
        name: String, nonce: String,
    }
    impl V8Handler {
        fn execute(&self,name:Option<&CefString>,object:Option<&mut V8Value>,arguments:Option<&[Option<V8Value>]>,retval:Option<&mut Option<V8Value>>,exception:Option<&mut CefString>)->i32 {
            let _=(object,exception);
            let Some(retval)=retval else{return 0;};
            *retval=v8_value_create_bool(0);
            if name.map(|n|n.to_string()).as_deref()!=Some(self.name.as_str()){return 1;}
            let Some(mut current)=v8_context_get_current_context() else{return 1;};
            let document=DOCUMENTS.with(|docs|docs.borrow().get(&self.document_key).cloned());
            let Some(document)=document.filter(|d|d.token==self.token && d.lifetime==self.lifetime && d.probe.is_none() && d.attempts<8) else{return 1;};
            if document.context.is_valid()==0 || document.context.is_same(Some(&mut current))==0{return 1;}
            // Verify the current native Browser identity as well as its numeric ID.
            let Some(frame)=current.frame().filter(|f|f.is_main()!=0) else{return 1;};
            let Some(mut browser)=frame.browser() else{return 1;};
            let seeded=SEEDS.with(|seeds|seeds.borrow().get(&self.document_key.0).is_some_and(|s|s.lifetime==self.lifetime && s.owner.is_same(Some(&mut browser))!=0));
            if !seeded || key(&browser,&frame)!=self.document_key{return 1;}
            let function=arguments.filter(|a|a.len()==2).and_then(|a|{
                let nonce=a[0].as_ref().filter(|v|v.is_string()!=0)?;
                let nonce=CefString::from(&nonce.string_value()).to_string();
                if !same_nonce(&nonce,&self.nonce){return None;}
                a[1].as_ref().filter(|v|v.is_valid()!=0 && v.is_function()!=0).cloned()
            });
            let captured=DOCUMENTS.with(|docs|{
                let mut docs=docs.borrow_mut();
                let Some(doc)=docs.get_mut(&self.document_key).filter(|d|d.token==self.token && d.lifetime==self.lifetime && d.probe.is_none() && d.attempts<8) else{return false;};
                doc.attempts+=1;
                if let Some(function)=function{doc.probe=Some(function);true}else{false}
            });
            if captured {
                *retval=v8_value_create_bool(1);
                diagnostic(format_args!("stage=native_capture_success"));
                send(&frame,"NB_MEMORY_DOCUMENT",json!({"token":self.token,"url":CefString::from(&frame.url()).to_string()}));
            }
            1
        }
    }
}
fn probe(document: &Document, current: &mut V8Context) -> Value {
    if document.context.is_valid() == 0 || document.context.is_same(Some(current)) == 0 {
        diagnostic(format_args!("stage=probe_context_changed"));
        return unknown("Document context changed");
    }
    let function_present = document.probe.is_some();
    let function_valid = document.probe.as_ref().is_some_and(|v| v.is_valid() != 0);
    let Some(function) = document.probe.as_ref().filter(|_| function_valid) else {
        diagnostic(format_args!("stage=probe_no_observer function_present={function_present} function_valid={function_valid}"));
        return unknown("Early safety observation unavailable");
    };
    if document.context.enter() == 0 {
        diagnostic(format_args!(
            "stage=probe_cannot_enter function_valid={function_valid}"
        ));
        return unknown("Cannot enter document context");
    }
    // Execute the saved closure through native V8, including after Page.freeze.
    // Do not depend on page timers, a mutable global name or Function.call.
    let value = function.execute_function(None, None);
    let value_present = value.is_some();
    let value_string = value.as_ref().is_some_and(|v| v.is_string() != 0);
    let raw = value
        .filter(|v| v.is_string() != 0)
        .map(|v| CefString::from(&v.string_value()).to_string());
    document.context.exit();
    let Some(raw) = raw.filter(|s| s.len() <= 2048) else {
        diagnostic(format_args!(
            "stage=probe_invalid_string value_present={value_present} value_string={value_string}"
        ));
        return unknown("Invalid safety result");
    };
    let Some(value) = serde_json::from_str::<Value>(&raw).ok().filter(|v| {
        v["known"].is_boolean()
            && (v["reason"].is_null() || v["reason"].as_str().is_some_and(|s| s.len() <= 160))
            && v["scroll_x"].as_f64().is_some_and(f64::is_finite)
            && v["scroll_y"].as_f64().is_some_and(f64::is_finite)
    }) else {
        diagnostic(format_args!("stage=probe_invalid_json_schema"));
        return unknown("Invalid safety result");
    };
    diagnostic(format_args!(
        "stage=probe_js known={} reason_code={}",
        value["known"].as_bool().unwrap_or(false),
        reason_code(&value["reason"])
    ));
    // Forward only the four declared fields, never arbitrary renderer data.
    json!({"known":value["known"],"reason":value["reason"],
        "scroll_x":value["scroll_x"],"scroll_y":value["scroll_y"]})
}

wrap_render_process_handler! {
    struct MemoryRenderer;
    impl RenderProcessHandler {
        fn on_browser_created(&self,browser:Option<&mut Browser>,extra_info:Option<&mut DictionaryValue>) {
            let (Some(browser),Some(extra))=(browser,extra_info) else{return;};
            let name=CefString::from(&extra.string(Some(&CefString::from("nb_memory_name")))).to_string();
            let nonce=CefString::from(&extra.string(Some(&CefString::from("nb_memory_nonce")))).to_string();
            // Names are native-generated, bounded identifiers; nonce is a separate secret.
            if name.len()<16 || name.len()>128 || !name.bytes().all(|b|b.is_ascii_alphanumeric() || b==b'_') || nonce.len()!=32 || !nonce.bytes().all(|b|b.is_ascii_hexdigit()){return;}
            let lifetime=NEXT.fetch_add(1,Ordering::Relaxed);
            SEEDS.with(|seeds|{seeds.borrow_mut().insert(browser.identifier(),Seed{owner:browser.clone(),name,nonce,lifetime});});
        }
        fn on_context_created(&self, browser:Option<&mut Browser>, frame:Option<&mut Frame>, context:Option<&mut V8Context>) {
            let (Some(browser),Some(frame),Some(context))=(browser,frame,context) else { return; };
            if frame.is_main()==0 { return; }
            let Some(mut main)=frame.v8_context() else { return; };
            if context.is_same(Some(&mut main))==0 { return; }
            let seed=SEEDS.with(|seeds|seeds.borrow().get(&browser.identifier()).cloned());
            let Some(seed)=seed.filter(|s|s.owner.is_same(Some(&mut *browser))!=0) else{return;};
            let Some(token)=new_token() else { return; };
            let document_key=key(browser,frame);
            DOCUMENTS.with(|docs|{docs.borrow_mut().insert(document_key.clone(),Document{token:token.clone(),context:context.clone(),probe:None,lifetime:seed.lifetime,attempts:0});});
            // CEF calls this before Blink installs conditional API properties.
            // Set only a native capture binding here; never evaluate the observer.
            let mut handler=Capture::new(document_key,token,seed.lifetime,seed.name.clone(),seed.nonce);
            let installed=context.global().and_then(|global|{
                let mut function=v8_value_create_function(Some(&CefString::from(seed.name.as_str())),Some(&mut handler))?;
                Some(global.set_value_bykey(Some(&CefString::from(seed.name.as_str())),Some(&mut function),sys::cef_v8_propertyattribute_t::V8_PROPERTY_ATTRIBUTE_DONTENUM.into())!=0)
            }).unwrap_or(false);
            diagnostic(format_args!("stage=native_binding installed={installed}"));
        }
        fn on_context_released(&self,browser:Option<&mut Browser>,frame:Option<&mut Frame>,context:Option<&mut V8Context>) {
            let (Some(browser),Some(frame),Some(context))=(browser,frame,context) else { return; };
            let key=key(browser,frame);
            DOCUMENTS.with(|docs|{
                let mut docs=docs.borrow_mut();
                if docs.get(&key).is_some_and(|doc|doc.context.is_same(Some(context))!=0){docs.remove(&key);}
            });
        }
        fn on_browser_destroyed(&self,browser:Option<&mut Browser>) {
            let Some(browser)=browser else{return;};
            let lifetime=SEEDS.with(|seeds|{
                let mut seeds=seeds.borrow_mut();
                if seeds.get(&browser.identifier()).is_some_and(|s|s.owner.is_same(Some(&mut *browser))!=0){seeds.remove(&browser.identifier()).map(|s|s.lifetime)}else{None}
            });
            if let Some(lifetime)=lifetime{DOCUMENTS.with(|docs|docs.borrow_mut().retain(|(id,_),doc|*id!=browser.identifier() || doc.lifetime!=lifetime));}
        }
        fn on_process_message_received(&self,browser:Option<&mut Browser>,frame:Option<&mut Frame>,source_process:ProcessId,message:Option<&mut ProcessMessage>)->i32 {
            let (Some(browser),Some(frame),Some(message))=(browser,frame,message) else { return 0; };
            if source_process!=ProcessId::BROWSER || CefString::from(&message.name()).to_string()!="NB_MEMORY_PROBE" { return 0; }
            if frame.is_main()==0 { return 1; }
            let Some(arguments)=message.argument_list().filter(|v|v.size()==1) else { return 1; };
            let raw=CefString::from(&arguments.string(0)).to_string();
            if raw.len()>4096 { return 1; }
            let Ok(value)=serde_json::from_str::<Value>(&raw) else { return 1; };
            let Some(request)=value["request"].as_str().filter(|s|!s.is_empty()&&s.len()<=256) else { return 1; };
            let Some(token)=value["token"].as_str().filter(|s|!s.is_empty()&&s.len()<=256) else { return 1; };
            // Clone out of TLS before executing JS: no RefCell borrow spans a
            // native V8 call that could re-enter a lifecycle callback.
            let document=DOCUMENTS.with(|docs|docs.borrow().get(&key(browser,frame)).cloned());
            let mut result=if let Some(document)=document.filter(|doc|doc.token==token) {
                if let Some(mut current)=frame.v8_context(){probe(&document,&mut current)}
                else{unknown("Current main-world context unavailable")}
            }else{unknown("Document token changed")};
            result["request"]=json!(request);result["token"]=json!(token);
            send(frame,"NB_MEMORY_PROBE_RESULT",result);
            1
        }
    }
}
pub(crate) fn handler() -> RenderProcessHandler {
    MemoryRenderer::new()
}
