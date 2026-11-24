#[path = "cef_super_hibernation.rs"]
mod cef_super_hibernation;
pub fn take_hibernate_shutdown_token()->Option<crate::super_hibernation_protocol::Token>{cef_super_hibernation::take_shutdown()}
use crate::okna::{sistema, Mesto};
use crate::sessiya::{Komanda, Sessiya};
use cef::*;
use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::sync::mpsc::{channel, Receiver, Sender};

pub(crate) enum Sobytie {
    Sluzhebnoe(serde_json::Value),
    PoiskVybratPervoe(u64,u64,String,bool),
    DownloadNachata(u64,String),
    DevtoolsGotovy(u64),
    DevtoolsZakryty(u64),
    #[cfg(target_os="windows")]
    DevtoolsZakrytOkno(u64,u32),
    Komanda(Komanda),
    Zagolovok(u64, i32, String),
    Adres(u64, i32, String),
    Zagruzka(u64, i32, bool, bool, bool),
    Zakryta(u64, Option<i32>),
    MemoryRefused(u64, i32),
    RouteRefused(u64,i32),
    MemoryMessage(u64,i32,serde_json::Value),
    MemoryFrozen(u64,i32,String,bool),
    MemoryLoaded(u64,i32),
    MainLoadFailed(u64,i32,String,u64,String,i32),
    NavigationFailed(u64,i32,String,u64,i32),
    Ikonka(u64, String, String),
    AdresnyiVvod,
    #[cfg(target_os = "windows")]
    ZakrytOkno(u64, i32),
}

wrap_display_handler! {
    struct VidStranicy { id: u64, tx: Sender<Sobytie>, privatnaya:bool, bootstrap:std::sync::Arc<crate::memory_bootstrap::Bootstrap> }
    impl DisplayHandler {
        fn on_media_access_change(&self,browser:Option<&mut Browser>,has_video_access:i32,has_audio_access:i32){if let Some(b)=browser{crate::memory_guard::capture(b.identifier(),has_video_access!=0||has_audio_access!=0);}}
        fn on_console_message(&self,browser:Option<&mut Browser>,_level:LogSeverity,message:Option<&CefString>,_source:Option<&CefString>,_line:i32)->i32{
            if let Some(data)=message.map(|m|m.to_string()).and_then(|m|m.strip_prefix("__NEXUSBRO_COSMETIC__").map(str::to_owned)).filter(|s|s.len()<16000).and_then(|s|serde_json::from_str::<serde_json::Value>(&s).ok()){
                let host=browser.and_then(|b|b.main_frame()).and_then(|f|reqwest::Url::parse(&CefString::from(&f.url()).to_string()).ok()).and_then(|u|u.host_str().map(str::to_owned)).unwrap_or_default();
                let _=self.tx.send(Sobytie::Sluzhebnoe(serde_json::json!({"tip":"blocked_dom","id":self.id,"host":host,"rules":data,"privatnaya":self.privatnaya})));return 1;
            }
            self.privatnaya as i32
        }

        fn on_favicon_urlchange(&self, browser: Option<&mut Browser>, icon_urls: Option<&mut CefStringList>) {
            crate::cef_interfeys::ikonka(self.id, self.tx.clone(), browser, icon_urls);
        }
        fn on_title_change(&self, browser: Option<&mut Browser>, title: Option<&CefString>) {
            if let (Some(browser),Some(frame))=(browser.as_ref(),frame.as_ref()){crate::cef_youtube::on_loaded(browser,frame);}
            if self.bootstrap.waiting_for_target(){return;}
            if let (Some(b),Some(t)) = (browser,title) { let _ = self.tx.send(Sobytie::Zagolovok(self.id, b.identifier(), t.to_string())); }
        }
        // ⛔ Без `mut` в объявлении: грамматика `wrap_*!` принимает только
        // шаблон `имя: тип`, иначе «no rules expected». Внутри берём `&mut`.
        fn on_address_change(&self, browser: Option<&mut Browser>, frame: Option<&mut Frame>, url: Option<&CefString>) {
            if frame.as_ref().map(|f| f.is_main() != 0).unwrap_or(false) {
                if let Some(u) = url {
                    let u = u.to_string();
                    if self.bootstrap.skip_address(&u){return;}
                    if let Some(b)=browser.as_ref(){let _ = self.tx.send(Sobytie::Adres(self.id, b.identifier(), u.clone()));}

                }
            }
        }
    }
}

wrap_load_handler! {
    struct ZagruzkaStranicy { id: u64, tx: Sender<Sobytie>, privatnaya:bool, bootstrap:std::sync::Arc<crate::memory_bootstrap::Bootstrap> }
    impl LoadHandler {
        fn on_load_end(&self, browser: Option<&mut Browser>, frame: Option<&mut Frame>, _http_status_code: i32) {
            if (200..600).contains(&_http_status_code)&&frame.as_ref().is_some_and(|f|f.is_main()!=0){if let Some(b)=browser.as_ref(){crate::memory_guard::load_succeeded(b.identifier());}}
            if self.bootstrap.waiting_for_target(){return;}
            if frame.as_ref().is_some_and(|f|f.is_main()!=0){if let Some(b)=browser.as_ref(){crate::password_cef::install(b,self.id,self.privatnaya,self.tx.clone());let _=self.tx.send(Sobytie::MemoryLoaded(self.id,b.identifier()));}}
            if !crate::borba_s_reklamoy::vklyuchena() { return; }
            if let Some(f) = frame {
                if f.is_main() != 0 {
                    let url = CefString::from(&f.url()).to_string();
                    if let Some(js) = crate::borba_s_reklamoy::skript_dlya(&url) {
                        f.execute_java_script(Some(&CefString::from(js)), None, 0);
                    }
                }
            }
        }
        fn on_load_error(&self, _browser: Option<&mut Browser>, frame: Option<&mut Frame>, error_code: Errorcode,
            error_text: Option<&CefString>, _failed_url: Option<&CefString>) {
            if error_code!=Errorcode::ABORTED&&frame.as_ref().is_some_and(|f|f.is_main()!=0){if let (Some(browser),Some(url))=(_browser.as_ref(),_failed_url){
                let failed_url=url.to_string();let identity=browser.identifier();
                let native_code:cef::sys::cef_errorcode_t=error_code.into();
                if let Some(epoch)=crate::memory_guard::diagnostic_epoch(identity,&failed_url){
                    let _=self.tx.send(Sobytie::NavigationFailed(self.id,identity,failed_url.clone(),epoch,native_code as i32));
                }
                let accepted=crate::memory_guard::native_load_error(identity,&failed_url);
                eprintln!("NEXUSBRO_ROUTE_ERROR browser={identity} native_candidate={accepted}");
                if accepted{
                    if let Some(epoch)=crate::memory_guard::native_error_epoch(identity,&failed_url){
                        if let Some(logical_target)=self.bootstrap.mark_terminal_failure(identity,&failed_url,epoch){
                            // Loading(false) may already have been suppressed;
                            // settle the failed request without claiming success.
                            let _=self.tx.send(Sobytie::MainLoadFailed(self.id,identity,failed_url,epoch,logical_target,native_code as i32));
                            eprintln!("NEXUSBRO_ROUTE_ERROR browser={identity} terminal_initial=true");
                        }
                    }
                }
            }}
            if !self.privatnaya && frame.map(|f| f.is_main() != 0).unwrap_or(false) {
                eprintln!("cef: вкладка {} load_error {:?}: {}", self.id, error_code, error_text.map(|t| t.to_string()).unwrap_or_default());
            }
        }
        fn on_loading_state_change(&self, browser: Option<&mut Browser>, is_loading: i32, can_go_back: i32, can_go_forward: i32) {
            if is_loading==0&&self.bootstrap.waiting_for_target(){return;}
            if let Some(b)=browser{let _ = self.tx.send(Sobytie::Zagruzka(self.id, b.identifier(), is_loading != 0, can_go_back != 0, can_go_forward != 0));}
        }
    }
}

wrap_life_span_handler! {
    struct ZhiznStranicy { id: u64, tx: Sender<Sobytie>, privatnaya:bool, bootstrap:std::sync::Arc<crate::memory_bootstrap::Bootstrap> }
    impl LifeSpanHandler {
        fn on_after_created(&self,browser:Option<&mut Browser>){if let Some(b)=browser{crate::memory_bootstrap::start(b,self.bootstrap.clone());}}
        fn on_before_popup(&self, _browser: Option<&mut Browser>, _frame: Option<&mut Frame>, _popup_id: i32,
            target_url: Option<&CefString>, _target_frame_name: Option<&CefString>, target_disposition: WindowOpenDisposition,
            _user_gesture: i32, _popup_features: Option<&PopupFeatures>, _window_info: Option<&mut WindowInfo>,
            _client: Option<&mut Option<Client>>, _settings: Option<&mut BrowserSettings>,
            _extra_info: Option<&mut Option<DictionaryValue>>, _no_javascript_access: Option<&mut i32>) -> i32 {
            if let Some(u) = target_url {
                crate::cef_interfeys::open_link(self.id,&u.to_string(),self.privatnaya||target_disposition==WindowOpenDisposition::OFF_THE_RECORD,target_disposition==WindowOpenDisposition::NEW_BACKGROUND_TAB,&self.tx);
            }
            1
        }
        fn do_close(&self, browser: Option<&mut Browser>) -> i32 {
            #[cfg(target_os = "windows")]
            {
                // CEF по умолчанию посылает WM_CLOSE верхнему родителю — Tauri!
                // Разрушаем только собственный HWND вкладки на CEF UI-потоке,
                // после возврата из callback, чтобы не войти в teardown рекурсивно.
                if let Some(b)=browser{let _ = self.tx.send(Sobytie::ZakrytOkno(self.id,b.identifier()));}
                return 1;
            }
            #[cfg(not(target_os = "windows"))]
            { 0 }
        }
        fn on_before_close(&self, browser: Option<&mut Browser>) {
            if let Some(browser)=browser{
                let identity=browser.identifier();
                crate::cef_youtube::documents().revoke(identity);
                self.bootstrap.close(identity);
                SLEEP_CLOSES.with(|m|{m.borrow_mut().remove(&identity);});
                ROUTE_CLOSES.with(|m|{m.borrow_mut().remove(&identity);});
                crate::memory_guard::closed(identity);
                crate::cef_interfeys::cancel_context(identity);
                crate::password_cef::closed(self.id);
                let _ = self.tx.send(Sobytie::Zakryta(self.id,Some(identity)));
            }
        }
    }
}

thread_local! {static ROUTE_CLOSES:std::cell::RefCell<std::collections::HashSet<i32>>=Default::default();}
thread_local! {static SLEEP_CLOSES:std::cell::RefCell<std::collections::HashSet<i32>>=Default::default();}
wrap_jsdialog_handler! {
    struct MemoryDialog { id:u64, tx:Sender<Sobytie> }
    impl JsdialogHandler {
        fn on_before_unload_dialog(&self,browser:Option<&mut Browser>,_message_text:Option<&CefString>,_is_reload:i32,callback:Option<&mut JsdialogCallback>)->i32 {
            let Some(browser)=browser else{return 0;};
            let identity=browser.identifier();
            if ROUTE_CLOSES.with(|m|m.borrow_mut().remove(&identity)){
                if let Some(callback)=callback{callback.cont(0,None);let _=self.tx.send(Sobytie::RouteRefused(self.id,identity));return 1;}
                return 0;
            }
            if SLEEP_CLOSES.with(|m|m.borrow_mut().remove(&identity)) {
                if let Some(callback)=callback {
                    callback.cont(0,None);
                    let _=self.tx.send(Sobytie::MemoryRefused(self.id,identity));
                    return 1;
                }
            }
            0
        }
    }
}

wrap_focus_handler! {
    struct FokusStranicy;
    impl FocusHandler {
        fn on_set_focus(&self, _browser: Option<&mut Browser>, source: FocusSource) -> i32 {
            // Загрузка страницы не должна отбирать клавиатуру у адресной строки.
            // Физический клик по странице (SYSTEM) CEF обрабатывает сам.
            i32::from(source == FocusSource::NAVIGATION)
        }
    }
}

wrap_request_handler! {
    struct SboyStranicy { id: u64, privatnaya:bool, tx:Sender<Sobytie>, bootstrap:std::sync::Arc<crate::memory_bootstrap::Bootstrap> }
    impl RequestHandler {
        fn on_before_browse(&self,browser:Option<&mut Browser>,frame:Option<&mut Frame>,request:Option<&mut Request>,_user_gesture:i32,_is_redirect:i32)->i32{
            if frame.as_ref().is_some_and(|f|f.is_main()!=0){if let Some(browser)=browser{
                if let Some(request)=request.as_ref(){let url=CefString::from(&request.url()).to_string();crate::cef_youtube::on_navigation(browser.identifier(),&url);if self.bootstrap.allow_initial_blank(&url){return 0;}if self.bootstrap.defer(&url){return 1;}}
                crate::cef_interfeys::cancel_context(browser.identifier());
                let url=request.as_ref().map(|r|CefString::from(&r.url()).to_string()).unwrap_or_default();
                let method=request.as_ref().map(|r|CefString::from(&r.method()).to_string()).unwrap_or_default();
                let get=method=="GET"||(method.is_empty()&&self.bootstrap.owned_blank_navigation(&url));
                crate::memory_guard::navigation_to(browser.identifier(),get,&url);
            }}
            0
        }
        fn on_open_urlfrom_tab(&self,_browser:Option<&mut Browser>,_frame:Option<&mut Frame>,target_url:Option<&CefString>,target_disposition:WindowOpenDisposition,_user_gesture:i32)->i32{
            let new_tab=matches!(target_disposition,WindowOpenDisposition::NEW_BACKGROUND_TAB|WindowOpenDisposition::NEW_FOREGROUND_TAB|WindowOpenDisposition::NEW_WINDOW|WindowOpenDisposition::NEW_POPUP|WindowOpenDisposition::OFF_THE_RECORD);
            if !new_tab{return 0;}
            if let Some(url)=target_url{crate::cef_interfeys::open_link(self.id,&url.to_string(),self.privatnaya||target_disposition==WindowOpenDisposition::OFF_THE_RECORD,target_disposition==WindowOpenDisposition::NEW_BACKGROUND_TAB,&self.tx);}
            1
        }
        fn on_render_process_terminated(&self, _browser: Option<&mut Browser>, status: TerminationStatus,
            error_code: i32, error_string: Option<&CefString>) {
            if self.privatnaya {return;}
            eprintln!("cef: вкладка {} renderer terminated {:?}, code={}: {}", self.id, status, error_code,
                error_string.map(|s| s.to_string()).unwrap_or_default());
        }
        fn resource_request_handler(&self, _browser: Option<&mut Browser>, frame: Option<&mut Frame>,
            _request: Option<&mut Request>, is_navigation: i32, _is_download: i32,
            request_initiator: Option<&CefString>, _disable_default_handling: Option<&mut i32>) -> Option<ResourceRequestHandler> {
            // Вызов ресурса: прокси видит только ИМЯ УЗЛА, а реклама по
            // пути (`/promo`, `/ads/gif`) и трекеры-картинки так проходят.
            // Здесь виден полный адрес ещё до загрузки.
            // ⛔ Документ верхней навигации (`is_navigation`) не трогаем
            // никогда: иначе поиск Яндекса с рекламным путём не откроется.
            // ⛔ Решение принимает САМ адрес запроса. Инициатор в решении не
            // участвует: страница на сайте везде кидает трекеры-картинки, и
            // «сложить инициатора в решатель» означало бы вырезать сторонние
            // запросы любого сайта (это ломалось бы на логинах/капче).
            if !crate::borba_s_reklamoy::vklyuchena() { return None; }
            let initiator = request_initiator.map(|s| s.to_string()).unwrap_or_default();
            let navigaciya = is_navigation != 0 && frame.as_ref().map(|f| f.is_main() != 0).unwrap_or(true);
            Some(crate::borba_s_reklamoy::handler_resursov(self.id, &initiator, navigaciya,self.tx.clone(),self.privatnaya).into())
        }
    }
}

