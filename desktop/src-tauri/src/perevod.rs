//! Перевод текста без навигации и без ослабления CSP страницы.
use cef::*;
use crate::{cef_vkladki::Sobytie,sessiya::Komanda};
use std::sync::mpsc::{Sender,SyncSender};
use std::sync::{Mutex,OnceLock};
use std::collections::HashMap;
use serde_json::{Value,json};

wrap_dev_tools_message_observer! {
    struct Snimok { id:u64, token:String, message:i32, tx:Sender<Sobytie> }
    impl DevToolsMessageObserver {
        fn on_dev_tools_method_result(&self,_browser:Option<&mut Browser>,message_id:i32,success:i32,result:Option<&[u8]>) {
            if message_id!=self.message{return;}
            let data=result.filter(|b|b.len()<2_000_000).and_then(|b|serde_json::from_slice::<Value>(b).ok());
            if success!=0 {
                if let Some(value)=data.as_ref().and_then(|v|v.get("result")).and_then(|v|v.get("value")) {
                    if value["token"]==self.token && value["items"].is_array() {
                        let _=self.tx.send(Sobytie::Sluzhebnoe(json!({"tip":"perevod-zapros","id":self.id,"token":self.token,"items":value["items"]})));
                        return;
                    }
                }
            }
            let _=self.tx.send(Sobytie::Sluzhebnoe(json!({"tip":"perevod","id":self.id,"error":"Не удалось прочитать текст страницы"})));
        }
    }
}
pub fn nachat(b:&Browser,id:u64,token:String,tx:Sender<Sobytie>)->Option<Registration>{
    let f=b.main_frame()?;let url=reqwest::Url::parse(&CefString::from(&f.url()).to_string()).ok()?;
    if !matches!(url.scheme(),"http"|"https"){return None;}
    let h=b.host()?;
    static NEXT:std::sync::atomic::AtomicI32=std::sync::atomic::AtomicI32::new(1_000_000);
    let message=NEXT.fetch_add(1,std::sync::atomic::Ordering::Relaxed);
    let registration=h.add_dev_tools_message_observer(Some(&mut Snimok::new(id,token.clone(),message,tx)))?;
    let expression=include_str!("../scripts/perevod.js").replace("__TOKEN__",&serde_json::to_string(&token).ok()?);
    let params=dictionary_value_create()?;
    params.set_string(Some(&CefString::from("expression")),Some(&CefString::from(expression)));
    params.set_bool(Some(&CefString::from("returnByValue")),1);
    let _=h.execute_dev_tools_method(message,Some(&CefString::from("Runtime.evaluate")),Some(&mut params.clone()));
    Some(registration)
}
pub fn primenit(b:&Browser,token:&str,items:&[(u32,String)]){
    if let Some(f)=b.main_frame(){
        let args=json!([token,items.iter().map(|(id,text)|json!({"id":id,"text":text})).collect::<Vec<_>>()]);
        let js=format!("window.__nexusbroTranslation?.apply(...{})",args);
        f.execute_java_script(Some(&CefString::from(js)),None,0);
    }
}
pub fn original(b:&Browser){if let Some(f)=b.main_frame(){f.execute_java_script(Some(&CefString::from("window.__nexusbroTranslation?.restore()")),None,0);}}

type Key=(String,u64);
static TASKS:OnceLock<Mutex<HashMap<Key,tokio::task::AbortHandle>>>=OnceLock::new();
pub fn otmenit(super_id:&str,id:u64){
    if let Some(task)=TASKS.get_or_init(Default::default).lock().unwrap().remove(&(super_id.into(),id)){task.abort();}
}
#[derive(serde::Deserialize)]
struct Tekst {id:u32,text:String}
pub fn zapros(super_id:String,data:&Value,port:u16,kanal:SyncSender<Komanda>){
    let Some(id)=data["id"].as_u64() else{return};
    let Some(token)=data["token"].as_str().filter(|s|s.len()<100).map(str::to_owned) else{return};
    let Ok(items)=serde_json::from_value::<Vec<Tekst>>(data["items"].clone()) else{return};
    if items.len()>1000||items.iter().map(|i|i.text.len()).sum::<usize>()>1_000_000{return;}
    otmenit(&super_id,id);
    let key=(super_id,id);
    let task=tauri::async_runtime::spawn(async move{
        let total=items.len() as u32;
        let client=reqwest::Client::builder().no_proxy().proxy(reqwest::Proxy::all(format!("http://127.0.0.1:{port}")).unwrap()).timeout(std::time::Duration::from_secs(12)).build();
        let Ok(client)=client else{return};
        let gate=std::sync::Arc::new(tokio::sync::Semaphore::new(4));
        let mut jobs=tokio::task::JoinSet::new();
        for item in items {let client=client.clone();let gate=gate.clone();jobs.spawn(async move{
            let _permit=gate.acquire().await.map_err(|_|())?;
            let text=perevesti(&client,&item.text).await.map_err(|_|())?;
            Ok::<_,()>((item.id,text))
        });}
        let mut done=0;let mut buffer=Vec::new();let mut error=false;
        while let Some(result)=jobs.join_next().await{
            match result {Ok(Ok(item))=>{buffer.push(item);done+=1;},_=>{error=true;jobs.abort_all();}}
            if !buffer.is_empty()&&(buffer.len()>=6||done==total||error){
                let _=otpravit(&kanal,Komanda::PerevodOtvet{id,token:token.clone(),items:std::mem::take(&mut buffer),done,total,error:if error{Some("Сервис перевода недоступен. Можно вернуть оригинал и повторить.".into())}else{None}}).await;
            }
            if error {let _=otpravit(&kanal,Komanda::PerevodOtvet{id,token:token.clone(),items:Vec::new(),done,total,error:Some("Сервис перевода недоступен. Можно вернуть оригинал и повторить.".into())}).await;break;}
        }
        if total==0{let _=otpravit(&kanal,Komanda::PerevodOtvet{id,token,items:Vec::new(),done:0,total:0,error:Some("На странице нет доступного текста для перевода".into())}).await;}
    });
    TASKS.get_or_init(Default::default).lock().unwrap().insert(key,task.inner().abort_handle());
}
async fn otpravit(tx:&SyncSender<Komanda>,mut cmd:Komanda)->Result<(),()>{loop{match tx.try_send(cmd){Ok(())=>return Ok(()),Err(std::sync::mpsc::TrySendError::Full(c))=>{cmd=c;tokio::time::sleep(std::time::Duration::from_millis(10)).await;},Err(_)=>return Err(())}}}
async fn perevesti(client:&reqwest::Client,text:&str)->Result<String,()>{
    let mut chunks=Vec::new();let mut chunk=String::new();
    for c in text.chars(){if chunk.len()+c.len_utf8()>900{chunks.push(std::mem::take(&mut chunk));}chunk.push(c);}
    if !chunk.is_empty(){chunks.push(chunk);}
    let mut out=String::new();
    for chunk in chunks {
        let r=client.get("https://translate.googleapis.com/translate_a/single").query(&[("client","gtx"),("sl","auto"),("tl","ru"),("dt","t"),("q",chunk.as_str())]).send().await.map_err(|_|())?;
        if !r.status().is_success(){return Err(());}
        let body=r.text().await.map_err(|_|())?;let v:Value=serde_json::from_str(&body).map_err(|_|())?;
        let sentences=v[0].as_array().ok_or(())?;
        for sentence in sentences {out.push_str(sentence[0].as_str().ok_or(())?);}
    }
    Ok(out)
}
