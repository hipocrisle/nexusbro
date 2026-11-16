//! Поиск выполняется CEF в документе страницы, а не в оболочке.
use cef::*;
use std::sync::mpsc::Sender;
use crate::cef_vkladki::Sobytie;
// CEF сообщает количество и выбранное совпадение отдельными уведомлениями.
// Handler может запрашиваться заново, состояние принадлежит вкладке UI-потока.
thread_local!{static RESULTS:std::cell::RefCell<std::collections::HashMap<u64,(i32,i32,i32)>>=Default::default();}
pub fn ochistit(id:u64){RESULTS.with(|r|{r.borrow_mut().remove(&id);});}
wrap_find_handler! {
    struct Poisk { id: u64, tx: Sender<Sobytie> }
    impl FindHandler {
        fn on_find_result(&self, _browser: Option<&mut Browser>, identifier: i32,
            count: i32, _selection_rect: Option<&Rect>, active_match_ordinal: i32, final_update: i32) {
            if std::env::var_os("NEXUSBRO_TEST_DATA").is_some(){eprintln!("cef: find {} request={identifier} count={count} active={active_match_ordinal} final={final_update}",self.id);}
            let result=RESULTS.with(|r|{
                let mut r=r.borrow_mut();let state=r.entry(self.id).or_insert((identifier,-1,0));
                if identifier<state.0{return None;}
                if identifier!=state.0{*state=(identifier,-1,0);}
                if count>=0{state.1=count;}
                if active_match_ordinal>0{state.2=active_match_ordinal;}
                if state.1==0{state.2=0;}
                Some((state.1.max(0),if state.1>0{state.2.min(state.1)}else{0}))
            });
            if let Some((count,active))=result{let _=self.tx.send(Sobytie::Sluzhebnoe(serde_json::json!({"tip":"poisk","id":self.id,"count":count,"active":active})));}
        }
    }
}
pub fn handler(id:u64,tx:Sender<Sobytie>)->FindHandler{Poisk::new(id,tx)}