wrap_client! {
    struct KlientVkladki { id: u64, tx: Sender<Sobytie>, privatnaya:bool, bootstrap:std::sync::Arc<crate::memory_bootstrap::Bootstrap> }
    impl Client {
        fn on_process_message_received(&self,browser:Option<&mut Browser>,frame:Option<&mut Frame>,source_process:ProcessId,message:Option<&mut ProcessMessage>)->i32{
            let (Some(b),Some(f),Some(m))=(browser,frame,message)else{return 0;};
            if let Some(data)=crate::memory_cef::receive(b,f,source_process,m){
                if !self.bootstrap.approved(){
                    if data["kind"]=="NB_MEMORY_DOCUMENT"{eprintln!("NEXUSBRO_MEMORY_DOCUMENT browser={} accepted=false bootstrap_approved=false",b.identifier());}
                    return 1;
                }
                if data["kind"]=="NB_MEMORY_DOCUMENT"{
                    let current=CefString::from(&f.url()).to_string();
                    let Some(token)=data["token"].as_str().filter(|s|!s.is_empty()&&s.len()<128)else{return 1;};
                    let owned_blank=self.bootstrap.owned_blank_navigation(&current)&&data["url"].as_str()==Some("about:blank");
                    let uri_matches=data["url"].as_str()==Some(current.as_str())||owned_blank;
                    let initial_blank=self.bootstrap.rejects_initial_document(&current);
                    let internal_error=!initial_blank&&crate::memory_guard::error_document(b.identifier(),&current,data["url"].as_str().unwrap_or(""),token.into());
                    if (!uri_matches&&!internal_error)||initial_blank{eprintln!("NEXUSBRO_MEMORY_DOCUMENT browser={} accepted=false uri_matches={uri_matches} initial_blank={initial_blank}",b.identifier());return 1;}
                    if internal_error{eprintln!("NEXUSBRO_ROUTE_ERROR browser={} internal_document_confirmed=true",b.identifier());}
                    else{crate::memory_guard::document(b.identifier(),token.into());}
                    if owned_blank{eprintln!("NEXUSBRO_MEMORY_DOCUMENT browser={} accepted=true owned_blank=true guard={}",b.identifier(),route_guard_code(crate::memory_guard::reason(b.identifier()).unwrap_or("")));}
                }
                let _=self.tx.send(Sobytie::MemoryMessage(self.id,b.identifier(),data));return 1;
            }0
        }

        fn jsdialog_handler(&self)->Option<JsdialogHandler>{Some(MemoryDialog::new(self.id,self.tx.clone()))}
        fn download_handler(&self)->Option<DownloadHandler>{Some(crate::zagruzki::handler(self.id,self.tx.clone(),self.privatnaya))}
        fn find_handler(&self) -> Option<FindHandler> { Some(crate::poisk::handler(self.id, self.tx.clone())) }
        fn keyboard_handler(&self) -> Option<KeyboardHandler> { Some(crate::cef_interfeys::klaviatura(self.id, self.tx.clone())) }
        fn context_menu_handler(&self) -> Option<ContextMenuHandler> { Some(crate::cef_interfeys::menyu(self.id, self.tx.clone(), self.privatnaya)) }
        fn focus_handler(&self) -> Option<FocusHandler> { Some(FokusStranicy::new()) }
        fn request_handler(&self) -> Option<RequestHandler> { Some(SboyStranicy::new(self.id, self.privatnaya, self.tx.clone(),self.bootstrap.clone())) }
        fn display_handler(&self) -> Option<DisplayHandler> { Some(VidStranicy::new(self.id, self.tx.clone(), self.privatnaya,self.bootstrap.clone())) }
        fn load_handler(&self) -> Option<LoadHandler> { Some(ZagruzkaStranicy::new(self.id, self.tx.clone(), self.privatnaya,self.bootstrap.clone())) }
        fn life_span_handler(&self) -> Option<LifeSpanHandler> { Some(ZhiznStranicy::new(self.id, self.tx.clone(), self.privatnaya,self.bootstrap.clone())) }
    }
}

// Идентификаторы сохраняются до закрытия всей приватной группы: загрузка
// может продолжаться после закрытия исходной вкладки, но не переживает группу.
thread_local! {static PRIVATE_IDS:std::cell::RefCell<std::collections::HashSet<u64>>=Default::default();}

struct Konteyner {
    okno: u32,
    routes: std::rc::Rc<std::cell::RefCell<crate::cef_route_context::Registry>>,
    choices:std::cell::RefCell<HashMap<u64,crate::tab_routes::RouteChoice>>,
    #[cfg(target_os = "linux")]
    svyaz: x11rb::rust_connection::RustConnection,
}

impl Konteyner {
    fn sozdat(roditel: u64, m: Mesto, routes: std::rc::Rc<std::cell::RefCell<crate::cef_route_context::Registry>>) -> anyhow::Result<Self> {
        #[cfg(target_os = "linux")]
        {
            use x11rb::connection::Connection;
            use x11rb::protocol::xproto::*;
            let (svyaz, ekran) = x11rb::connect(None)?;
            let ekran = &svyaz.setup().roots[ekran];
            let okno = svyaz.generate_id()?;
            svyaz
                .create_window(
                    ekran.root_depth,
                    okno,
                    roditel as u32,
                    m.x as i16,
                    m.y as i16,
                    m.w as u16,
                    m.h as u16,
                    0,
                    WindowClass::INPUT_OUTPUT,
                    ekran.root_visual,
                    &CreateWindowAux::new().colormap(ekran.default_colormap).background_pixel(0x171a24),
                )?
                .check()?;
            Ok(Self { okno, svyaz, routes,choices:Default::default() })
        }
        #[cfg(target_os = "windows")]
        {
            use windows::Win32::Foundation::HWND;
            use windows::Win32::UI::WindowsAndMessaging::*;
            let okno = unsafe {
                CreateWindowExW(
                    WINDOW_EX_STYLE(0),
                    windows::core::w!("STATIC"),
                    None,
                    WS_CHILD | WS_CLIPCHILDREN | WS_CLIPSIBLINGS,
                    m.x,
                    m.y,
                    m.w as i32,
                    m.h as i32,
                    HWND(roditel as usize as _),
                    None,
                    None,
                    None,
                )?
            };
            crate::fon_konteynera::set(okno.0 as usize as u64,0x171a24);
            Ok(Self {
                okno: okno.0 as usize as u32,
                routes,choices:Default::default(),
            })
        }
        #[cfg(not(any(target_os = "windows", target_os = "linux")))]
        {
            let _ = (roditel, m);
            anyhow::bail!("система не поддерживается")
        }
    }

    fn fon(&self,rgb:u32){
        crate::fon_konteynera::set(self.okno as u64,rgb);
        #[cfg(target_os="linux")]{use x11rb::{connection::Connection,protocol::xproto::*};let _=self.svyaz.change_window_attributes(self.okno,&ChangeWindowAttributesAux::new().background_pixel(rgb));let _=self.svyaz.clear_area(false,self.okno,0,0,0,0);let _=self.svyaz.flush();}
    }

    fn razmer(&self) -> Option<(u32, u32)> {
        #[cfg(target_os = "linux")]
        {
            use x11rb::protocol::xproto::*;
            let r = self.svyaz.get_geometry(self.okno).ok()?.reply().ok()?;
            Some((r.width as u32, r.height as u32))
        }
        #[cfg(target_os = "windows")]
        {
            use windows::Win32::{
                Foundation::{HWND, RECT},
                UI::WindowsAndMessaging::GetClientRect,
            };
            let mut r = RECT::default();
            unsafe {
                GetClientRect(HWND(self.okno as usize as _), &mut r).ok()?;
            }
            Some(((r.right - r.left) as u32, (r.bottom - r.top) as u32))
        }
        #[cfg(not(any(target_os = "windows", target_os = "linux")))]
        {
            None
        }
    }
}

impl Drop for Konteyner {
    fn drop(&mut self){
        #[cfg(target_os="windows")]unsafe{let _=windows::Win32::UI::WindowsAndMessaging::DestroyWindow(windows::Win32::Foundation::HWND(self.okno as usize as _));}
        #[cfg(target_os="linux")]{use x11rb::{connection::Connection,protocol::xproto::ConnectionExt};let _=self.svyaz.destroy_window(self.okno);let _=self.svyaz.flush();}
    }
}

pub(crate) fn handle(b: &Browser) -> u32 {
    b.host()
        .map(|h| {
            #[cfg(target_os = "windows")]
            {
                h.window_handle().0 as usize as u32
            }
            #[cfg(not(target_os = "windows"))]
            {
                h.window_handle() as u32
            }
        })
        .unwrap_or(0)
}

fn sozdat_brauzer(
    k: &Konteyner,
    id: u64,
    adres: &str,
    tx: &Sender<Sobytie>,
    privatnaya:bool,
) -> anyhow::Result<Option<Browser>> {
    let choice=k.choices.borrow().get(&id).cloned().unwrap_or_default();
    let Some(route)=k.routes.borrow().descriptor(&choice)else{return Ok(None);};
    sozdat_brauzer_routed(k,id,adres,tx,privatnaya,&route)
}
fn sozdat_brauzer_routed(k:&Konteyner,id:u64,adres:&str,tx:&Sender<Sobytie>,privatnaya:bool,route:&crate::tab_routes::NativeRoute)->anyhow::Result<Option<Browser>>{
    let privatnaya=privatnaya||crate::privatnost::process_privatnyj();
    let (w, h) = k.razmer().unwrap_or((1200, 700));
    #[cfg(target_os = "windows")]
    let roditel = sys::HWND(k.okno as usize as *mut sys::HWND__);
    #[cfg(not(target_os = "windows"))]
    let roditel = k.okno as sys::cef_window_handle_t;
    let info = WindowInfo {
        runtime_style: RuntimeStyle::ALLOY,
        ..WindowInfo::default()
    }
    .set_as_child(
        roditel,
        &Rect {
            x: 0,
            y: 0,
            width: w as i32,
            height: h as i32,
        },
    );
    let Some(mut context)=k.routes.borrow_mut().prepare(&route,privatnaya).unwrap_or(None)else{return Ok(None);};
    if privatnaya{PRIVATE_IDS.with(|ids|{ids.borrow_mut().insert(id);});}
    let bootstrap=crate::memory_bootstrap::Bootstrap::new(adres)?;
    let extra=bootstrap.extra()?;
    let b = browser_host_create_browser_sync(
        Some(&info),
        Some(&mut KlientVkladki::new(id, tx.clone(), privatnaya,bootstrap)),
        // An explicit initial blank starts the renderer before CDP waits for its ACK.
        Some(&CefString::from("about:blank")),
        Some(&BrowserSettings::default()),
        Some(&mut extra.clone()),
        Some(&mut context),
    )
    .ok_or_else(|| anyhow::anyhow!("не создалась обычная вкладка"))?;
    if let Some(h) = b.host() {
        h.set_zoom_level(
            (crate::nastrojki::zagruzit().masshtab_stranic as f64 / 100.0).ln() / 1.2_f64.ln(),
        );
    }
    k.routes.borrow_mut().record_browser(b.identifier(),route.key.clone());
    Ok(Some(b))
}

fn pokazat(s: &Sessiya, b: &HashMap<u64, Browser>) {
    for (id, b) in b {
        let _ = sistema::pokazat(handle(b), *id == s.aktivnaya && !s.vkladki.iter().any(|v|v.id==*id&&v.memory_pending));
    }
}

#[cfg(target_os = "windows")]
fn sync_native_visibility_for(
    visibility: &mut crate::cef_visibility_win::Visibility,
    k: &Konteyner, s: &Sessiya, browsers: &HashMap<u64, Browser>, allowed: bool,
) {
    let windows: Vec<_> = browsers.iter().map(|(id, browser)| (
        browser.identifier(), handle(browser) as u64, allowed && *id == s.aktivnaya && !s.vkladki.iter().any(|v|v.id==*id&&v.memory_pending),
    )).collect();
    visibility.sync(k.okno as u64, &windows);
}

pub fn rabotat(
    roditel: u64,
    mesto: Mesto,
    profil: &std::path::Path,
    start: Option<&str>,
    bootstrap: crate::tab_routes::RouteBootstrap,
) -> anyhow::Result<()> {
    let routes=std::rc::Rc::new(std::cell::RefCell::new(crate::cef_route_context::Registry::new(profil,bootstrap,crate::privatnost::process_privatnyj())?));
    let k = Konteyner::sozdat(roditel, mesto,routes)?;
    let initially_locked=std::env::args().any(|arg|arg=="--nb-browser-locked");
    let initial_lock_generation=std::env::args().find_map(|arg|arg.strip_prefix("--nb-lock-generation=").and_then(|value|value.parse::<u64>().ok())).unwrap_or(0);
    let mut s = start
        .map(Sessiya::novaya)
        .unwrap_or_else(|| Sessiya::zagruzit(profil));
    if crate::privatnost::process_privatnyj(){for v in &mut s.vkladki{v.privatnaya=true;}}
    for v in &s.vkladki{k.choices.borrow_mut().insert(v.id,v.route.clone());}
    let (tx, rx) = channel();
    #[cfg(target_os = "windows")]
    let (rx, event_relay) = crate::cef_events::relay(&tx, rx, Sobytie::Komanda(Komanda::ObnovitInterfeys), request_wake)?;
    let restored_scroll=if std::env::args().any(|a|a=="--nb-restore-escrow"){
        cef_super_hibernation::restore_startup(&k,&mut s,&tx,&rx)?
    }else{HashMap::new()};
    for v in &s.vkladki{k.choices.borrow_mut().insert(v.id,v.route.clone());}
    let mut brauzery = HashMap::new();
    if initially_locked{for tab in &mut s.vkladki{tab.otlozhena=true;}let _=sistema::pokazat(k.okno,false);}
    for v in &s.vkladki {
        if !initially_locked && !v.otlozhena {
            if let Some(browser)=sozdat_brauzer(&k, v.id, &v.adres, &tx, v.privatnaya)?{brauzery.insert(v.id,browser);}
        }
    }
    pokazat(&s, &brauzery);
    let tx_stdin = tx.clone();
    std::thread::spawn(move || {
        for l in std::io::stdin().lock().lines() {
            let Ok(l) = l else { break };
            let l=zeroize::Zeroizing::new(l);
            if let Ok(komanda) = serde_json::from_str::<Komanda>(&l) {
                if tx_stdin.send(Sobytie::Komanda(komanda)).is_err() {
                    break;
                }
            }
        }
    });
    eprintln!(
        "cef: контейнер {}, браузеров {}, активная {}, размер {:?}, runtime Alloy",
        k.okno,
        brauzery.len(),
        s.aktivnaya,
        k.razmer()
    );
    println!("NB-OKNO {}", k.okno);
    println!("NB-VKLADKI {}", serde_json::to_string(&s)?);
    std::io::stdout().flush()?;
    let razmer = k.razmer();
    let rabota = Rabota {
        k,
        s,
        tx,
        rx,
        #[cfg(target_os = "windows")]
        _event_relay: event_relay,
        #[cfg(target_os = "windows")]
        native_visibility: Default::default(),
        brauzery,
        deferred_actions:Default::default(),pending_downloads:Default::default(),data_requests:Default::default(),route_attempts:Default::default(),sleep_requests:Default::default(),memory_registrations:Default::default(),memory_last_used:Default::default(),memory_next_scan:std::time::Instant::now(),memory_enabled:crate::nastrojki::zagruzit().memory_saver,memory_minutes:crate::nastrojki::zagruzit().memory_saver_minutes,memory_scroll:restored_scroll,hibernate:Default::default(),
        razmer,
        devtools: HashMap::new(), devtools_pending:HashMap::new(), devtools_procent:40,devtools_pixels:None, ui_last:String::new(), perevody:HashMap::new(),perevod_avto:HashMap::new(),perevod_due:HashMap::new(),perevod_ui:HashMap::new(),
        label:String::new(), extras:HashMap::new(), closed_labels:Default::default(), closing:false, browser_locked:initially_locked, browser_lock_generation:initial_lock_generation, force_private:crate::privatnost::process_privatnyj(),
    };
    #[cfg(target_os = "windows")]
    {
        // CEF keeps its native message loop. Our commands wake it on demand;
        // infrequent maintenance only watches native resize and timed actions.
        let rabota = std::rc::Rc::new(std::cell::RefCell::new(rabota));
        UI_WORK.with(|slot| *slot.borrow_mut() = std::rc::Rc::downgrade(&rabota));
        UI_RUNNING.store(true, std::sync::atomic::Ordering::Release);
        if post_delayed_task(ThreadId::UI, Some(&mut Obrabotat::new(true)), 0) != 1 {
            UI_RUNNING.store(false, std::sync::atomic::Ordering::Release);
            UI_WORK.with(|slot| *slot.borrow_mut() = Default::default());
            anyhow::bail!("не удалось запланировать обработку команд CEF");
        }
        run_message_loop();
        UI_RUNNING.store(false, std::sync::atomic::Ordering::Release);
        UI_WORK.with(|slot| *slot.borrow_mut() = Default::default());
        // Dropping Rabota stops every relay, including additional windows.
        drop(rabota);
        Ok(())
    }
    #[cfg(not(target_os = "windows"))]
    {
        let mut rabota = rabota;
        loop {
            do_message_loop_work();
            rabota.shag()?;
            if rabota.hibernate.stopped(){break;}
            std::thread::sleep(std::time::Duration::from_millis(8));
        }
        drop(rabota);
        Ok(())
    }
}

