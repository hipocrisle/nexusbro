//! DevTools принадлежат исходному BrowserHost и встраиваются в его контейнер.
use cef::*;
use std::sync::mpsc::Sender;
use crate::cef_vkladki::Sobytie;
wrap_life_span_handler! {
    struct Zhizn { id: u64, tx: Sender<Sobytie> }
    impl LifeSpanHandler {
        fn on_after_created(&self, browser: Option<&mut Browser>) {
            if let Some(b) = browser {
                let okno = crate::cef_vkladki::handle(b);
                let _ = crate::okna::sistema::pokazat(okno, false);
                let _ = self.tx.send(Sobytie::DevtoolsGotovy(self.id, okno));
            }
        }
        fn do_close(&self,_browser:Option<&mut Browser>)->i32{
            #[cfg(target_os="windows")]{let _=self.tx.send(Sobytie::DevtoolsZakrytOkno(self.id));return 1;}
            #[cfg(not(target_os="windows"))]{0}
        }
        fn on_before_close(&self, _browser: Option<&mut Browser>) {
            let _ = self.tx.send(Sobytie::DevtoolsZakryty(self.id));
        }
    }
}
wrap_keyboard_handler! {
    struct Klavishi { id:u64, tx:Sender<Sobytie> }
    impl KeyboardHandler {
        fn on_pre_key_event(&self,_browser:Option<&mut Browser>,event:Option<&KeyEvent>,_os_event:Option<&mut crate::cef_interfeys::NativeKeyEvent>,_is_keyboard_shortcut:Option<&mut i32>)->i32{
            if let Some(e)=event {
                if e.type_==KeyEventType::RAWKEYDOWN||e.type_==KeyEventType::KEYDOWN {
                    if e.windows_key_code==123||(e.windows_key_code==73&&e.modifiers&(1<<2)!=0&&e.modifiers&(1<<1)!=0){let _=self.tx.send(Sobytie::Komanda(crate::sessiya::Komanda::Instrumenty{id:self.id}));return 1;}
                }
            }
            0
        }
    }
}
wrap_client! {
    struct Klient { id: u64, tx: Sender<Sobytie> }
    impl Client {
        fn keyboard_handler(&self)->Option<KeyboardHandler>{Some(Klavishi::new(self.id,self.tx.clone()))}
        fn life_span_handler(&self) -> Option<LifeSpanHandler> { Some(Zhizn::new(self.id,self.tx.clone())) }
    }
}
pub fn klient(id:u64, tx:Sender<Sobytie>) -> Client { Klient::new(id,tx) }
