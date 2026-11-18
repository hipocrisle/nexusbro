//! Password DOM access uses native DevTools responses, never page console messages.
use crate::cef_vkladki::Sobytie;
use cef::*;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    sync::mpsc::Sender,
    sync::{
        atomic::{AtomicBool, AtomicI32, Ordering},
        Arc, Mutex,
    },
};
use zeroize::{Zeroize, Zeroizing};
#[derive(Clone, Serialize, Deserialize)]
pub struct Secret {
    pub username: String,
    pub password: String,
}
impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret([redacted])")
    }
}
impl Drop for Secret {
    fn drop(&mut self) {
        self.username.zeroize();
        self.password.zeroize();
    }
}
static NEXT: AtomicI32 = AtomicI32::new(3000000);
static ENABLED: AtomicBool = AtomicBool::new(false);
thread_local! {static ACTIVE:std::cell::RefCell<HashMap<String,(std::time::Instant,u64,Registration)>>=Default::default();}
fn command(b: &Browser, message: i32, method: &str, values: Value) -> bool {
    let Some(host) = b.host() else {
        return false;
    };
    let Some(params) = dictionary_value_create() else {
        return false;
    };
    for (key, v) in values.as_object().into_iter().flatten() {
        let key = CefString::from(key.as_str());
        match v {
            Value::String(s) => {
                params.set_string(Some(&key), Some(&CefString::from(s.as_str())));
            }
            Value::Bool(v) => {
                params.set_bool(Some(&key), *v as i32);
            }
            Value::Number(v) => {
                if let Some(v) = v.as_i64() {
                    params.set_int(Some(&key), v as i32);
                }
            }
            _ => {}
        }
    }
    host.execute_dev_tools_method(
        message,
        Some(&CefString::from(method)),
        Some(&mut params.clone()),
    ) != 0
}
fn origin(b: &Browser) -> Option<String> {
    let f = b.main_frame()?;
    crate::passwords::secure_origin(&CefString::from(&f.url()).to_string()).ok()
}
struct Watch {
    base: i32,
    id: u64,
    token: String,
    expected: String,
    setup: bool,
    context: Mutex<Option<i32>>,
    frame: Mutex<Option<String>>,
    unique: Mutex<Option<String>>,
    contexts: Mutex<HashMap<i32, (String, String)>>,
    expression: Mutex<Option<Zeroizing<String>>>,
    tx: Sender<Sobytie>,
}
wrap_dev_tools_message_observer! {
 struct Observer{state:Arc<Watch>}
 impl DevToolsMessageObserver{
  fn on_dev_tools_method_result(&self,browser:Option<&mut Browser>,message_id:i32,success:i32,result:Option<&[u8]>){
   let st=&self.state;if message_id<st.base||message_id>st.base+4{return;}
   let fail=||{if !st.setup{let _=st.tx.send(Sobytie::Sluzhebnoe(json!({"tip":"credential-result","token":st.token,"error":"Документ изменился или форма недоступна"})));}};
   let Some(b)=browser else{fail();return;};
   let data=result.filter(|b|b.len()<2_000_000).and_then(|b|serde_json::from_slice::<Value>(b).ok());
   if success==0||origin(b).as_deref()!=Some(st.expected.as_str()){fail();return;}
   let Some(data)=data else{fail();return;};
   if message_id==st.base{if !command(b,st.base+1,"Page.getFrameTree",json!({})){fail();}return;}
   if message_id==st.base+1{if let Some(frame)=data["frameTree"]["frame"]["id"].as_str(){*st.frame.lock().unwrap()=Some(frame.into());if !command(b,st.base+2,"Page.createIsolatedWorld",json!({"frameId":frame,"worldName":"NexusBro password manager","grantUniveralAccess":false})){fail();}}else{fail();}return;}
   if message_id==st.base+2{
    let Some(context)=data["executionContextId"].as_i64().and_then(|n|i32::try_from(n).ok())else{fail();return;};*st.context.lock().unwrap()=Some(context);let unique=st.contexts.lock().unwrap().get(&context).filter(|(_,frame)|Some(frame)==st.frame.lock().unwrap().as_ref()).map(|(unique,_)|unique.clone());let Some(unique)=unique else{fail();return;};*st.unique.lock().unwrap()=Some(unique.clone());
    if st.setup{if !command(b,st.base+3,"Runtime.addBinding",json!({"name":"__nexusbroPasswordCandidate","executionContextId":context})){fail();}}
    else{let expression=st.expression.lock().unwrap();if let Some(js)=expression.as_ref(){if !command(b,st.base+3,"Runtime.evaluate",json!({"expression":js.as_str(),"uniqueContextId":unique,"returnByValue":true,"silent":true})){fail();}}else{fail();}}
    return;
   }
   if st.setup{if message_id==st.base+3{if let Some(unique)=st.unique.lock().unwrap().clone(){let hook=include_str!("../scripts/password_capture.js").replace("__INSTALL__","true");let js=format!("(()=>{{if(location.origin!=={}||window.top!==window)throw Error('Document changed');return ({});}})()",json!(st.expected),hook.trim().trim_end_matches(';'));command(b,st.base+4,"Runtime.evaluate",json!({"expression":js,"uniqueContextId":unique,"returnByValue":true,"silent":true}));}}return;}
   if message_id==st.base+3{
    let value=data.get("exceptionDetails").is_none().then(||data["result"].get("value").cloned()).flatten();
    if let Some(value)=value{let _=st.tx.send(Sobytie::Sluzhebnoe(json!({"tip":"credential-result","token":st.token,"origin":st.expected,"value":value})));}else{fail();}
   }
  }
  fn on_dev_tools_event(&self,browser:Option<&mut Browser>,method:Option<&CefString>,params:Option<&[u8]>){
   let st=&self.state;let method=method.map(|m|m.to_string()).unwrap_or_default();
   let parsed=params.filter(|p|p.len()<2_000_000).and_then(|p|serde_json::from_slice::<Value>(p).ok());
   if method=="Runtime.executionContextCreated"{if let Some(p)=parsed{let c=&p["context"];if let (Some(id),Some(unique),Some(frame))=(c["id"].as_i64().and_then(|n|i32::try_from(n).ok()),c["uniqueId"].as_str(),c["auxData"]["frameId"].as_str()){let mut contexts=st.contexts.lock().unwrap();if contexts.len()<4096{contexts.insert(id,(unique.into(),frame.into()));}}}return;}
   if method=="Runtime.executionContextsCleared"{st.contexts.lock().unwrap().clear();return;}
   if method=="Runtime.executionContextDestroyed"{if let Some(p)=parsed{if let Some(id)=p["executionContextId"].as_i64(){st.contexts.lock().unwrap().remove(&(id as i32));}}return;}
   if !st.setup||!ENABLED.load(Ordering::Relaxed)||method!="Runtime.bindingCalled"{return;}
   let Some(b)=browser else{return;};if origin(b).as_deref()!=Some(st.expected.as_str()){return;}
   let Some(p)=params.filter(|p|p.len()<20000).and_then(|p|serde_json::from_slice::<Value>(p).ok())else{return;};
   let context=st.context.lock().unwrap().unwrap_or(-1);if st.contexts.lock().unwrap().get(&context).map(|(unique,_)|unique)!=st.unique.lock().unwrap().as_ref(){return;}
   if p["name"]!="__nexusbroPasswordCandidate"||p["executionContextId"].as_i64().map(|n|n as i32)!=*st.context.lock().unwrap(){return;}
   if let Some(value)=p["payload"].as_str().and_then(|s|serde_json::from_str::<Value>(s).ok()){
    let _=st.tx.send(Sobytie::Sluzhebnoe(json!({"tip":"credential-offer","id":st.id,"origin":st.expected,"value":value})));
   }
  }
 }
}
pub fn enabled(value: bool) {
    ENABLED.store(value, Ordering::Relaxed);
    if !value {
        ACTIVE.with(|m| m.borrow_mut().clear());
    }
}
pub fn closed(id: u64) {
    ACTIVE.with(|m| m.borrow_mut().retain(|_, (_, owner, _)| *owner != id));
}
pub fn install(b: &Browser, id: u64, private: bool, tx: Sender<Sobytie>) {
    if private || crate::privatnost::process_privatnyj() || !ENABLED.load(Ordering::Relaxed) {
        return;
    }
    let Some(expected) = origin(b) else {
        return;
    };
    let Some(host) = b.host() else {
        return;
    };
    let base = NEXT.fetch_add(8, Ordering::Relaxed);
    let state = Arc::new(Watch {
        base,
        id,
        token: String::new(),
        expected,
        setup: true,
        context: Mutex::new(None),
        frame: Mutex::new(None),
        unique: Mutex::new(None),
        contexts: Mutex::new(HashMap::new()),
        expression: Mutex::new(None),
        tx,
    });
    if let Some(reg) = host.add_dev_tools_message_observer(Some(&mut Observer::new(state))) {
        ACTIVE.with(|m| {
            m.borrow_mut()
                .insert(format!("hook-{id}"), (std::time::Instant::now(), id, reg));
        });
        command(b, base, "Runtime.enable", json!({}));
    }
}
pub fn request(
    b: &Browser,
    id: u64,
    token: String,
    expected: String,
    payload: Option<Secret>,
    tx: Sender<Sobytie>,
) -> bool {
    if origin(b).as_deref() != Some(expected.as_str()) {
        return false;
    }
    let Some(host) = b.host() else {
        return false;
    };
    let base = NEXT.fetch_add(8, Ordering::Relaxed);
    let js = if let Some(payload) = payload {
        include_str!("../scripts/password_fill.js")
            .replace("__ORIGIN__", &json!(expected).to_string())
            .replace("__USERNAME__", &json!(payload.username).to_string())
            .replace("__PASSWORD__", &json!(payload.password).to_string())
    } else {
        include_str!("../scripts/password_capture.js").replace("__INSTALL__", "false")
    };
    let js=format!("(()=>{{if(location.origin!=={}||window.top!==window)throw Error('Origin changed');return ({});}})()",json!(expected),js.trim().trim_end_matches(';'));
    let state = Arc::new(Watch {
        base,
        id,
        token: token.clone(),
        expected: expected.clone(),
        setup: false,
        context: Mutex::new(None),
        frame: Mutex::new(None),
        unique: Mutex::new(None),
        contexts: Mutex::new(HashMap::new()),
        expression: Mutex::new(Some(Zeroizing::new(js))),
        tx,
    });
    let Some(reg) = host.add_dev_tools_message_observer(Some(&mut Observer::new(state))) else {
        return false;
    };
    ACTIVE.with(|m| {
        let mut m = m.borrow_mut();
        m.retain(|k, (t, _, _)| k.starts_with("hook-") || t.elapsed().as_secs() < 20);
        m.insert(token.clone(), (std::time::Instant::now(), id, reg));
    });
    command(b, base, "Runtime.enable", json!({}))
}
pub fn completed(data: &Value) {
    if data["tip"] == "credential-result" {
        if let Some(token) = data["token"].as_str() {
            ACTIVE.with(|m| {
                m.borrow_mut().remove(token);
            });
        }
    }
}