// These primitives contain no CEF object or UI-thread Rc. Relay threads only
// forward owned events and post a task; every Browser operation stays on UI.
#[cfg(target_os = "windows")]
static UI_RUNNING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
#[cfg(target_os = "windows")]
static UI_WAKE_PENDING: crate::cef_events::WakeGate = crate::cef_events::WakeGate::new();
#[cfg(target_os = "windows")]
thread_local! {
    static UI_WORK: std::cell::RefCell<std::rc::Weak<std::cell::RefCell<Rabota>>> = Default::default();
}

#[cfg(target_os = "windows")]
pub(crate) fn request_wake() {
    use std::sync::atomic::Ordering;
    if !UI_RUNNING.load(Ordering::Acquire) || !UI_WAKE_PENDING.claim() {
        return;
    }
    if post_task(ThreadId::UI, Some(&mut Obrabotat::new(false))) != 1 {
        // No permanent pending bit on shutdown/post failure. Maintenance is
        // also a bounded fallback if CEF rejects a concurrent wake.
        UI_WAKE_PENDING.release();
    }
}

#[cfg(target_os = "windows")]
wrap_task! {
    struct Obrabotat { maintenance: bool }
    impl Task {
        fn execute(&self) {
            use std::sync::atomic::Ordering;
            if !self.maintenance {
                // Clear BEFORE draining: arrivals during a tick post the next
                // task even if the current drain has just passed their lane.
                UI_WAKE_PENDING.release();
            }
            if !UI_RUNNING.load(Ordering::Acquire) { return; }
            let next = UI_WORK.with(|slot| {
                let Some(work) = slot.borrow().upgrade() else { return Some(100) };
                // CEF may dispatch nested work during Browser creation. Do not
                // panic or borrow Rabota recursively; maintenance retries it.
                let Ok(mut work) = work.try_borrow_mut() else { return Some(100) };
                if let Err(e) = work.shag() {
                    eprintln!("cef: обработка команд: {e}");
                    quit_message_loop();
                    return None;
                }
                if work.hibernate.stopped(){quit_message_loop();return None;}
                Some(if work.has_live_devtools() { 16 } else { 100 })
            });
            if self.maintenance {
                if let Some(delay) = next {
                    if post_delayed_task(ThreadId::UI, Some(&mut Obrabotat::new(true)), delay) != 1 {
                        eprintln!("cef: не удалось продолжить обслуживание окон");
                        quit_message_loop();
                    }
                }
            }
        }
    }
}

#[derive(Clone,Copy,PartialEq,Eq)]
enum MemoryStage { Preflight,Freezing,Verify,Closing }
struct MemoryAttempt {
    identity:i32,token:String,request:String,stage:MemoryStage,automatic:bool,
    started:std::time::Instant,wake:bool,timed_out:bool,scroll:(f64,f64),
}
fn memory_refused(id:u64,error:&str,s:&mut Sessiya,tx:&Sender<Sobytie>,requests:&mut HashMap<u64,MemoryAttempt>,registrations:&mut HashMap<u64,Registration>,browsers:&HashMap<u64,Browser>){
    if let Some(attempt)=requests.remove(&id){
        if attempt.stage!=MemoryStage::Preflight{if let Some(b)=browsers.get(&id).filter(|b|b.identifier()==attempt.identity){crate::memory_cef::resume(b);}}
        SLEEP_CLOSES.with(|m|{m.borrow_mut().remove(&attempt.identity);});
        if let Some(tab)=s.vkladki.iter_mut().find(|v|v.id==id){tab.memory_pending=false;}
        eprintln!("NEXUSBRO_MEMORY tab={id} browser={} automatic={} refused={error}",attempt.identity,attempt.automatic);
        let _=tx.send(Sobytie::Sluzhebnoe(serde_json::json!({"tip":"memory-result","id":id,"ok":false,"automatic":attempt.automatic,"canceled":error=="Выгрузка отменена: вкладка закрывается","error":error})));
    }
    registrations.remove(&id);
}
fn memory_blocked(id:u64,automatic:bool,s:&Sessiya,browsers:&HashMap<u64,Browser>,pending_tools:&HashMap<u64,std::time::Instant>,force_private:bool)->Option<&'static str>{
    memory_blocked_scoped(id,automatic,s,browsers,pending_tools,force_private,false)
}
fn memory_blocked_scoped(id:u64,automatic:bool,s:&Sessiya,browsers:&HashMap<u64,Browser>,pending_tools:&HashMap<u64,std::time::Instant>,force_private:bool,allow_selected:bool)->Option<&'static str>{
    let Some(tab)=s.vkladki.iter().find(|v|v.id==id)else{return Some("Вкладка уже закрыта");};
    let Some(browser)=browsers.get(&id)else{return Some("Вкладка уже выгружена");};
    if allow_selected&&browser.main_frame().map(|f|CefString::from(&f.url()).to_string()).as_deref()!=Some(tab.adres.as_str()){return Some("Адрес документа ещё не подтверждён");}
    if tab.route_pending{return Some("Вкладка меняет сетевой путь");}
    if s.aktivnaya==id&&!allow_selected{return Some("Активную вкладку нельзя выгрузить");}
    if tab.privatnaya||force_private{return Some("Приватная вкладка остаётся в памяти");}
    if tab.zakreplena{return Some("Сначала открепите вкладку");}
    // Legacy per-tab keep flags no longer override the automatic browser policy.
    if tab.zagruzka||browser.is_loading()!=0{return Some("Дождитесь загрузки страницы");}
    if browser.frame_count()!=1{return Some("Вкладка содержит вложенные документы и остаётся в памяти");}
    // The navigation controller is authoritative even if a late document
    // notification raced a newer request-method notification.
    match browser.host().and_then(|h|h.visible_navigation_entry()) {
        Some(entry) if entry.has_post_data()!=0=>return Some("Страница получена отправкой формы и не выгружается"),
        None=>return Some("История текущего документа ещё неизвестна"),
        _=>{}
    }
    if pending_tools.contains_key(&id)||browser.host().is_some_and(|h|h.has_dev_tools()!=0){return Some("Сначала закройте инструменты разработчика");}
    if crate::zagruzki::tab_busy(id){return Some("Вкладка выполняет загрузку файла");}
    if automatic&&(browser.can_go_back()!=0||browser.can_go_forward()!=0){return Some("Сохраняем внутреннюю историю переходов вкладки");}
    crate::memory_guard::reason(browser.identifier())
}

struct RouteAttempt {
    route:crate::tab_routes::RouteChoice,
    prepared:Option<crate::tab_routes::NativeRoute>,
    generation:u64, expected:i32, closing:bool, closed:bool,
    since:std::time::Instant,
    failure:Option<String>,
}
fn route_guard_code(reason:&str)->&'static str{
    match reason{
        ""=>"clear",
        "Состояние документа ещё неизвестно"=>"document_unknown",
        "Страница получена отправкой формы и не выгружается"=>"non_get",
        "Во вкладке был ввод; перезагрузите страницу после сохранения данных"=>"edited",
        "Вкладка использует камеру, микрофон или захват экрана"=>"capture",
        "Страница удерживает несохранённые данные"=>"before_unload",
        "Дождитесь загрузки страницы"=>"loading",
        "Вкладка занята; завершите текущую операцию"=>"busy",
        "Браузер заблокирован"=>"locked",
        "Вкладка уже изменилась"|"Вкладка уже изменилась; выберите путь повторно"|"Вкладка закрыта"|"Смена пути отменена: вкладка закрывается"=>"stale",
        "Истекло время подготовки маршрута"=>"timeout",
        "Безопасный профиль маршрута недоступен"|"Недопустимый маршрут"=>"context",
        "Не удалось создать страницу на выбранном пути"|"Страница уже недоступна"|"Не удалось открыть новый путь и восстановить прежнюю страницу; выберите путь повторно"=>"unavailable",
        _=>"protected_or_unavailable",
    }
}
fn route_error_code(error:&str)->&'static str{
    match route_guard_code(error){
        "capture"=>"capture", "busy"=>"busy", "before_unload"=>"before_unload",
        "locked"=>"locked", "stale"=>"stale", "timeout"=>"timeout",
        "context"|"unavailable"=>"profile_unavailable",
        _=>"unknown",
    }
}
fn route_result(tx:&Sender<Sobytie>,id:u64,generation:u64,browser_id:i32,error:Option<&str>){
    let _=tx.send(Sobytie::Sluzhebnoe(serde_json::json!({"tip":"route-result","id":id,"generation":generation,"browser_id":browser_id,"ok":error.is_none(),"error":error,"error_code":error.map(route_error_code)})));
}
/// A lock cancels preparation, but cannot undo an already committed close.
fn cancel_routes_for_lock(s:&mut Sessiya,attempts:&mut HashMap<u64,RouteAttempt>,_browsers:&HashMap<u64,Browser>,tx:&Sender<Sobytie>)->bool{
    let ids:Vec<_>=attempts.iter().filter(|(_,a)|!a.closing).map(|(id,_)|*id).collect();
    let changed=!ids.is_empty();
    for id in ids{if let Some(attempt)=attempts.remove(&id){
        if let Some(tab)=s.vkladki.iter_mut().find(|t|t.id==id){tab.route_pending=false;tab.route_error=Some("Смена пути отменена: браузер заблокирован".into());}
        route_result(tx,id,attempt.generation,attempt.expected,Some("Браузер заблокирован"));
    }}changed
}
struct Rabota {
    k: Konteyner,
    s: Sessiya,
    tx: Sender<Sobytie>,
    rx: Receiver<Sobytie>,
    #[cfg(target_os = "windows")]
    _event_relay: crate::cef_events::Relay<Sobytie>,
    #[cfg(target_os = "windows")]
    native_visibility: crate::cef_visibility_win::Visibility,
    brauzery: HashMap<u64, Browser>,
    sleep_requests: HashMap<u64,MemoryAttempt>,
    route_attempts:HashMap<u64,RouteAttempt>,
    data_requests:Vec<(std::time::Instant,Komanda)>,
    deferred_actions:HashMap<u64,Vec<Komanda>>,pending_downloads:HashMap<u64,Vec<String>>,
    memory_registrations:HashMap<u64,Registration>,
    memory_last_used:HashMap<u64,std::time::Instant>,
    memory_next_scan:std::time::Instant,
    memory_enabled:bool,
    memory_minutes:u32,
    memory_scroll:HashMap<u64,(f64,f64)>,
    hibernate:cef_super_hibernation::Engine,
    razmer: Option<(u32, u32)>,
    devtools: HashMap<u64,u32>,
    devtools_pending:HashMap<u64,std::time::Instant>,
    devtools_procent: u32,
    devtools_pixels:Option<u32>,
    ui_last: String,
    perevody: HashMap<u64,(String,Registration)>,
    perevod_avto:HashMap<u64,String>,
    perevod_due:HashMap<u64,std::time::Instant>,
    perevod_ui:HashMap<u64,serde_json::Value>,
    label:String,
    extras:HashMap<String,Rabota>,
    closed_labels:std::collections::HashSet<String>,
    closing:bool,
    browser_locked:bool,
    browser_lock_generation:u64,
    force_private:bool,
}

