//! Поиск выполняется CEF в документе страницы, а не в оболочке.
use cef::*;
use std::sync::mpsc::Sender;
use crate::cef_vkladki::Sobytie;
wrap_find_handler! {
    struct Poisk { id: u64, tx: Sender<Sobytie> }
    impl FindHandler {
        fn on_find_result(&self, _browser: Option<&mut Browser>, _identifier: i32,
            count: i32, _selection_rect: Option<&Rect>, active_match_ordinal: i32, final_update: i32) {
            if final_update != 0 {
                let _ = self.tx.send(Sobytie::Sluzhebnoe(serde_json::json!({
                    "tip":"poisk", "id":self.id, "count":count, "active":active_match_ordinal
                })));
            }
        }
    }
}
pub fn handler(id: u64, tx: Sender<Sobytie>) -> FindHandler { Poisk::new(id, tx) }
