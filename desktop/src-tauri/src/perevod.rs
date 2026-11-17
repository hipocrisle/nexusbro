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
            let _=self.tx.send(Sobytie::Sluzhebnoe(json!({"tip":"perevod","id":self.id,"token":self.token,"error":"Не удалось прочитать текст страницы"})));
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
    params.set_string(Some(&CefString::from("expression")),Some(&CefString::from(expression.as_str())));
    params.set_bool(Some(&CefString::from("returnByValue")),1);
    let _=h.execute_dev_tools_method(message,Some(&CefString::from("Runtime.evaluate")),Some(&mut params.clone()));
    Some(registration)
}
pub fn primenit(b:&Browser,token:&str,items:&[(u32,String)]){
    if let Some(f)=b.main_frame(){
        let args=json!([token,items.iter().map(|(id,text)|json!({"id":id,"text":text})).collect::<Vec<_>>()]);
        let js=format!("window.__nexusbroTranslation?.apply(...{})",args);
        f.execute_java_script(Some(&CefString::from(js.as_str())),None,0);
    }
}
pub fn original(b:&Browser){if let Some(f)=b.main_frame(){f.execute_java_script(Some(&CefString::from("window.__nexusbroTranslation?.restore()")),None,0);}}

type Key=(String,u64);
static TASKS:OnceLock<Mutex<HashMap<Key,(String,tokio::task::AbortHandle)>>>=OnceLock::new();
pub fn otmenit(super_id:&str,id:u64){
    if let Some((_,task))=TASKS.get_or_init(Default::default).lock().unwrap().remove(&(super_id.into(),id)){task.abort();}
}
pub fn otmenit_super(super_id:&str){
    let mut tasks=TASKS.get_or_init(Default::default).lock().unwrap();
    tasks.retain(|(super_key,_),(_,task)|{if super_key==super_id{task.abort();false}else{true}});
}
pub fn otmenit_vse(){let mut tasks=TASKS.get_or_init(Default::default).lock().unwrap();for (_,(_,task)) in tasks.drain(){task.abort();}}
struct Ochistit {key:Key,token:String}
impl Drop for Ochistit{fn drop(&mut self){let mut tasks=TASKS.get_or_init(Default::default).lock().unwrap();if tasks.get(&self.key).is_some_and(|(t,_)|t==&self.token){tasks.remove(&self.key);}}}
#[derive(serde::Deserialize)]
struct Tekst {id:u32,text:String}
pub fn zapros(super_id:String,data:&Value,port:u16,kanal:SyncSender<Komanda>){
    let Some(id)=data["id"].as_u64() else{return};
    let Some(token)=data["token"].as_str().filter(|s|s.len()<100).map(str::to_owned) else{return};
    let Ok(items)=serde_json::from_value::<Vec<Tekst>>(data["items"].clone()) else{return};
    if items.len()>1000||items.iter().map(|i|i.text.len()).sum::<usize>()>1_000_000{return;}
    otmenit(&super_id,id);
    let key=(super_id,id);let cleanup=Ochistit{key:key.clone(),token:token.clone()};
    let label=data["label"].as_str().unwrap_or("").to_owned();
    let task_token=token.clone();
    let mut tasks=TASKS.get_or_init(Default::default).lock().unwrap();
    let task=tauri::async_runtime::spawn(async move{
        let _cleanup=cleanup;
        let total=items.len() as u32;
        let client=reqwest::Client::builder().no_proxy().proxy(reqwest::Proxy::all(format!("http://127.0.0.1:{port}")).unwrap()).connect_timeout(std::time::Duration::from_secs(6)).timeout(std::time::Duration::from_secs(20)).build();
        let Ok(client)=client else{return};
        // Только RAM текущей страницы: одинаковый текст отправляется один раз,
        // ответ разворачивается обратно на все исходные DOM IDs. Приватный текст
        // не попадает в глобальный кэш, файлы или журнал.
        let mut unique:Vec<Tekst>=Vec::new();
        let mut by_text:HashMap<String,usize>=HashMap::new();
        let mut aliases:HashMap<u32,Vec<u32>>=HashMap::new();
        for item in items {
            let text=item.text.split_whitespace().collect::<Vec<_>>().join(" ");
            if let Some(index)=by_text.get(&text){aliases.entry(unique[*index].id).or_default().push(item.id);}
            else{by_text.insert(text.clone(),unique.len());unique.push(Tekst{id:item.id,text});}
        }
        let mut batches=std::collections::VecDeque::new();let mut batch=Vec::new();let mut bytes=0;
        for item in unique {
            let size=item.text.len()+24;
            if !batch.is_empty()&&(batch.len()>=24||bytes+size>2400){batches.push_back(std::mem::take(&mut batch));bytes=0;}
            bytes+=size;batch.push(item);
        }
        if !batch.is_empty(){batches.push_back(batch);}
        // Всего два сетевых запроса на страницу одновременно; не тысячи q и
        // не искусственные паузы после каждого успешно переведённого пакета.
        let mut active=tokio::task::JoinSet::new();
        let mut done=0;
        loop {
            while active.len()<2 {
                let Some(batch)=batches.pop_front() else{break};
                let client=client.clone();
                active.spawn(async move{perevesti_paket(&client,batch).await});
            }
            let Some(answer)=active.join_next().await else{break};
            let (mut result,failure)=match answer {
                Ok(Ok(items))=>(items,None),
                Ok(Err(error))=>(Vec::new(),Some(error)),
                Err(_)=>(Vec::new(),Some("Не удалось завершить перевод пакета страницы".into())),
            };
            let extra=result.iter().flat_map(|(id,text)|aliases.get(id).into_iter().flatten().map(move |alias|(*alias,text.clone()))).collect::<Vec<_>>();
            result.extend(extra);done+=result.len() as u32;
            if otpravit(&kanal,&label,Komanda::PerevodOtvet{id,token:token.clone(),items:result,done,total,error:failure.clone()}).await.is_err(){return;}
            if failure.is_some(){active.abort_all();break;}
        }
        if total==0{let _=otpravit(&kanal,&label,Komanda::PerevodOtvet{id,token,items:Vec::new(),done:0,total:0,error:Some("На странице нет доступного текста для перевода".into())}).await;}
    });
    tasks.insert(key,(task_token,task.inner().abort_handle()));
}
async fn otpravit(tx:&SyncSender<Komanda>,label:&str,cmd:Komanda)->Result<(),()>{let mut cmd=if label.is_empty(){cmd}else{Komanda::OknoKomanda{label:label.into(),komanda:Box::new(cmd)}};loop{match tx.try_send(cmd){Ok(())=>return Ok(()),Err(std::sync::mpsc::TrySendError::Full(c))=>{cmd=c;tokio::time::sleep(std::time::Duration::from_millis(10)).await;},Err(_)=>return Err(())}}}
// Разделители — private-use символы, не переводимые слова и не newline.
// Обычные переносы Google вправе объединять: это вызывало до одного запроса
// на каждый DOM-узел. При повреждённых границах текст не приклеивается не туда.
fn marker(index:usize)->String{format!("\u{e000}{index}\u{e001}")}
fn paket_text(items:&[Tekst])->String{
    items.iter().enumerate().map(|(i,item)|format!("{}\n{}",marker(i),item.text)).collect::<Vec<_>>().join("\n")
}
fn razobrat_paket(text:&str,items:&[Tekst])->Result<Vec<(u32,String)>,String>{
    let mut result=Vec::new();let mut rest=text;
    for (index,item) in items.iter().enumerate(){
        let current=marker(index);
        let Some(tail)=rest.strip_prefix(current.as_str()) else{return Err("Сервис изменил границы текста. Перевод остановлен, оригинал сохранён.".into())};
        let (translated,next)=if index+1<items.len(){
            let next_marker=marker(index+1);
            let Some((translated,_next))=tail.split_once(next_marker.as_str()) else{return Err("Сервис изменил границы текста. Перевод остановлен, оригинал сохранён.".into())};
            (translated,&tail[translated.len()..])
        }else{(tail,"")};
        if translated.contains('\u{e000}')||translated.contains('\u{e001}') {return Err("Сервис вернул неожиданные границы текста".into());}
        result.push((item.id,translated.trim().to_owned()));rest=next;
    }
    Ok(result)
}
async fn perevesti_paket(client:&reqwest::Client,items:Vec<Tekst>)->Result<Vec<(u32,String)>,String>{
    if items.len()==1{return Ok(vec![(items[0].id,perevesti(client,&items[0].text).await?)]);}
    // Не допускать неоднозначности, если исходный сайт сам содержит разделители.
    if items.iter().any(|i|i.text.contains('\u{e000}')||i.text.contains('\u{e001}')){
        return Err("Текст страницы содержит служебные разделители перевода".into());
    }
    razobrat_paket(&perevesti(client,&paket_text(&items)).await?,&items)
}
async fn perevesti(client:&reqwest::Client,text:&str)->Result<String,String>{
    // Общий предел всех окон/страниц: auto-режим нескольких вкладок не
    // превращается в шквал запросов. Permit освобождается и при abort.
    static REQUESTS:OnceLock<tokio::sync::Semaphore>=OnceLock::new();
    let _permit=REQUESTS.get_or_init(||tokio::sync::Semaphore::new(2)).acquire().await.map_err(|_|"Перевод остановлен".to_owned())?;
    let mut chunks=Vec::new();let mut chunk=String::new();
    // Обычно пакет <2400 bytes. Отдельный длинный DOM-узел ограничен снимком
    // 10k; режем безопасно по UTF-8, не превышая длину запроса сервиса.
    for c in text.chars(){if chunk.len()+c.len_utf8()>4000{chunks.push(std::mem::take(&mut chunk));}chunk.push(c);}
    if !chunk.is_empty(){chunks.push(chunk);}
    let mut out=String::new();
    for chunk in chunks {
        let endpoint=if std::env::var_os("NEXUSBRO_TEST_DATA").is_some(){std::env::var("NEXUSBRO_TEST_TRANSLATE_URL").unwrap_or_else(|_|"https://translate.googleapis.com/translate_a/single".into())}else{"https://translate.googleapis.com/translate_a/single".into()};
        let mut answer=None;let mut last="Нет ответа от сервиса перевода через выбранный выход".to_owned();
        for attempt in 0..2 {
            match client.get(&endpoint).query(&[("client","dict-chrome-ex"),("sl","auto"),("tl","ru"),("dt","t"),("q",chunk.as_str())]).send().await{
                Ok(r)=>{let status=r.status();if status.is_success(){
                    answer=Some(r.text().await.map_err(|_|"Не удалось прочитать ответ сервиса перевода".to_owned())?);break;
                }last=if status.as_u16()==429{"Сервис ограничил частоту запросов (429). Повторите перевод позже.".into()}else{format!("Сервис перевода вернул HTTP {}. Оригинал можно восстановить.",status.as_u16())};
                if status.as_u16()!=429&&!status.is_server_error(){break;}
                // Не игнорировать Retry-After и не держать страницу в скрытых
                // минутных повторах: при длинном cooldown нужен явный повтор.
                if let Some(wait)=r.headers().get(reqwest::header::RETRY_AFTER).and_then(|h|h.to_str().ok()).and_then(|s|s.parse::<u64>().ok()){
                    if wait>3{break;}
                    if attempt==0{tokio::time::sleep(std::time::Duration::from_secs(wait.max(1))).await;continue;}
                }
                },Err(e)=>{last=if e.is_timeout(){"Истекло время ожидания перевода через выбранный выход".into()}else{"Не удалось подключиться к сервису перевода через выбранный выход".into()};}
            }
            if attempt==0{tokio::time::sleep(std::time::Duration::from_millis(700)).await;}
        }
        let body=answer.ok_or(last)?;let v:Value=serde_json::from_str(&body).map_err(|_|"Сервис вернул некорректный ответ перевода".to_owned())?;
        let sentences=v[0].as_array().ok_or_else(||"Неизвестный формат ответа перевода".to_owned())?;
        for sentence in sentences {out.push_str(sentence[0].as_str().ok_or_else(||"В ответе перевода отсутствует текст".to_owned())?);}
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn paket_preserves_dom_boundaries_even_without_newlines(){
        let items=vec![Tekst{id:7,text:"Hello".into()},Tekst{id:9,text:"World".into()}];
        assert_eq!(razobrat_paket("\u{e000}0\u{e001}Привет\u{e000}1\u{e001}Мир",&items).unwrap(),vec![(7,"Привет".into()),(9,"Мир".into())]);
        assert!(razobrat_paket("Привет Мир",&items).is_err());
        assert!(razobrat_paket("\u{e000}1\u{e001}Мир\u{e000}0\u{e001}Привет",&items).is_err());
    }
}