impl Rabota {
    fn hibernate_safe(&self)->bool{
        self.label.is_empty()&&self.extras.is_empty()&&!self.force_private&&!self.browser_locked&&!self.closing
        &&self.route_attempts.is_empty()&&self.sleep_requests.is_empty()&&self.pending_downloads.is_empty()&&self.devtools.is_empty()&&self.devtools_pending.is_empty()&&self.perevody.is_empty()&&self.data_requests.is_empty()&&self.deferred_actions.is_empty()
        &&self.s.vkladki.iter().all(|v|!v.privatnaya&&!v.route_pending&&v.route_error.is_none())
        &&self.brauzery.keys().all(|id|memory_blocked_scoped(*id,true,&self.s,&self.brauzery,&self.devtools_pending,self.force_private,true).is_none())
    }
    fn hibernate_shag(&mut self)->anyhow::Result<()>{
        if self.hibernate.stopped(){return Ok(());}
        let events=self.rx.try_iter().take(128).collect::<Vec<_>>();
        for event in events{
            if let Sobytie::Komanda(Komanda::BrowserLocked{locked,generation})=&event{if *generation>=self.browser_lock_generation{self.browser_locked=*locked;self.browser_lock_generation=*generation;}else{continue;}}
            if matches!(&event,Sobytie::Komanda(Komanda::GlavnoePokazat|Komanda::OknoNovoe{..})){self.hibernate.invalidate(&self.brauzery);}
            if let Sobytie::Komanda(command)=&event{let safe=self.hibernate_safe();self.hibernate.command(command,&self.k,&self.brauzery,safe)?;}
            if let Sobytie::Zakryta(_,Some(identity))=&event{self.k.routes.borrow_mut().forget_browser(*identity);}
            let safe=self.hibernate_safe();self.hibernate.event(&event,&self.k,&self.s,&mut self.brauzery,safe);
        }
        let safe=self.hibernate_safe();self.hibernate.tick(&self.s,&self.brauzery,safe)
    }
    fn route_publish(&self)->anyhow::Result<()>{
        if self.label.is_empty(){println!("NB-VKLADKI {}",serde_json::to_string(&self.s)?);}else{println!("NB-UI {}",serde_json::json!({"tip":"okno-session","label":self.label,"sessiya":self.s}));}
        std::io::stdout().flush()?;Ok(())
    }
    fn route_tick(&mut self)->anyhow::Result<()>{
        if self.browser_locked||self.closing{return Ok(());}
        let mut changed=false;
        let pending:Vec<_>=self.route_attempts.keys().copied().collect();
        for id in pending{
            let Some(mut attempt)=self.route_attempts.remove(&id)else{continue;};
            let Some(tab)=self.s.vkladki.iter().find(|v|v.id==id).cloned()else{continue;};
            let actual=self.brauzery.get(&id).map(|b|b.identifier()).unwrap_or(0);
            let mut failure=attempt.failure.take();
            if !attempt.closed&&actual!=attempt.expected{failure=Some("Вкладка уже изменилась".to_string());}
            if !attempt.closing&&attempt.since.elapsed().as_secs()>45{failure=Some("Истекло время подготовки маршрута".to_string());}
            if failure.is_none(){
                if let Some(prepared)=attempt.prepared.as_ref(){
                    let readiness={self.k.routes.borrow_mut().prepare(prepared,tab.privatnaya||self.force_private)};
                    match readiness{
                        Ok(Some(_))=>{
                            if actual!=0&&self.k.routes.borrow().key_for_browser(actual).as_deref()==Some(prepared.key.as_str()){
                                // Alias change to exactly the same effective route: no reload.
                                self.k.choices.borrow_mut().insert(id,attempt.route.clone());
                                self.k.routes.borrow_mut().bind(attempt.route.clone(),prepared.clone());
                                if let Some(tab)=self.s.vkladki.iter_mut().find(|v|v.id==id){tab.route=attempt.route.clone();tab.route_pending=false;tab.route_error=None;}
                                route_result(&self.tx,id,attempt.generation,attempt.expected,None);changed=true;continue;
                            }
                            if !attempt.closed&&!attempt.closing{
                                if self.sleep_requests.contains_key(&id)||self.pending_downloads.contains_key(&id)||self.devtools_pending.contains_key(&id)||self.devtools.contains_key(&id)||crate::zagruzki::tab_busy(id){failure=Some("Вкладка занята; завершите текущую операцию".into());}
                                else if let Some(reason)=crate::memory_guard::route_reason(actual){failure=Some(reason.into());}
                                else if let Some(browser)=self.brauzery.get(&id){
                                    if let Some(host)=browser.host(){
                                        // This was explicitly requested by the user: reload into
                                        // another isolated profile, not a background eviction.
                                        // Do not demand an untouched page or run the automatic
                                        // memory purity probe. The real beforeunload callback
                                        // below still cancels a page that refuses graceful close.
                                        ROUTE_CLOSES.with(|m|{m.borrow_mut().insert(actual);});
                                        attempt.closing=true;host.close_browser(0);
                                    }else{failure=Some("Страница уже недоступна".into());}
                                }
                            }
                            if attempt.closed{
                                match sozdat_brauzer_routed(&self.k,id,&tab.adres,&self.tx,tab.privatnaya||self.force_private,prepared){
                                    Ok(Some(browser))=>{
                                        let identity=browser.identifier();self.brauzery.insert(id,browser);
                                        self.k.choices.borrow_mut().insert(id,attempt.route.clone());self.k.routes.borrow_mut().bind(attempt.route.clone(),prepared.clone());
                                        if let Some(tab)=self.s.vkladki.iter_mut().find(|v|v.id==id){tab.route=attempt.route.clone();tab.route_pending=false;tab.route_error=None;tab.browser_id=identity;tab.otlozhena=false;tab.zagruzka=tab.adres!="about:blank";tab.nazad=false;tab.vpered=false;}
                                        route_result(&self.tx,id,attempt.generation,attempt.expected,None);changed=true;continue;
                                    }
                                    Ok(None)=>{},Err(_)=>{self.k.choices.borrow_mut().insert(id,tab.route.clone());failure=Some("Не удалось создать страницу на выбранном пути".into());}
                                }
                            }
                        }
                        Ok(None)=>{},Err(_)=>failure=Some("Безопасный профиль маршрута недоступен".into()),
                    }
                }
            }
            if let Some(mut error)=failure{
                eprintln!("NEXUSBRO_ROUTE_RESULT browser={} attempt={} ok=false guard={}",attempt.expected,attempt.generation,route_guard_code(&error));
                if attempt.closed&&attempt.expected!=0&&!self.brauzery.contains_key(&id){
                    match sozdat_brauzer(&self.k,id,&tab.adres,&self.tx,tab.privatnaya||self.force_private){
                        Ok(Some(browser))=>{let identity=browser.identifier();self.brauzery.insert(id,browser);if let Some(tab)=self.s.vkladki.iter_mut().find(|v|v.id==id){tab.browser_id=identity;tab.otlozhena=false;}},
                        _=>{error="Не удалось открыть новый путь и восстановить прежнюю страницу; выберите путь повторно".into();}
                    }
                }
                ROUTE_CLOSES.with(|m|{m.borrow_mut().remove(&attempt.expected);});
                if let Some(tab)=self.s.vkladki.iter_mut().find(|v|v.id==id){tab.route_pending=false;tab.route_error=Some(error.clone());}
                route_result(&self.tx,id,attempt.generation,attempt.expected,Some(&error));changed=true;
            }else{self.route_attempts.insert(id,attempt);}
        }
        let wanted:Vec<_>=self.s.vkladki.iter().filter(|t|!t.otlozhena&&!t.route_pending&&t.route_error.is_none()&&!self.brauzery.contains_key(&t.id)&&!self.route_attempts.contains_key(&t.id)).cloned().collect();
        for tab in wanted{
            self.k.choices.borrow_mut().insert(tab.id,tab.route.clone());
            let descriptor=self.k.routes.borrow().descriptor(&tab.route);
            if descriptor.is_none(){
                let Some(generation)=tab.route_generation.checked_add(1)else{continue;};
                if let Some(row)=self.s.vkladki.iter_mut().find(|v|v.id==tab.id){row.route_generation=generation;row.route_pending=true;}
                self.route_attempts.insert(tab.id,RouteAttempt{route:tab.route.clone(),prepared:None,generation,expected:0,closing:false,closed:true,since:std::time::Instant::now(),failure:None});
                let _=self.tx.send(Sobytie::Sluzhebnoe(serde_json::json!({"tip":"route-needed","id":tab.id,"generation":generation,"route":tab.route,"browser_id":0})));changed=true;continue;
            }
            match self.k.routes.borrow_mut().prepare(&descriptor.unwrap(),tab.privatnaya||self.force_private){
                Ok(Some(_))=>{},Ok(None)=>continue,Err(_)=>{
                    if let Some(row)=self.s.vkladki.iter_mut().find(|v|v.id==tab.id){row.route_error=Some("Безопасный профиль маршрута недоступен".into());}changed=true;continue;
                }
            }
            match sozdat_brauzer(&self.k,tab.id,&tab.adres,&self.tx,tab.privatnaya){
                Ok(Some(browser))=>{let identity=browser.identifier();self.brauzery.insert(tab.id,browser);if let Some(row)=self.s.vkladki.iter_mut().find(|v|v.id==tab.id){row.browser_id=identity;row.zagruzka=row.adres!="about:blank";}changed=true;},
                Ok(None)=>{},Err(_)=>{if let Some(row)=self.s.vkladki.iter_mut().find(|v|v.id==tab.id){row.route_error=Some("Не удалось открыть страницу".into());}changed=true;}
            }
        }
        let ready:Vec<_>=self.deferred_actions.keys().filter(|id|self.brauzery.contains_key(id)).copied().collect();
        for id in ready{if let Some(commands)=self.deferred_actions.remove(&id){for command in commands{let _=self.tx.send(Sobytie::Komanda(command));}}}
        let ready:Vec<_>=self.pending_downloads.keys().filter(|id|self.brauzery.contains_key(id)).copied().collect();
        for id in ready{if let Some(downloads)=self.pending_downloads.remove(&id){if let Some(host)=self.brauzery.get(&id).and_then(|b|b.host()){for url in downloads{host.start_download(Some(&CefString::from(url.as_str())));}}}}
        self.deferred_actions.retain(|id,_|self.s.vkladki.iter().any(|t|t.id==*id));self.pending_downloads.retain(|id,_|self.s.vkladki.iter().any(|t|t.id==*id));
        if changed{pokazat(&self.s,&self.brauzery);self.route_publish()?;}Ok(())
    }
    #[cfg(target_os = "windows")]
    fn sync_native_visibility(&mut self) {
        sync_native_visibility_for(&mut self.native_visibility, &self.k, &self.s,
            &self.brauzery, !self.browser_locked && !self.closing);
    }
    #[cfg(target_os = "windows")]
    fn has_live_devtools(&self) -> bool {
        self.devtools.contains_key(&self.s.aktivnaya) || self.extras.values().any(Self::has_live_devtools)
    }
    fn set_browser_locked(&mut self, locked:bool, generation:u64){
        if generation<self.browser_lock_generation{return;}
        self.browser_lock_generation=generation;
        self.browser_locked=locked;
        for extra in self.extras.values_mut(){extra.set_browser_locked(locked,generation);}
        if locked{
            if cancel_routes_for_lock(&mut self.s,&mut self.route_attempts,&self.brauzery,&self.tx){let _=self.route_publish();}
            for browser in self.brauzery.values(){crate::cef_youtube::stop(browser);crate::cef_interfeys::cancel_context(browser.identifier());}
            for browser in self.brauzery.values(){
                if let Some(host)=browser.host(){host.set_focus(0);host.close_dev_tools();}
                let _=sistema::pokazat(handle(browser),false);
            }
            for hwnd in self.devtools.values(){let _=sistema::pokazat(*hwnd,false);}
            self.devtools_pending.clear();self.devtools.clear();
            let _=sistema::pokazat(self.k.okno,false);
        }
        #[cfg(target_os = "windows")]
        self.sync_native_visibility();
    }
    fn shag(&mut self) -> anyhow::Result<()> {
        if self.hibernate.active()||self.hibernate.stopped(){return self.hibernate_shag();}
        self.route_tick()?;
        if !self.data_requests.is_empty(){
            let contexts=self.k.routes.borrow_mut().all_owned();
            if !matches!(&contexts,Ok(None))||self.data_requests[0].0.elapsed().as_secs()>10{
                for (_,command) in self.data_requests.drain(..){match command{
                    Komanda::DannyeStatistika{token}=>match &contexts{
                        Ok(Some(contexts))=>crate::dannye_statistika::izmerit(contexts.clone(),token,self.label.clone(),self.tx.clone()),
                        _=>{let _=self.tx.send(Sobytie::Sluzhebnoe(serde_json::json!({"tip":"dannye-obem","token":token,"cookies":null,"cache_bytes":null,"cache_memory":false})));}
                    },
                    Komanda::OchistitDannye{token,cookies,cache}=>match &contexts{
                        Ok(Some(contexts))=>crate::dannye_saytov::ochistit(contexts.clone(),token,self.label.clone(),cookies,cache,self.tx.clone()),
                        _=>{let _=self.tx.send(Sobytie::Sluzhebnoe(serde_json::json!({"tip":"ochistka-zavershena","token":token,"error":"Не все профили маршрутов доступны; очистка не подтверждена"})));}
                    },_=>{}
                }}
            }
        }
        crate::cef_interfeys::prune_contexts();
        for extra in self.extras.values_mut(){
            if let Err(e)=extra.shag(){
                // Ошибка одного дополнительного окна не останавливает Super.
                eprintln!("cef: дополнительное окно: {e}");extra.closing=true;
                for b in extra.brauzery.values(){if let Some(h)=b.host(){h.close_browser(1);}}
            }
        }
        let closed:Vec<_>=self.extras.iter().filter(|(_,e)|e.closing&&e.brauzery.is_empty()).map(|(label,_)|label.clone()).collect();
        for label in closed{self.extras.remove(&label);println!("NB-UI {}",serde_json::json!({"tip":"okno-closed","label":label}));std::io::stdout().flush()?;}

        if self.label.is_empty() && !self.s.vkladki.iter().chain(self.extras.values().flat_map(|e|e.s.vkladki.iter())).any(|v|v.privatnaya)
            && self.k.routes.borrow().has_private(){
            crate::zagruzki::zakryt_privatnye();self.k.routes.borrow_mut().clear_private();PRIVATE_IDS.with(|ids|ids.borrow_mut().clear());
            let _=self.tx.send(Sobytie::Sluzhebnoe(serde_json::json!({"tip":"private-ended"})));
        }
        let Self {
            k,
            s,
            tx,
            rx,
            brauzery,
            deferred_actions,pending_downloads,data_requests,route_attempts,sleep_requests,memory_registrations,memory_last_used,memory_next_scan,memory_enabled,memory_minutes,memory_scroll,hibernate,
            razmer,
            devtools, devtools_pending, devtools_procent, devtools_pixels, ui_last, perevody, perevod_avto, perevod_due, perevod_ui, label, extras, closed_labels, closing, browser_locked, browser_lock_generation, force_private,
            #[cfg(target_os = "windows")]
            native_visibility,
            ..
        } = self;
        let mut izmeneno = false;
        let mut layout_dirty=false;
        let mut fokus = false;
        let mut fokus_klaviatury = false;
        let mut fokus_posle_devtools=false;
        let mut handled_events = 0;
        for sobytie in rx.try_iter().take(128) {
            handled_events += 1;
            if hibernate.active(){
                if let Sobytie::Komanda(Komanda::BrowserLocked{locked,generation})=&sobytie{if *generation>=*browser_lock_generation{*browser_locked=*locked;*browser_lock_generation=*generation;}else{continue;}}
                if matches!(&sobytie,Sobytie::Komanda(Komanda::GlavnoePokazat|Komanda::OknoNovoe{..})){hibernate.invalidate(brauzery);}
                if let Sobytie::Komanda(command)=&sobytie{let safe=!*browser_locked&&!*closing&&extras.is_empty()&&brauzery.keys().all(|id|memory_blocked_scoped(*id,true,s,brauzery,devtools_pending,*force_private,true).is_none());hibernate.command(command,k,brauzery,safe)?;}
                let safe=!*browser_locked&&!*closing&&extras.is_empty();
                if hibernate.event(&sobytie,k,s,brauzery,safe){continue;}
                // Normal address/loading/UI updates are not committed after freeze.
                continue;
            }
            // A lock covers queued/in-flight UI actions too. Lifecycle events
            // still complete, while no key, popup or stale command can reopen a page.
            if *browser_locked && matches!(&sobytie,Sobytie::Komanda(command) if !matches!(command,
                Komanda::BrowserLocked{..}|Komanda::RouteApply{..}|Komanda::RouteFailed{..}|Komanda::PasswordsEnabled{..}|Komanda::MemorySettings{..}|Komanda::OknoKomanda{..}|
                Komanda::OknoZakryt{..}|Komanda::GlavnoeSkryto|Komanda::GlavnoePokazat)) {continue;}

            // No input or navigation reaches a WebContents after close was
            // committed. Selecting it records intent; the replacement waits.
            let target=match &sobytie {
                Sobytie::Komanda(Komanda::Vybrat{id}|Komanda::Pereyti{id,..}|Komanda::Obnovit{id}|Komanda::ObnovitBezKesha{id}|Komanda::Nazad{id}|Komanda::Vpered{id}|Komanda::Instrumenty{id}|Komanda::Issledovat{id,..}|Komanda::Nayti{id,..}|Komanda::Perevesti{id}|Komanda::PasswordForm{id,..})=>Some(*id),
                _=>None,
            };
            if let Some(id)=target {
                if route_attempts.contains_key(&id){continue;}

                if let Some(attempt)=sleep_requests.get_mut(&id){
                    if attempt.stage==MemoryStage::Closing {
                        if matches!(&sobytie,Sobytie::Komanda(Komanda::Vybrat{..})){attempt.wake=true;s.aktivnaya=id;fokus=true;izmeneno=true;}
                        else{let _=tx.send(Sobytie::Sluzhebnoe(serde_json::json!({"tip":"memory-result","id":id,"ok":false,"error":"Вкладка завершает выгрузку; дождитесь её восстановления"})));}
                        continue;
                    }
                    memory_refused(id,"Вкладка снова используется",s,tx,sleep_requests,memory_registrations,brauzery);fokus=true;
                }
                memory_last_used.insert(id,std::time::Instant::now());
            }
            let budit = match &sobytie {
                Sobytie::Komanda(
                    Komanda::Pereyti { id, .. }
                    | Komanda::Obnovit { id }
                    | Komanda::ObnovitBezKesha { id }
                    | Komanda::Nayti { id, .. }
                    | Komanda::Perevesti { id }
                    | Komanda::Instrumenty { id }
                    | Komanda::Issledovat { id, .. },
                ) => Some(*id),
                _ => None,
            };
            let mut razbuzhen = None;
            if let Some(id) = budit {
                if !brauzery.contains_key(&id) {
                    if let Some(v) = s.vkladki.iter_mut().find(|v| v.id == id) {
                        if let Sobytie::Komanda(Komanda::Pereyti { adres, .. }) = &sobytie {
                            v.adres = adres.clone();
                        }
                        if let Some(browser)=sozdat_brauzer(k, id, &v.adres, tx, v.privatnaya)?{brauzery.insert(id,browser);}
                        v.otlozhena = false;v.zagruzka=v.adres!="about:blank";
                        razbuzhen = Some(id);
                    }
                }
            }
            if let Some(id)=budit{
                if !brauzery.contains_key(&id){
                    if let Sobytie::Komanda(command)=&sobytie{let queue=deferred_actions.entry(id).or_default();if queue.len()<4{queue.push(command.clone());}}
                    if let Some(v)=s.vkladki.iter_mut().find(|v|v.id==id){v.otlozhena=false;}
                    izmeneno=true;continue;
                }
            }
            // Поиск, перевод и отчёты защиты не меняют сохранённую сессию.
            // Пробуждение отложенной страницы при этом обязательно публикуем.
            let session_dirty = razbuzhen.is_some() || !matches!(&sobytie,
                Sobytie::Sluzhebnoe(_) | Sobytie::NavigationFailed(..) | Sobytie::PoiskVybratPervoe(..) | Sobytie::DevtoolsGotovy(..) |
                Sobytie::DevtoolsZakryty(_) | Sobytie::AdresnyiVvod |
                Sobytie::Komanda(Komanda::Nayti{..} | Komanda::PoiskZakryt{..} |
                    Komanda::Perevesti{..} | Komanda::PerevodOriginal{..} |
                    Komanda::PerevodOtvet{..} | Komanda::PerevodAvto{..} | Komanda::FonKonteynera{..} | Komanda::OchistitDannye{..} | Komanda::DannyeStatistika{..} | Komanda::PasswordForm{..} | Komanda::PasswordsEnabled{..} | Komanda::BrowserLocked{..} | Komanda::PageContextChoose{..} | Komanda::Instrumenty{..} |
                    Komanda::Issledovat{..} | Komanda::InstrumentyRazmer{..} |
                    Komanda::ZagruzkaUpravlenie{..} | Komanda::Masshtab{..}));
            let private_command=matches!(&sobytie,Sobytie::Komanda(Komanda::NovayaPrivatnaya{..}));
            let memory_automatic=matches!(&sobytie,Sobytie::Komanda(Komanda::MemoryAuto{..}));
            match sobytie {
                Sobytie::Komanda(Komanda::HibernatePrepare{request})=>{
                    // Fixed diagnostic vocabulary only; never forward native
                    // messages, addresses or document-provided error strings.
                    let blocked=s.vkladki.iter().filter(|tab|brauzery.contains_key(&tab.id))
                        .find_map(|tab|memory_blocked_scoped(tab.id,true,s,brauzery,devtools_pending,*force_private,true));
                    if let Some(reason)=blocked{
                        let reason=match reason{
                            "Сохраняем внутреннюю историю переходов вкладки"=>"native_history",
                            "Дождитесь загрузки страницы"=>"native_loading",
                            "Приватная вкладка остаётся в памяти"=>"native_private",
                            "Сначала открепите вкладку"=>"native_pinned",
                            "Вкладка содержит вложенные документы и остаётся в памяти"=>"native_frames",
                            "Страница получена отправкой формы и не выгружается"=>"native_post",
                            "Сначала закройте инструменты разработчика"=>"native_devtools",
                            "Вкладка выполняет загрузку файла"=>"native_download",
                            "Вкладка использует камеру, микрофон или захват экрана"=>"native_capture",
                            "Во вкладке был ввод; перезагрузите страницу после сохранения данных"=>"native_edited",
                            "Состояние документа ещё неизвестно"|"История текущего документа ещё неизвестна"|"Адрес документа ещё не подтверждён"|"Вкладка уже закрыта"|"Вкладка уже выгружена"|"Вкладка меняет сетевой путь"=>"native_document_unknown",
                            _=>"native_document_unknown",
                        };
                        let token=crate::super_hibernation_protocol::Token{generation:request.scope.generation,attempt:request.scope.attempt,revision:request.revision};
                        let frame=crate::super_hibernation_protocol::Frame::Refused{token,reason:reason.into()};
                        println!("NB-HIBERNATE {}",serde_json::to_string(&frame)?);std::io::stdout().flush()?;continue;
                    }
                    let safe=label.is_empty()&&extras.is_empty()&&!*force_private&&!*browser_locked&&!*closing
                        &&route_attempts.is_empty()&&sleep_requests.is_empty()&&pending_downloads.is_empty()&&devtools.is_empty()&&devtools_pending.is_empty()&&perevody.is_empty()&&data_requests.is_empty()&&deferred_actions.is_empty()
                        &&s.vkladki.iter().all(|v|!v.privatnaya&&!v.route_pending&&v.route_error.is_none())
                        &&brauzery.keys().all(|id|memory_blocked_scoped(*id,true,s,brauzery,devtools_pending,*force_private,true).is_none());
                    hibernate.begin(request,k,s,brauzery,memory_scroll,safe,tx)?;continue;
                }
                Sobytie::Komanda(Komanda::HibernateCommit{..}|Komanda::HibernateAbort{..}|Komanda::HibernateFinalAck{..})=>{continue;}
                Sobytie::Komanda(Komanda::OknoNovoe{label:new_label,parent,x,y,w,h,privatnaya,sessiya})=>{
                    if !label.is_empty()||extras.contains_key(&new_label){continue;}
                    if closed_labels.contains(&new_label){println!("NB-UI {}",serde_json::json!({"tip":"okno-closed","label":new_label}));std::io::stdout().flush()?;continue;}
                    let mut child_k=match Konteyner::sozdat(parent,Mesto{x,y,w,h},k.routes.clone()){Ok(k)=>k,Err(e)=>{eprintln!("cef: новое окно: {e}");println!("NB-UI {}",serde_json::json!({"tip":"okno-closed","label":new_label}));std::io::stdout().flush()?;continue;}};

                    let (child_tx,child_rx)=channel();
                    #[cfg(target_os = "windows")]
                    let (child_rx, child_event_relay) = match crate::cef_events::relay(&child_tx, child_rx, Sobytie::Komanda(Komanda::ObnovitInterfeys), request_wake) {
                        Ok(relay) => relay,
                        Err(e) => {
                            eprintln!("cef: обработчик нового окна: {e}");
                            println!("NB-UI {}",serde_json::json!({"tip":"okno-closed","label":new_label}));
                            std::io::stdout().flush()?;
                            continue;
                        }
                    };
                    let restoring=sessiya.is_some();
                    let mut child_s=sessiya.unwrap_or_else(||Sessiya::novaya("about:blank"));
                    if child_s.vkladki.is_empty(){child_s=Sessiya::novaya("about:blank");}
                    static NEXT_VIEW:std::sync::atomic::AtomicU64=std::sync::atomic::AtomicU64::new(1);
                    let base=NEXT_VIEW.fetch_add(1,std::sync::atomic::Ordering::Relaxed)*1_000_000;
                    let active=child_s.vkladki.iter().position(|v|v.id==child_s.aktivnaya).unwrap_or(0);
                    for (i,v) in child_s.vkladki.iter_mut().enumerate(){v.id=base+i as u64;v.privatnaya|=privatnaya||crate::privatnost::process_privatnyj();v.zagruzka=false;v.memory_pending=false;v.otlozhena=restoring&&i!=active;}
                    child_s.aktivnaya=child_s.vkladki[active].id;child_s.sleduyushchaya=base+child_s.vkladki.len() as u64;
                    let mut child_b=HashMap::new();
                    for tab in &child_s.vkladki{child_k.choices.borrow_mut().insert(tab.id,tab.route.clone());}
                    let tab=&child_s.vkladki[active];
                    match sozdat_brauzer(&child_k,tab.id,&tab.adres,&child_tx,tab.privatnaya){Ok(b)=>{if let Some(b)=b{child_b.insert(tab.id,b);}},Err(e)=>{eprintln!("cef: страница нового окна: {e}");println!("NB-UI {}",serde_json::json!({"tip":"okno-closed","label":new_label}));std::io::stdout().flush()?;continue;}}
                    let hwnd=child_k.okno;let size=child_k.razmer();
                    let session=child_s.clone();
                    extras.insert(new_label.clone(),Rabota{k:child_k,s:child_s,tx:child_tx,rx:child_rx,#[cfg(target_os = "windows")] _event_relay:child_event_relay,#[cfg(target_os = "windows")] native_visibility:Default::default(),brauzery:child_b,deferred_actions:Default::default(),pending_downloads:Default::default(),data_requests:Default::default(),route_attempts:Default::default(),sleep_requests:Default::default(),memory_registrations:Default::default(),memory_last_used:Default::default(),memory_next_scan:std::time::Instant::now(),memory_enabled:crate::nastrojki::zagruzit().memory_saver,memory_minutes:crate::nastrojki::zagruzit().memory_saver_minutes,memory_scroll:Default::default(),hibernate:Default::default(),razmer:size,devtools:HashMap::new(),devtools_pending:HashMap::new(),devtools_procent:40,devtools_pixels:None,ui_last:String::new(),perevody:HashMap::new(),perevod_avto:HashMap::new(),perevod_due:HashMap::new(),perevod_ui:HashMap::new(),label:new_label.clone(),extras:HashMap::new(),closed_labels:Default::default(),closing:false,browser_locked:*browser_locked,browser_lock_generation:*browser_lock_generation,force_private:privatnaya||crate::privatnost::process_privatnyj()});
                    println!("NB-UI {}",serde_json::json!({"tip":"okno-ready","label":new_label,"hwnd":hwnd,"sessiya":session}));std::io::stdout().flush()?;
                    continue;
                }
                Sobytie::Komanda(Komanda::RoutePrepareSnapshot{snapshot,generation})=>{
                    let error=k.routes.borrow_mut().snapshot(snapshot).err().map(|_|"Некорректный снимок маршрутов");
                    let _=tx.send(Sobytie::Sluzhebnoe(serde_json::json!({"tip":"route-snapshot-result","generation":generation,"ok":error.is_none(),"error":error,"error_code":error.map(route_error_code)})));continue;
                }
                Sobytie::Komanda(Komanda::RouteApply{id,route,generation,prepared,expected_browser})=>{
                    let Some(tab)=s.vkladki.iter_mut().find(|v|v.id==id)else{route_result(tx,id,generation,expected_browser,Some("Вкладка закрыта"));continue;};
                    let actual=brauzery.get(&id).map(|b|b.identifier()).unwrap_or(0);
                    let pending=route_attempts.get(&id);
                    let matching=pending.is_some_and(|a|a.generation==generation&&a.expected==expected_browser&&a.route==route&&!a.closing);
                    if *browser_locked{
                        if generation>tab.route_generation&&pending.is_none(){tab.route_generation=generation;tab.route_error=Some("Браузер заблокирован".into());izmeneno=true;}
                        if matching{route_attempts.remove(&id);tab.route_pending=false;tab.route_error=Some("Браузер заблокирован".into());izmeneno=true;}
                        route_result(tx,id,generation,expected_browser,Some("Браузер заблокирован"));continue;
                    }
                    if actual!=expected_browser||(!matching&&(generation<=tab.route_generation||pending.is_some())){
                        // Accepted attempt numbers never become reusable when an
                        // old parent snapshot races a wake/recreated Browser.
                        if generation>tab.route_generation&&pending.is_none(){tab.route_generation=generation;tab.route_error=Some("Вкладка уже изменилась; выберите путь повторно".into());izmeneno=true;}
                        route_result(tx,id,generation,expected_browser,Some("Вкладка уже изменилась"));continue;
                    }
                    tab.route_generation=generation; // consumed also on failure; never reused
                    if !prepared.validate(){tab.route_pending=false;tab.route_error=Some("Недопустимый маршрут".into());route_attempts.remove(&id);route_result(tx,id,generation,expected_browser,tab.route_error.as_deref());izmeneno=true;continue;}
                    tab.route_pending=true;tab.route_error=None;
                    if perevody.remove(&id).is_some(){let _=tx.send(Sobytie::Sluzhebnoe(serde_json::json!({"tip":"perevod-otmena","id":id})));}perevod_due.remove(&id);
                    route_attempts.insert(id,RouteAttempt{route,prepared:Some(prepared),generation,expected:expected_browser,closing:false,closed:expected_browser==0,since:std::time::Instant::now(),failure:None});
                    izmeneno=true;continue;
                }
                Sobytie::Komanda(Komanda::RouteFailed{id,generation,error,expected_browser})=>{
                    if route_attempts.get(&id).is_some_and(|a|a.generation==generation&&a.expected==expected_browser&&!a.closing){
                        route_attempts.remove(&id);if let Some(tab)=s.vkladki.iter_mut().find(|v|v.id==id){tab.route_pending=false;tab.route_error=Some(error.chars().take(300).collect());route_result(tx,id,generation,expected_browser,tab.route_error.as_deref());izmeneno=true;}
                    }continue;
                }
                Sobytie::RouteRefused(id,identity)=>{
                    if route_attempts.get(&id).is_some_and(|a|a.expected==identity){let a=route_attempts.remove(&id).unwrap();if let Some(tab)=s.vkladki.iter_mut().find(|v|v.id==id){tab.route_pending=false;tab.route_error=Some("Страница удерживает несохранённые данные".into());route_result(tx,id,a.generation,a.expected,tab.route_error.as_deref());izmeneno=true;}}continue;
                }
                Sobytie::Komanda(Komanda::OknoKomanda{label:target,komanda})=>{if let Some(extra)=extras.get(&target){let _=extra.tx.send(Sobytie::Komanda(*komanda));}continue;}
                Sobytie::Komanda(Komanda::OknoZakryt{label:target})=>{
                    closed_labels.insert(target.clone());
                    if let Some(extra)=extras.get_mut(&target){extra.closing=true;extra.s.vkladki.retain(|v|extra.brauzery.contains_key(&v.id));for b in extra.brauzery.values(){if let Some(h)=b.host(){h.close_browser(1);}}}
                    else{println!("NB-UI {}",serde_json::json!({"tip":"okno-closed","label":target}));std::io::stdout().flush()?;}
                    continue;
                }
                Sobytie::Komanda(Komanda::GlavnoePokazat)=>{if label.is_empty(){*closing=false;}}
                Sobytie::Komanda(Komanda::GlavnoeSkryto)=>{
                    if label.is_empty(){
                        *closing=true; // Не создаём скрытую приватную blank-вкладку.
                        for tab in s.vkladki.iter().filter(|v|v.privatnaya){
                            if let Some(h)=brauzery.get(&tab.id).and_then(|b|b.host()){h.close_browser(1);}else{let _=tx.send(Sobytie::Zakryta(tab.id,None));}
                        }
                    }
                }
                Sobytie::Komanda(Komanda::PageContextChoose{token,command_id})=>{let ids=brauzery.values().map(|b|b.identifier()).collect::<Vec<_>>();crate::cef_interfeys::choose_context(&token,command_id,&ids);}
                Sobytie::Komanda(Komanda::OpenLink{opener_id,adres,privatnaya,background})=>{
                    let Some(opener)=s.vkladki.iter().find(|v|v.id==opener_id&&brauzery.contains_key(&opener_id))else{continue;};
                    let private=privatnaya||opener.privatnaya||*force_private;
                    let opener_route=opener.route.clone();
                    let active=s.aktivnaya;
                    let mut next=s.clone();let id=next.dobavit(&adres);
                    {let tab=next.vkladki.iter_mut().find(|v|v.id==id).unwrap();tab.privatnaya=private;tab.route=opener_route.clone();}
                    k.choices.borrow_mut().insert(id,opener_route);
                    if background{next.aktivnaya=active;}
                    match sozdat_brauzer(k,id,&adres,tx,private){
                        Ok(browser)=>{if let Some(browser)=browser{brauzery.insert(id,browser);}*s=next;fokus=true;if !background{fokus_klaviatury=true;}}
                        Err(_)=>eprintln!("cef: не удалось открыть ссылку в новой вкладке"),
                    }
                }
                Sobytie::Komanda(Komanda::Novaya { adres }|Komanda::NovayaPrivatnaya { adres }) => {
                    if label.is_empty(){*closing=false;}
                    let mut novaya = s.clone();
                    let private=private_command||*force_private;
                    let id = novaya.dobavit(&adres);
                    novaya.vkladki.iter_mut().find(|v|v.id==id).unwrap().privatnaya=private;
                    match sozdat_brauzer(&k, id, &adres, &tx, private) {
                        Ok(b) => {
                            if let Some(b)=b{brauzery.insert(id, b);}
                            *s = novaya;
                            fokus = true;
                            fokus_klaviatury = true;
                        }
                        Err(e) => eprintln!("cef: {e}"),
                    }
                }
                Sobytie::Komanda(Komanda::ZagruzkaUpravlenie{id,action})=>{crate::zagruzki::upravlyat(id,&action);}
                Sobytie::Komanda(Komanda::Skachat{adres,privatnaya})=>{
                    let privatnaya=privatnaya||*force_private;
                    let target=s.vkladki.iter().find(|v|v.id==s.aktivnaya&&v.privatnaya==privatnaya).or_else(||s.vkladki.iter().find(|v|v.privatnaya==privatnaya&&brauzery.contains_key(&v.id))).map(|v|v.id);
                    let target=if let Some(id)=target{id}else{let id=s.dobavit("about:blank");s.vkladki.iter_mut().find(|v|v.id==id).unwrap().privatnaya=privatnaya;if let Some(browser)=sozdat_brauzer(k,id,"about:blank",tx,privatnaya)?{brauzery.insert(id,browser);}fokus=true;id};
                    if let Some(h)=brauzery.get(&target).and_then(|b|b.host()){h.start_download(Some(&CefString::from(adres.as_str())));}
                    else{if let Some(tab)=s.vkladki.iter_mut().find(|v|v.id==target){tab.otlozhena=false;}let queue=pending_downloads.entry(target).or_default();if queue.len()<4{queue.push(adres);}}
                }
                Sobytie::Komanda(Komanda::VosstanovitZakrytuyu) => {
                    if let Some(old)=s.zakrytye.pop(){
                        let id=s.dobavit(&old.adres);let current=s.vkladki.iter_mut().find(|v|v.id==id).unwrap();current.zagolovok=old.zagolovok;current.zakreplena=old.zakreplena;current.route=old.route.clone();k.choices.borrow_mut().insert(id,old.route.clone());
                        if let Some(browser)=sozdat_brauzer(k,id,&old.adres,tx,false)?{brauzery.insert(id,browser);}fokus=true;fokus_klaviatury=true;
                    }
                }
                Sobytie::Komanda(Komanda::Vybrat { id }) => {
                    if s.vkladki.iter().any(|v| v.id == id) {
                        s.aktivnaya = id;
                        fokus = true;
                        fokus_klaviatury = true;
                    }
                }
                Sobytie::Komanda(Komanda::MemorySettings{enabled,minutes})=>{
                    let policy_changed=*memory_enabled!=enabled||*memory_minutes!=minutes.clamp(15,240);
                    *memory_enabled=enabled;*memory_minutes=minutes.clamp(15,240);
                    if policy_changed{let ids=sleep_requests.iter().filter(|(_,a)|a.automatic&&a.stage!=MemoryStage::Closing).map(|(id,_)|*id).collect::<Vec<_>>();for id in ids{memory_refused(id,"Настройки экономии памяти изменились",s,tx,sleep_requests,memory_registrations,brauzery);}}
                    for extra in extras.values(){let _=extra.tx.send(Sobytie::Komanda(Komanda::MemorySettings{enabled,minutes}));}
                }
                Sobytie::Komanda(Komanda::MemoryKeep{id,keep})=>{
                    if let Some(v)=s.vkladki.iter_mut().find(|v|v.id==id){v.memory_keep=keep;}
                    if keep&&sleep_requests.get(&id).is_some_and(|a|a.stage!=MemoryStage::Closing){memory_refused(id,"Вкладка исключена из автоматической выгрузки",s,tx,sleep_requests,memory_registrations,brauzery);}
                }
                Sobytie::Komanda(Komanda::Usypit{id}|Komanda::MemoryAuto{id}) => {
                    let automatic=memory_automatic;
                    if automatic&&!*memory_enabled{continue;}
                    let reason=memory_blocked(id,automatic,s,brauzery,devtools_pending,*force_private)
                        .or_else(||if sleep_requests.contains_key(&id){Some("Выгрузка этой вкладки уже запрошена")}else{None})
                        .or_else(||if perevody.contains_key(&id){Some("Дождитесь завершения перевода страницы")}else{None});
                    if let Some(error)=reason {
                        eprintln!("NEXUSBRO_MEMORY tab={id} automatic={automatic} refused={error}");
                        if !automatic{let _=tx.send(Sobytie::Sluzhebnoe(serde_json::json!({"tip":"memory-result","id":id,"ok":false,"error":error})));}
                        continue;
                    }
                    if let Some(browser)=brauzery.get(&id){if let Some(token)=crate::memory_guard::token(browser.identifier()){
                        static NEXT_MEMORY:std::sync::atomic::AtomicU64=std::sync::atomic::AtomicU64::new(1);
                        let request=NEXT_MEMORY.fetch_add(1,std::sync::atomic::Ordering::Relaxed).to_string();
                        let identity=browser.identifier();
                        crate::cef_interfeys::cancel_context(identity);
                        let sent=crate::memory_cef::probe(browser,&request,&token);
                        sleep_requests.insert(id,MemoryAttempt{identity,token,request,stage:MemoryStage::Preflight,automatic,started:std::time::Instant::now(),wake:false,timed_out:false,scroll:(0.0,0.0)});
                        if let Some(v)=s.vkladki.iter_mut().find(|v|v.id==id){v.memory_pending=true;}
                        eprintln!("NEXUSBRO_MEMORY tab={id} browser={identity} requested={}",if automatic{"automatic"}else{"manual"});
                        if !sent{memory_refused(id,"Не удалось проверить состояние страницы",s,tx,sleep_requests,memory_registrations,brauzery);}
                    }}
                }
                Sobytie::Komanda(Komanda::Zakryt { id }) => {
                    if let Some(attempt)=route_attempts.remove(&id){
                        ROUTE_CLOSES.with(|m|{m.borrow_mut().remove(&attempt.expected);});if let Some(b)=brauzery.get(&id){crate::memory_cef::resume(b);}
                        if let Some(tab)=s.vkladki.iter_mut().find(|v|v.id==id){tab.route_pending=false;}
                        route_result(tx,id,attempt.generation,attempt.expected,Some("Смена пути отменена: вкладка закрывается"));
                    }
                    if sleep_requests.contains_key(&id){memory_refused(id,"Выгрузка отменена: вкладка закрывается",s,tx,sleep_requests,memory_registrations,brauzery);}
                    fokus_klaviatury |= s.aktivnaya == id;
                    if let Some(h) = brauzery.get(&id).and_then(|b| b.host()) {
                        h.close_browser(0);
                    } else {
                        let _ = tx.send(Sobytie::Zakryta(id,None));
                    }
                }
                Sobytie::Komanda(Komanda::ZakrytDrugie { id, sprava }) => {
                    if let Some(position)=s.vkladki.iter().position(|v|v.id==id){
                        let ids=s.vkladki.iter().enumerate().filter(|(i,v)|v.id!=id&&!v.zakreplena&&(!sprava||*i>position)).map(|(_,v)|v.id).collect::<Vec<_>>();
                        for id in ids{
                            if let Some(attempt)=route_attempts.remove(&id){
                                ROUTE_CLOSES.with(|m|{m.borrow_mut().remove(&attempt.expected);});if let Some(b)=brauzery.get(&id){crate::memory_cef::resume(b);}
                                if let Some(tab)=s.vkladki.iter_mut().find(|v|v.id==id){tab.route_pending=false;}
                                route_result(tx,id,attempt.generation,attempt.expected,Some("Смена пути отменена: вкладка закрывается"));
                            }
                            if sleep_requests.contains_key(&id){memory_refused(id,"Выгрузка отменена: вкладка закрывается",s,tx,sleep_requests,memory_registrations,brauzery);}
                            if let Some(h)=brauzery.get(&id).and_then(|b|b.host()){h.close_browser(0);}
                            else{let _=tx.send(Sobytie::Zakryta(id,None));}
                        }
                    }
                }
                #[cfg(target_os = "windows")]
                Sobytie::ZakrytOkno(id,identity) => {
                    if let Some(b) = brauzery.get(&id).filter(|b|b.identifier()==identity) {
                        use windows::Win32::{
                            Foundation::HWND,
                            System::Threading::{GetCurrentProcessId, GetCurrentThreadId},
                            UI::WindowsAndMessaging::*,
                        };
                        unsafe {
                            let h = HWND(handle(b) as usize as _);
                            let mut pid = 0;
                            let thread = GetWindowThreadProcessId(h, Some(&mut pid));
                            if pid == GetCurrentProcessId()
                                && thread == GetCurrentThreadId()
                                && GetParent(h).ok().map(|p| p.0 as usize as u32) == Some(k.okno)
                            {
                                if let Err(e) = DestroyWindow(h) {
                                    eprintln!("cef: закрытие окна вкладки {id}: {e}");
                                }
                            } else {
                                eprintln!("cef: отказ закрытия чужого окна вкладки {id}");
                            }
                        }
                    }
                }
                Sobytie::NavigationFailed(id,identity,url,epoch,code)=>{
                    if !brauzery.get(&id).is_some_and(|browser|browser.identifier()==identity)
                        ||crate::memory_guard::diagnostic_epoch(identity,&url)!=Some(epoch){continue;}
                    if let Some(tab)=s.vkladki.iter().find(|t|t.id==id&&t.adres==url&&!t.route_pending){
                        let _=tx.send(Sobytie::Sluzhebnoe(serde_json::json!({"tip":"navigation-error","id":id,"code":code,"route":tab.route,"route_generation":tab.route_generation})));
                    }
                }
                Sobytie::MainLoadFailed(id,identity,failed_url,epoch,logical_target,code)=>{
                    if !brauzery.get(&id).is_some_and(|browser|browser.identifier()==identity)
                        ||crate::memory_guard::native_error_epoch(identity,&failed_url)!=Some(epoch){continue;}
                    if let Some(tab)=s.vkladki.iter_mut().find(|tab|tab.id==id&&(tab.adres==failed_url||tab.adres==logical_target)){
                        tab.zagruzka=false;izmeneno=true;
                        let _=tx.send(Sobytie::Sluzhebnoe(serde_json::json!({"tip":"navigation-error","id":id,"code":code,"route":tab.route,"route_generation":tab.route_generation})));
                    }
                }
                Sobytie::MemoryLoaded(id,identity)=>{
                    if let Some(browser)=brauzery.get(&id).filter(|b|b.identifier()==identity){
                        if let Some((x,y))=memory_scroll.remove(&id){if let Some(frame)=browser.main_frame(){
                            let js=format!("requestAnimationFrame(()=>scrollTo({x},{y}))");
                            frame.execute_java_script(Some(&CefString::from(js.as_str())),None,0);
                        }}
                    }
                }
                Sobytie::MemoryMessage(id,identity,data)=>{
                    let Some(browser)=brauzery.get(&id).filter(|b|b.identifier()==identity)else{continue;};
                    if data["kind"]=="NB_MEMORY_DOCUMENT"{
                        if sleep_requests.get(&id).is_some_and(|a|a.stage!=MemoryStage::Closing&&Some(a.token.as_str())!=data["token"].as_str()){
                            memory_refused(id,"Документ изменился во время проверки",s,tx,sleep_requests,memory_registrations,brauzery);fokus=true;izmeneno=true;
                        }
                        continue;
                    }
                    let Some(attempt)=sleep_requests.get(&id).filter(|a|a.identity==identity&&Some(a.request.as_str())==data["request"].as_str()&&Some(a.token.as_str())==data["token"].as_str())else{continue;};
                    if !matches!(attempt.stage,MemoryStage::Preflight|MemoryStage::Verify){continue;}
                    let automatic=attempt.automatic;
                    if data["known"]!=true{
                        let reason=data["reason"].as_str().filter(|s|s.len()<=160).unwrap_or("missing-reason");
                        eprintln!("NEXUSBRO_MEMORY tab={id} browser={identity} automatic={automatic} probe=unknown reason={reason}");
                    }
                    let reason=if *browser_locked||*closing{Some("Браузер заблокирован или закрывается".to_owned())}else{memory_blocked(id,automatic,s,brauzery,devtools_pending,*force_private).map(str::to_owned)}
                        // Disk policy is an independent safety gate if delivery
                        // of a just-saved disable/interval change was delayed.
                        .or_else(||if automatic{match crate::nastrojki::zagruzit_sync(){
                            Ok(value)=>{let prefs=value.unwrap_or_default();if !prefs.memory_saver||prefs.memory_saver_minutes.clamp(15,240)!=*memory_minutes{Some("Настройки экономии памяти изменились".into())}else{None}},
                            Err(_)=>Some("Не удалось подтвердить настройки экономии памяти".into()),
                        }}else{None})
                        .or_else(||if crate::memory_guard::token(identity).as_deref()!=Some(attempt.token.as_str()){Some("Документ изменился во время проверки".into())}else{None})
                        .or_else(||if data["known"]!=true{Some("Безопасность выгрузки страницы не подтверждена".into())}else{None})
                        .or_else(||data["reason"].as_str().map(str::to_owned));
                    if let Some(reason)=reason{memory_refused(id,&reason,s,tx,sleep_requests,memory_registrations,brauzery);fokus=true;izmeneno=true;continue;}
                    let stage=attempt.stage;
                    let request=attempt.request.clone();
                    if stage==MemoryStage::Preflight {
                        if let Some(registration)=crate::memory_cef::freeze(browser,id,&request,tx.clone()){
                            memory_registrations.insert(id,registration);
                            sleep_requests.get_mut(&id).unwrap().stage=MemoryStage::Freezing;
                        }else{memory_refused(id,"Не удалось подготовить страницу к выгрузке",s,tx,sleep_requests,memory_registrations,brauzery);fokus=true;}
                    }else{
                        let attempt=sleep_requests.get_mut(&id).unwrap();
                        attempt.scroll=(data["scroll_x"].as_f64().filter(|v|v.is_finite()).unwrap_or(0.0).clamp(-1e8,1e8),data["scroll_y"].as_f64().filter(|v|v.is_finite()).unwrap_or(0.0).clamp(-1e8,1e8));
                        if let Some(host)=browser.host(){
                            attempt.stage=MemoryStage::Closing;attempt.started=std::time::Instant::now();
                            SLEEP_CLOSES.with(|m|{m.borrow_mut().insert(identity);});
                            eprintln!("NEXUSBRO_MEMORY tab={id} browser={identity} verified=two-phase close=graceful automatic={automatic}");
                            host.close_browser(0);
                        }else{memory_refused(id,"Страница уже недоступна",s,tx,sleep_requests,memory_registrations,brauzery);fokus=true;}
                    }
                }
                Sobytie::MemoryFrozen(id,identity,request,success)=>{
                    let Some(attempt)=sleep_requests.get_mut(&id).filter(|a|a.identity==identity&&a.request==request&&a.stage==MemoryStage::Freezing)else{
                        if !sleep_requests.contains_key(&id){if let Some(b)=brauzery.get(&id).filter(|b|b.identifier()==identity){crate::memory_cef::resume(b);}}continue;
                    };
                    attempt.stage=MemoryStage::Verify;
                    let sent=success&&brauzery.get(&id).is_some_and(|b|crate::memory_cef::probe(b,&attempt.request,&attempt.token));
                    if !sent{memory_refused(id,"Не удалось завершить проверку страницы",s,tx,sleep_requests,memory_registrations,brauzery);fokus=true;}
                }
                Sobytie::MemoryRefused(id,identity) => {
                    if sleep_requests.get(&id).is_some_and(|a|a.identity==identity){memory_refused(id,"Страница удерживает несохранённые данные и осталась в памяти",s,tx,sleep_requests,memory_registrations,brauzery);fokus=true;}
                }
                Sobytie::Zakryta(id,identity) => {
                    // A callback from the previous WebContents cannot close a
                    // replacement, and deferred-row removal has no live identity.
                    if brauzery.get(&id).map(|b|b.identifier())!=identity{continue;}
                    if let Some(identity)=identity{k.routes.borrow_mut().forget_browser(identity);}
                    if route_attempts.get(&id).is_some_and(|a|a.closing&&Some(a.expected)==identity){
                        brauzery.remove(&id);if let Some(a)=route_attempts.get_mut(&id){a.closed=true;}
                        memory_registrations.remove(&id);memory_scroll.remove(&id);memory_last_used.remove(&id);
                        crate::poisk::ochistit(id);continue;
                    }
                    if identity.is_some() && sleep_requests.get(&id).map(|a|a.identity)==identity {
                        let attempt=sleep_requests.remove(&id).unwrap();
                        memory_registrations.remove(&id);
                        memory_scroll.insert(id,attempt.scroll);
                        brauzery.remove(&id);
                        crate::poisk::ochistit(id);
                        if let Some(v)=s.vkladki.iter_mut().find(|v|v.id==id){v.otlozhena=true;v.memory_pending=false;v.zagruzka=false;v.nazad=false;v.vpered=false;}
                        // An explicit selection while close was pending wins;
                        // create the replacement only AFTER OnBeforeClose.
                        fokus=true;fokus_klaviatury|=attempt.wake&&s.aktivnaya==id;
                        izmeneno=true;
                        let _=tx.send(Sobytie::Sluzhebnoe(serde_json::json!({"tip":"memory-result","id":id,"ok":true})));
                        eprintln!("NEXUSBRO_MEMORY tab={id} browser={} closed=row-retained",identity.unwrap());
                        continue;
                    }
                    sleep_requests.remove(&id);memory_registrations.remove(&id);memory_scroll.remove(&id);memory_last_used.remove(&id);
                    crate::poisk::ochistit(id);
                    if let Some(tab)=s.vkladki.iter().find(|v|v.id==id&&!v.privatnaya&&v.adres!="about:blank").cloned(){s.zakrytye.push(tab);if s.zakrytye.len()>30{s.zakrytye.remove(0);}}
                    if s.aktivnaya == id {
                        // The close callback can arrive in a later relay tick
                        // than Komanda::Zakryt. Preserve focus for the neighbour.
                        fokus_klaviatury = true;
                        if let Some(i) = s.vkladki.iter().position(|v| v.id == id) {
                            if let Some(v) = s.vkladki.get(if i > 0 { i - 1 } else { i + 1 }) {
                                s.aktivnaya = v.id;
                            }
                        }
                    }
                    brauzery.remove(&id);
                    s.vkladki.retain(|v| v.id != id);
                    fokus = true;
                }
                Sobytie::Komanda(Komanda::Zakrepit { id, zakreplena }) => {
                    if let Some(v) = s.vkladki.iter_mut().find(|v| v.id == id) {
                        v.zakreplena = zakreplena;
                    }
                    s.vkladki.sort_by_key(|v| !v.zakreplena);
                }
                Sobytie::Komanda(Komanda::Pereyti { id, adres }) => {
                    if let Some(b) = brauzery.get(&id) {
                        if let Some(f) = b.main_frame() {
                            if razbuzhen != Some(id) {
                                f.load_url(Some(&CefString::from(adres.as_str())));
                            }
                        }
                        if let Some(v) = s.vkladki.iter_mut().find(|v| v.id == id) {
                            v.ikonka = None;
                            v.adres = adres;
                            v.zagruzka = true;
                        }
                    }
                }
                Sobytie::Komanda(Komanda::Nazad { id }) => {
                    if let Some(b) = brauzery.get(&id) {
                        b.go_back();
                    }
                }
                Sobytie::Komanda(Komanda::Vpered { id }) => {
                    if let Some(b) = brauzery.get(&id) {
                        b.go_forward();
                    }
                }
                Sobytie::Komanda(Komanda::Instrumenty { id }) => {
                    if let Some(b) = brauzery.get(&id) {
                        crate::cef_interfeys::instrumenty(b, None, true, id, tx.clone());
                    }
                }
                Sobytie::Komanda(Komanda::Issledovat { id, x, y }) => {
                    if let Some(b) = brauzery.get(&id) {
                        crate::cef_interfeys::instrumenty(b, Some(Point { x, y }), false, id, tx.clone());
                    }
                }
                Sobytie::Komanda(Komanda::Iskhodnik { id }) => {
                    if let Some(v) = s.vkladki.iter().find(|v| v.id == id) {
                        let adres = format!(
                            "view-source:{}",
                            v.adres.strip_prefix("view-source:").unwrap_or(&v.adres)
                        );
                        let mut next = s.clone();
                        let private=v.privatnaya;let route=v.route.clone();
                        let n = next.dobavit(&adres);{let tab=next.vkladki.iter_mut().find(|v|v.id==n).unwrap();tab.privatnaya=private;tab.route=route.clone();}k.choices.borrow_mut().insert(n,route);
                        match sozdat_brauzer(k, n, &adres, tx, private) {
                            Ok(b) => {
                                if let Some(b)=b{brauzery.insert(n, b);}
                                *s = next;
                                fokus = true;
                                fokus_klaviatury = true;
                            }
                            Err(e) => eprintln!("cef: исходный код: {e}"),
                        }
                    }
                }
                Sobytie::Komanda(Komanda::Peremestit { id, pered }) => s.peremestit(id, pered),
                Sobytie::Komanda(Komanda::Sleduyushchaya { nazad }) => {
                    if let Some(i) = s.vkladki.iter().position(|v| v.id == s.aktivnaya) {
                        let n = s.vkladki.len();
                        s.aktivnaya = s.vkladki[(i + if nazad { n - 1 } else { 1 }) % n].id;
                        fokus = true;
                        fokus_klaviatury = true;
                    }
                }
                Sobytie::Komanda(Komanda::Masshtab { procent }) => {
                    for extra in extras.values(){let _=extra.tx.send(Sobytie::Komanda(Komanda::Masshtab{procent}));}
                    let level = (procent.clamp(50, 200) as f64 / 100.0).ln() / 1.2_f64.ln();
                    for b in brauzery.values() {
                        if let Some(h) = b.host() {
                            h.set_zoom_level(level);
                        }
                    }
                }
                Sobytie::DevtoolsGotovy(id) => {if *browser_locked{if let Some(host)=brauzery.get(&id).and_then(|b|b.host()){host.close_dev_tools();}}else if brauzery.contains_key(&id){devtools_pending.insert(id,std::time::Instant::now());}}
                #[cfg(target_os="windows")]
                Sobytie::DevtoolsZakrytOkno(_id,hwnd)=>{
                    use windows::Win32::{Foundation::HWND,System::Threading::{GetCurrentProcessId,GetCurrentThreadId},UI::WindowsAndMessaging::*};
                    unsafe{let h=HWND(hwnd as usize as _);let mut pid=0;let tid=GetWindowThreadProcessId(h,Some(&mut pid));
                        if pid==GetCurrentProcessId()&&tid==GetCurrentThreadId()&&GetParent(h).ok().is_some_and(|p|p.0 as usize as u32==k.okno){let _=DestroyWindow(h);}}
                    layout_dirty=true;
                }
                Sobytie::DevtoolsZakryty(id) => {
                    devtools_pending.remove(&id);devtools.remove(&id);layout_dirty=true;
                    if s.aktivnaya==id&&!*closing{fokus=true;fokus_klaviatury=true;fokus_posle_devtools=true;}
                }
                Sobytie::Komanda(Komanda::FonKonteynera{rgb})=>{k.fon(rgb&0xffffff);}
                Sobytie::Komanda(Komanda::BrowserLocked{locked,generation})=>{
                    if generation<*browser_lock_generation{continue;}
                    *browser_lock_generation=generation;
                    let was_locked=*browser_locked;
                    *browser_locked=locked;
                    for extra in extras.values_mut(){extra.set_browser_locked(locked,generation);}
                    if locked{
                        izmeneno|=cancel_routes_for_lock(s,route_attempts,brauzery,tx);
                        for browser in brauzery.values(){crate::cef_youtube::stop(browser);crate::cef_interfeys::cancel_context(browser.identifier());}
                        for browser in brauzery.values(){if let Some(host)=browser.host(){host.set_focus(0);host.close_dev_tools();}let _=sistema::pokazat(handle(browser),false);}
                        for hwnd in devtools.values(){let _=sistema::pokazat(*hwnd,false);}
                        devtools_pending.clear();devtools.clear();
                        let _=sistema::pokazat(k.okno,false);
                    }else{
                        // Initial unlocked-state synchronization must not wake
                        // restored lazy tabs. A real unlock may resume them.
                        fokus=true;fokus_klaviatury=was_locked&&!brauzery.contains_key(&s.aktivnaya);layout_dirty=true;
                    }
                }
                Sobytie::Komanda(Komanda::PasswordsEnabled{enabled})=>{
                    crate::password_cef::enabled(enabled);
                    if enabled{for tab in &s.vkladki{if let Some(b)=brauzery.get(&tab.id){crate::password_cef::install(b,tab.id,tab.privatnaya,tx.clone());}}}
                }
                Sobytie::Komanda(Komanda::PasswordForm{id,token,origin,payload})=>{
                    let allowed=s.vkladki.iter().any(|v|v.id==id&&s.aktivnaya==id&&!v.privatnaya&&!v.otlozhena)&&!crate::privatnost::process_privatnyj();
                    let browser_present=brauzery.contains_key(&id);
                    let origin_matches=brauzery.get(&id).and_then(|b|b.main_frame()).and_then(|f|crate::passwords::secure_origin(&CefString::from(&f.url()).to_string()).ok()).as_deref()==Some(origin.as_str());
                    let ok=allowed&&brauzery.get(&id).is_some_and(|b|crate::password_cef::request(b,id,token.clone(),origin,payload,tx.clone()));
                    if !ok{eprintln!("NEXUSBRO_PASSWORD_REQUEST refused allowed={allowed} browser={browser_present} origin={origin_matches}");let _=tx.send(Sobytie::Sluzhebnoe(serde_json::json!({"tip":"credential-result","token":token,"error":"Форма недоступна"})));}
                }
                Sobytie::Komanda(Komanda::DannyeStatistika{token})=>{
                    if data_requests.len()<8{data_requests.push((std::time::Instant::now(),Komanda::DannyeStatistika{token}));}
                    else{let _=tx.send(Sobytie::Sluzhebnoe(serde_json::json!({"tip":"dannye-obem","token":token,"cookies":null,"cache_bytes":null,"cache_memory":false})));}
                }
                Sobytie::Komanda(Komanda::OchistitDannye{token,cookies,cache})=>{
                    if data_requests.len()<8{data_requests.push((std::time::Instant::now(),Komanda::OchistitDannye{token,cookies,cache}));}
                    else{let _=tx.send(Sobytie::Sluzhebnoe(serde_json::json!({"tip":"ochistka-zavershena","token":token,"error":"Подождите завершения текущей операции"})));}
                }
                Sobytie::Komanda(Komanda::ObnovitInterfeys)=>{
                    ui_last.clear();
                    let mut status=perevod_ui.get(&s.aktivnaya).cloned().unwrap_or_else(||serde_json::json!({"tip":"perevod","id":s.aktivnaya,"original":true}));
                    status["auto"]=serde_json::json!(s.vkladki.iter().find(|v|v.id==s.aktivnaya).is_some_and(|v|avto_dlya(perevod_avto,s.aktivnaya,&v.adres)));
                    let _=tx.send(Sobytie::Sluzhebnoe(status));
                }
                Sobytie::Komanda(Komanda::PerevodAvto{id,enabled})=>{
                    if enabled{if let Some(origin)=s.vkladki.iter().find(|v|v.id==id).and_then(|v|origin(&v.adres)){perevod_avto.insert(id,origin);if !perevody.contains_key(&id){perevod_due.insert(id,std::time::Instant::now());}}}
                    else{perevod_avto.remove(&id);perevod_due.remove(&id);}
                    let mut status=perevod_ui.get(&id).cloned().unwrap_or_else(||serde_json::json!({"tip":"perevod","id":id,"done":0,"total":0}));status["auto"]=serde_json::json!(enabled);perevod_ui.insert(id,status.clone());let _=tx.send(Sobytie::Sluzhebnoe(status));
                }
                Sobytie::Komanda(Komanda::InstrumentyRazmer{procent}) => { *devtools_procent=procent.clamp(10,90);*devtools_pixels=None;layout_dirty=true; }
                Sobytie::Komanda(Komanda::Perevesti{id}) => {
                    if let Some(b)=brauzery.get(&id){
                        let token=format!("{}-{}",id,std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_nanos());
                        if let Some(registration)=crate::perevod::nachat(b,id,token.clone(),tx.clone()){
                            perevody.insert(id,(token,registration));
                            let status=serde_json::json!({"tip":"perevod","id":id,"start":true,"done":0,"total":0,"auto":s.vkladki.iter().find(|v|v.id==id).is_some_and(|v|avto_dlya(perevod_avto,id,&v.adres))});perevod_ui.insert(id,status.clone());let _=tx.send(Sobytie::Sluzhebnoe(status));
                        } else {
                            let _=tx.send(Sobytie::Sluzhebnoe(serde_json::json!({"tip":"perevod","id":id,"error":"Перевод доступен для загруженных веб-страниц. Попробуйте после загрузки."})));
                        }
                    }
                }
                Sobytie::Komanda(Komanda::PerevodOriginal{id}) => {
                    perevod_avto.remove(&id);perevod_due.remove(&id);perevod_ui.remove(&id);
                    perevody.remove(&id);
                    if let Some(b)=brauzery.get(&id){crate::perevod::original(b);}
                    let _=tx.send(Sobytie::Sluzhebnoe(serde_json::json!({"tip":"perevod-otmena","id":id})));
                    let _=tx.send(Sobytie::Sluzhebnoe(serde_json::json!({"tip":"perevod","id":id,"original":true})));
                }
                Sobytie::Komanda(Komanda::PerevodOtvet{id,token,items,done,total,error}) => {
                    if perevody.get(&id).is_some_and(|(t,_)|*t==token){
                        if let Some(b)=brauzery.get(&id){crate::perevod::primenit(b,&token,&items);}
                        let status=serde_json::json!({"tip":"perevod","id":id,"done":done,"total":total,"error":error,"auto":s.vkladki.iter().find(|v|v.id==id).is_some_and(|v|avto_dlya(perevod_avto,id,&v.adres))});perevod_ui.insert(id,status.clone());let _=tx.send(Sobytie::Sluzhebnoe(status));
                    }
                }
                Sobytie::Komanda(Komanda::Nayti { id, text, vpered, dalshe }) => {
                    if let Some(h) = brauzery.get(&id).and_then(|b| b.host()) {
                        if text.is_empty() {crate::poisk::ochistit(id); h.stop_finding(1); }
                        else {crate::poisk::nachat(id,&text,vpered,dalshe); h.find(Some(&CefString::from(text.as_str())), vpered as i32, 0, dalshe as i32); }
                    }
                }
                Sobytie::PoiskVybratPervoe(id,generation,text,forward)=>{
                    if crate::poisk::aktualnyj(id,generation){if let Some(h)=brauzery.get(&id).and_then(|b|b.host()){h.find(Some(&CefString::from(text.as_str())),forward as i32,0,1);}}
                }
                Sobytie::Komanda(Komanda::PoiskZakryt { id }) => {
                    crate::poisk::ochistit(id);
                    if let Some(h) = brauzery.get(&id).and_then(|b| b.host()) { h.stop_finding(1);h.set_focus(1); }
                }
                Sobytie::Sluzhebnoe(mut data) => {
                    crate::password_cef::completed(&data);
                    if data["tip"]=="perevod"||data["tip"]=="perevod-zapros"{if let Some(token)=data["token"].as_str(){let id=data["id"].as_u64().unwrap_or(0);if !perevody.get(&id).is_some_and(|(t,_)|t==token){continue;}if data["tip"]=="perevod"{data["auto"]=serde_json::json!(s.vkladki.iter().find(|v|v.id==id).is_some_and(|v|avto_dlya(perevod_avto,id,&v.adres)));perevod_ui.insert(id,data.clone());}}}

                    if data["privatnaya"]==true{
                        let source=data["id"].as_u64().or_else(||data["tab_id"].as_u64());
                        if !k.routes.borrow().has_private()||!source.is_some_and(|id|PRIVATE_IDS.with(|ids|ids.borrow().contains(&id))){continue;}
                    }
                    if data["tip"]=="perevod-zapros"{
                        let id=data["id"].as_u64().unwrap_or(0);
                        let Some(tab)=s.vkladki.iter().find(|v|v.id==id&&!v.route_pending)else{continue;};
                        let Some(browser)=brauzery.get(&id)else{continue;};
                        let Some(key)=k.routes.borrow().key_for_browser(browser.identifier())else{continue;};
                        data["route_key"]=serde_json::json!(key);data["route_generation"]=serde_json::json!(tab.route_generation);data["browser_id"]=serde_json::json!(browser.identifier());
                    }
                    data["label"]=serde_json::json!(label);
                    println!("NB-UI {}", serde_json::to_string(&data)?);
                    std::io::stdout().flush()?;
                }
                Sobytie::AdresnyiVvod => {
                    if label.is_empty(){println!("NB-ADRES");}else{println!("NB-UI {}",serde_json::json!({"tip":"adres-vvesti","label":label}));}
                    std::io::stdout().flush()?;
                }
                Sobytie::Ikonka(id, stranica, ikonka) => {
                    if let Some(v) = s
                        .vkladki
                        .iter_mut()
                        .find(|v| v.id == id && v.adres == stranica)
                    {
                        v.ikonka = Some(ikonka);
                    }
                }
                Sobytie::Komanda(Komanda::Obnovit { id }) => {
                    if let Some(b) = brauzery.get(&id) {
                        b.reload();
                    }
                }
                Sobytie::Komanda(Komanda::ObnovitBezKesha { id }) => {
                    if let Some(b) = brauzery.get(&id) {
                        b.reload_ignore_cache();
                    }
                }
                Sobytie::DownloadNachata(id,adres)=>{
                    if !adres.is_empty(){if let Some(v)=s.vkladki.iter_mut().find(|v|v.id==id){v.adres=adres;v.zagruzka=false;}}
                }
                Sobytie::Zagolovok(id, identity, t) => {
                    if !brauzery.get(&id).is_some_and(|b|b.identifier()==identity){continue;}
                    if let Some(v) = s.vkladki.iter_mut().find(|v| v.id == id) {
                        v.zagolovok = t;
                    }
                }
                Sobytie::Adres(id, identity, u) => {
                    if !brauzery.get(&id).is_some_and(|b|b.identifier()==identity){continue;}
                    let changed=s.vkladki.iter().find(|v|v.id==id).is_some_and(|v|bez_fragmenta(&v.adres)!=bez_fragmenta(&u));
                    if changed{
                        perevody.remove(&id);perevod_ui.remove(&id);perevod_due.remove(&id);
                        let _=tx.send(Sobytie::Sluzhebnoe(serde_json::json!({"tip":"perevod-otmena","id":id})));
                        if avto_dlya(perevod_avto,id,&u){perevod_due.insert(id,std::time::Instant::now()+std::time::Duration::from_millis(450));}
                        let _=tx.send(Sobytie::Sluzhebnoe(serde_json::json!({"tip":"perevod","id":id,"original":!avto_dlya(perevod_avto,id,&u),"auto":avto_dlya(perevod_avto,id,&u),"done":0,"total":0})));
                    }
                    if let Some(v) = s.vkladki.iter_mut().find(|v| v.id == id) {
                        // CEF сообщает underlying URL даже для view-source. Режим
                        // сохраняем для адресной строки и восстановления сессии;
                        // явный Pereyti выше предварительно меняет v.adres.
                        let u = if v.adres.starts_with("view-source:")
                            && !u.starts_with("view-source:")
                        {
                            format!("view-source:{u}")
                        } else {
                            u
                        };
                        if v.adres != u {
                            v.ikonka = None;
                        }
                        v.adres = u;
                    }
                }
                Sobytie::Zagruzka(id, identity, l, n, vp) => {
                    if !brauzery.get(&id).is_some_and(|b|b.identifier()==identity){continue;}
                    if let Some(v) = s.vkladki.iter_mut().find(|v| v.id == id) {
                        v.zagruzka = l;
                        v.nazad = n;
                        v.vpered = vp;
                        if l{if perevody.remove(&id).is_some(){let _=tx.send(Sobytie::Sluzhebnoe(serde_json::json!({"tip":"perevod-otmena","id":id})));}perevod_due.remove(&id);}
                        else if avto_dlya(perevod_avto,id,&v.adres)&&!perevody.contains_key(&id){perevod_due.insert(id,std::time::Instant::now()+std::time::Duration::from_millis(250));}
                    }
                }
            }
            izmeneno |= session_dirty;
        }
        if hibernate.active(){return Ok(());}
        let memory_now=std::time::Instant::now();
        memory_last_used.insert(s.aktivnaya,memory_now);
        for id in brauzery.keys(){memory_last_used.entry(*id).or_insert(memory_now);}
        let expired=sleep_requests.iter().filter(|(_,a)|!a.timed_out&&a.started.elapsed().as_secs()>=8).map(|(id,a)|(*id,a.stage)).collect::<Vec<_>>();
        for (id,stage) in expired{
            if stage==MemoryStage::Closing{
                // Retain the close classification: a late callback must keep
                // the row, never become a normal user close after a timeout.
                sleep_requests.get_mut(&id).unwrap().timed_out=true;
                let _=tx.send(Sobytie::Sluzhebnoe(serde_json::json!({"tip":"memory-result","id":id,"ok":false,"error":"Страница долго завершает выгрузку. Можно выбрать другую вкладку или закрыть эту."})));
                eprintln!("NEXUSBRO_MEMORY tab={id} timeout=close intent=retained");
            }else{memory_refused(id,"Страница не ответила на проверку и осталась в памяти",s,tx,sleep_requests,memory_registrations,brauzery);fokus=true;izmeneno=true;}
        }
        if memory_now>=*memory_next_scan{
            let test_seconds=std::env::var_os("NEXUSBRO_TEST_DATA").and_then(|_|std::env::var("NEXUSBRO_TEST_MEMORY_IDLE_SECONDS").ok()).and_then(|s|s.parse::<u64>().ok()).map(|s|s.clamp(1,300));
            *memory_next_scan=memory_now+std::time::Duration::from_secs(if test_seconds.is_some(){1}else{10});
            let idle=std::time::Duration::from_secs(test_seconds.unwrap_or((*memory_minutes).clamp(15,240) as u64*60));
            if *memory_enabled&&!*browser_locked&&!*closing{
                // One operation per scan bounds protocol work and close bursts.
                if let Some(id)=s.vkladki.iter().filter(|v|!sleep_requests.contains_key(&v.id)&&memory_last_used.get(&v.id).is_some_and(|at|at.elapsed()>=idle))
                    .filter(|v|memory_blocked(v.id,true,s,brauzery,devtools_pending,*force_private).is_none()&&!perevody.contains_key(&v.id)).map(|v|v.id).next(){
                    memory_last_used.insert(id,memory_now);let _=tx.send(Sobytie::Komanda(Komanda::MemoryAuto{id}));
                }
            }
        }
        if s.vkladki.is_empty() && !*closing && !*browser_locked {
            let id = s.dobavit("about:blank");
            let private=*force_private;s.vkladki.iter_mut().find(|v|v.id==id).unwrap().privatnaya=private;
            if let Some(browser)=sozdat_brauzer(&k, id, "about:blank", &tx, private)?{brauzery.insert(id,browser);}
            fokus = true;
            izmeneno = true;
        }
        if !s.vkladki.is_empty() && !s.vkladki.iter().any(|v| v.id == s.aktivnaya) {
            s.aktivnaya = s.vkladki[0].id;
            fokus = true;
        }
        if fokus_klaviatury {
            if let Some(attempt)=sleep_requests.get_mut(&s.aktivnaya){
                if attempt.stage==MemoryStage::Closing{attempt.wake=true;}
                else{memory_refused(s.aktivnaya,"Вкладка снова используется",s,tx,sleep_requests,memory_registrations,brauzery);izmeneno=true;}
            }
        }
        if fokus && !*browser_locked {
            // Восстановленные страницы не создаются до явного выбора. Закрытие
            // соседней вкладки тоже не должно будить спящую автоматически.
            if fokus_klaviatury && !brauzery.contains_key(&s.aktivnaya) {
                if let Some(v) = s.vkladki.iter_mut().find(|v| v.id == s.aktivnaya) {
                    if let Some(browser)=sozdat_brauzer(k, v.id, &v.adres, tx, v.privatnaya)?{brauzery.insert(v.id,browser);}
                    v.otlozhena = false;v.zagruzka=v.adres!="about:blank";
                }
            }
            pokazat(&s, &brauzery);
        }
        // Restore the selected native Widget before focusing it. It may have
        // been hidden while its public Browser HWND remained alive.
        #[cfg(target_os = "windows")]
        sync_native_visibility_for(native_visibility, k, s, brauzery,
            !*browser_locked && !*closing);
        if fokus_klaviatury && !*browser_locked
            && (fokus_posle_devtools||s.vkladki
                .iter()
                .any(|v| v.id == s.aktivnaya && v.adres != "about:blank"))
        {
            if let Some(h) = brauzery.get(&s.aktivnaya).filter(|_|!s.vkladki.iter().any(|v|v.id==s.aktivnaya&&v.memory_pending)).and_then(|b| b.host()) {
                h.set_focus(1);
            }
        }
        for tab in &mut s.vkladki{
            let identity=brauzery.get(&tab.id).map(|b|b.identifier()).unwrap_or(0);
            if tab.browser_id!=identity{tab.browser_id=identity;izmeneno=true;}
            if identity==0&&tab.route_error.is_some()&&!tab.route_pending&&tab.zagruzka{
                tab.zagruzka=false;izmeneno=true;
            }
        }
        let novyj = k.razmer();
        let pending_ids:Vec<_>=devtools_pending.keys().copied().collect();
        for id in pending_ids{
            let elapsed=devtools_pending.get(&id).unwrap().elapsed();
            if !brauzery.contains_key(&id){devtools_pending.remove(&id);continue;}
            if let Some(hwnd)=crate::devtools::okno(id){
                if sistema::vstroit(hwnd,k.okno as u64,Mesto{x:0,y:0,w:320,h:320}).is_ok(){devtools.insert(id,hwnd);devtools_pending.remove(&id);layout_dirty=true;continue;}
            }
            if elapsed.as_secs()>=8{
                devtools_pending.remove(&id);
                eprintln!("cef: DevTools вкладки {id}: native-окно не готово");
                if let Some(h)=brauzery.get(&id).and_then(|b|b.host()){h.close_dev_tools();}
                let _=tx.send(Sobytie::Sluzhebnoe(serde_json::json!({"tip":"devtools-error","id":id,"error":"Не удалось встроить инструменты разработчика. Повторите F12."})));
            }
        }
        devtools.retain(|id,_| brauzery.contains_key(id));
        let now=std::time::Instant::now();
        let due:Vec<u64>=perevod_due.iter().filter(|(_,at)|**at<=now).map(|(id,_)|*id).collect();
        for id in due{if let Some(v)=s.vkladki.iter().find(|v|v.id==id){if v.zagruzka{continue;}if avto_dlya(perevod_avto,id,&v.adres){let _=tx.send(Sobytie::Komanda(Komanda::Perevesti{id}));}}perevod_due.remove(&id);}
        perevod_avto.retain(|id,_|s.vkladki.iter().any(|v|v.id==*id));perevod_ui.retain(|id,_|brauzery.contains_key(id));perevod_due.retain(|id,_|brauzery.contains_key(id));
        perevody.retain(|id,_|{if brauzery.contains_key(id){true}else{let _=tx.send(Sobytie::Sluzhebnoe(serde_json::json!({"tip":"perevod-otmena","id":id})));false}});
        let has_tools=devtools.contains_key(&s.aktivnaya);
        if has_tools {if let Some((w,h))=novyj {let panel=devtools_pixels.unwrap_or(w * *devtools_procent / 100);if let Some(p)=crate::razdelitel::shag(k.okno as u64,w,h,panel){if Some(p)!=*devtools_pixels{*devtools_pixels=Some(p);*devtools_procent=p.saturating_mul(100)/w.max(1);layout_dirty=true;}}}}
        if novyj!=*razmer||layout_dirty||izmeneno||fokus {
        if let Some((w,h))=novyj {
            let panel=if has_tools { devtools_pixels.unwrap_or(w * *devtools_procent / 100).max(280).min(w.saturating_sub(280)) } else {0};
            for (id,b) in brauzery.iter() {
                let width=if *id==s.aktivnaya && has_tools {w.saturating_sub(panel+6)} else {w};
                let _=sistema::peremestit(handle(b),Mesto{x:0,y:0,w:width,h});
            }
            for (id,okno) in devtools.iter() {
                let active=!*browser_locked&&*id==s.aktivnaya;
                if active { let _=sistema::peremestit(*okno,Mesto{x:w.saturating_sub(panel) as i32,y:0,w:panel,h}); }
                let _=sistema::pokazat(*okno,active);
            }
        }
        }
        *razmer=novyj;
        let tools_ui=serde_json::json!({"tip":"devtools","id":s.aktivnaya,"open":has_tools,"procent":*devtools_procent}).to_string();
        if *ui_last!=tools_ui {let mut data:serde_json::Value=serde_json::from_str(&tools_ui)?;data["label"]=serde_json::json!(label);println!("NB-UI {data}");std::io::stdout().flush()?;*ui_last=tools_ui; }
        if izmeneno {
            if label.is_empty(){println!("NB-VKLADKI {}", serde_json::to_string(&s)?);}else{println!("NB-UI {}",serde_json::json!({"tip":"okno-session","label":label,"sessiya":s}));}
            std::io::stdout().flush()?;
        }
        #[cfg(target_os = "windows")]
        if handled_events == 128 {
            // A tick is deliberately bounded. A burst must continue even when
            // the relay has already finished sending and no fresh event comes.
            request_wake();
        }
        #[cfg(not(target_os = "windows"))]
        let _ = handled_events;
        // Visibility is synchronized unconditionally before focus above,
        // including idle maintenance. Do not scan every live tab twice.
        Ok(())
    }
}

fn origin(url:&str)->Option<String>{reqwest::Url::parse(url).ok().filter(|u|matches!(u.scheme(),"http"|"https")).map(|u|u.origin().ascii_serialization())}
fn avto_dlya(modes:&HashMap<u64,String>,id:u64,url:&str)->bool{origin(url).is_some_and(|value|modes.get(&id)==Some(&value))}
fn bez_fragmenta(url:&str)->&str{url.split('#').next().unwrap_or(url)}
