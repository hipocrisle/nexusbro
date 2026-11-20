//! Native-only transport and lifecycle protocol. No console/page messages grant
//! permission to unload. All responses remain bound to Browser + document token.
use crate::cef_vkladki::Sobytie;
use cef::*;
use serde_json::{json, Value};
use std::sync::mpsc::Sender;
pub fn receive(
    browser: &Browser,
    frame: &Frame,
    source: ProcessId,
    message: &ProcessMessage,
) -> Option<Value> {
    if source != ProcessId::RENDERER || frame.is_main() == 0 {
        return None;
    }
    if browser
        .main_frame()
        .map(|f| CefString::from(&f.identifier()).to_string())
        != Some(CefString::from(&frame.identifier()).to_string())
    {
        return None;
    }
    let name = CefString::from(&message.name()).to_string();
    if !matches!(
        name.as_str(),
        "NB_MEMORY_DOCUMENT" | "NB_MEMORY_PROBE_RESULT"
    ) {
        return None;
    }
    let args = message.argument_list()?;
    if args.size() != 1 {
        return None;
    }
    let text = CefString::from(&args.string(0)).to_string();
    if text.len() > 4096 {
        return None;
    }
    let mut value: Value = serde_json::from_str(&text).ok()?;
    if !value.is_object() {
        return None;
    }
    value["kind"] = json!(name);
    Some(value)
}
pub fn probe(browser: &Browser, request: &str, token: &str) -> bool {
    let Some(frame) = browser.main_frame() else {
        return false;
    };
    let Some(message) = process_message_create(Some(&CefString::from("NB_MEMORY_PROBE"))) else {
        return false;
    };
    let Some(args) = message.argument_list() else {
        return false;
    };
    let text = json!({"request":request,"token":token}).to_string();
    if args.set_string(0, Some(&CefString::from(text.as_str()))) == 0 {
        return false;
    }
    frame.send_process_message(ProcessId::RENDERER, Some(&mut message.clone()));
    true
}
wrap_dev_tools_message_observer! {
 struct FreezeObserver { id:u64, identity:i32, request:String, message:i32, tx:Sender<Sobytie> }
 impl DevToolsMessageObserver {
  fn on_dev_tools_method_result(&self,_browser:Option<&mut Browser>,message_id:i32,success:i32,_result:Option<&[u8]>){
    if message_id==self.message{let _=self.tx.send(Sobytie::MemoryFrozen(self.id,self.identity,self.request.clone(),success!=0));}
  }
 }
}
fn lifecycle(browser: &Browser, message: i32, state: &str) -> bool {
    crate::cdp::send(
        browser,
        message,
        "Page.setWebLifecycleState",
        json!({"state":state}),
    )
}
pub fn freeze(
    browser: &Browser,
    id: u64,
    request: &str,
    tx: Sender<Sobytie>,
) -> Option<Registration> {
    let message = crate::cdp::reserve(1)?;
    let registration =
        browser
            .host()?
            .add_dev_tools_message_observer(Some(&mut FreezeObserver::new(
                id,
                browser.identifier(),
                request.into(),
                message,
                tx,
            )))?;
    lifecycle(browser, message, "frozen").then_some(registration)
}
pub fn resume(browser: &Browser) {
    if let Some(message) = crate::cdp::reserve(1) {
        let _ = lifecycle(browser, message, "active");
    }
}
