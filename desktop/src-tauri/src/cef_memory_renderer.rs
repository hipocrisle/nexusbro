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
}
thread_local! {
    static DOCUMENTS: RefCell<HashMap<(i32, String), Document>> = Default::default();
}
static NEXT: AtomicU64 = AtomicU64::new(1);

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
fn install(context: &V8Context) -> Option<V8Value> {
    if context.is_valid() == 0 || context.enter() == 0 {
        return None;
    }
    let mut value = None;
    let mut exception = None;
    let ok = context.eval(
        Some(&CefString::from(include_str!(
            "../scripts/memory_safety.js"
        ))),
        None,
        1,
        Some(&mut value),
        Some(&mut exception),
    );
    let result = value.filter(|v| ok != 0 && exception.is_none() && v.is_function() != 0);
    context.exit();
    result
}
fn probe(document: &Document, current: &mut V8Context) -> Value {
    if document.context.is_valid() == 0 || document.context.is_same(Some(current)) == 0 {
        return unknown("Document context changed");
    }
    let Some(function) = document.probe.as_ref().filter(|v| v.is_valid() != 0) else {
        return unknown("Early safety observation unavailable");
    };
    if document.context.enter() == 0 {
        return unknown("Cannot enter document context");
    }
    // Execute the saved closure through native V8, including after Page.freeze.
    // Do not depend on page timers, a mutable global name or Function.call.
    let value = function.execute_function(None, None);
    let raw = value
        .filter(|v| v.is_string() != 0)
        .map(|v| CefString::from(&v.string_value()).to_string());
    document.context.exit();
    let Some(raw) = raw.filter(|s| s.len() <= 2048) else {
        return unknown("Invalid safety result");
    };
    let Some(value) = serde_json::from_str::<Value>(&raw).ok().filter(|v| {
        v["known"].is_boolean()
            && (v["reason"].is_null() || v["reason"].as_str().is_some_and(|s| s.len() <= 160))
            && v["scroll_x"].as_f64().is_some_and(f64::is_finite)
            && v["scroll_y"].as_f64().is_some_and(f64::is_finite)
    }) else {
        return unknown("Invalid safety result");
    };
    // Forward only the four declared fields, never arbitrary renderer data.
    json!({"known":value["known"],"reason":value["reason"],
        "scroll_x":value["scroll_x"],"scroll_y":value["scroll_y"]})
}

wrap_render_process_handler! {
    struct MemoryRenderer;
    impl RenderProcessHandler {
        fn on_context_created(&self, browser:Option<&mut Browser>, frame:Option<&mut Frame>, context:Option<&mut V8Context>) {
            let (Some(browser),Some(frame),Some(context))=(browser,frame,context) else { return; };
            if frame.is_main()==0 { return; }
            // CDP password-manager isolated worlds must not replace this main-
            // world observer or announce a fictitious document navigation.
            let Some(mut main)=frame.v8_context() else { return; };
            if context.is_same(Some(&mut main))==0 { return; }
            let Some(token)=new_token() else { return; };
            let document=Document{token:token.clone(),context:context.clone(),probe:install(context)};
            DOCUMENTS.with(|docs|{docs.borrow_mut().insert(key(browser,frame),document);});
            send(frame,"NB_MEMORY_DOCUMENT",json!({"token":token}));
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
            if let Some(browser)=browser{DOCUMENTS.with(|docs|docs.borrow_mut().retain(|(id,_),_|*id!=browser.identifier()));}
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
