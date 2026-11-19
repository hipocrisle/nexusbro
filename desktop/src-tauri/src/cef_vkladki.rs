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
    Zagolovok(u64, String),
    Adres(u64, String),
    Zagruzka(u64, bool, bool, bool),
    Zakryta(u64),
    Ikonka(u64, String, String),
    AdresnyiVvod,
    #[cfg(target_os = "windows")]
    ZakrytOkno(u64),
}

wrap_display_handler! {
    struct VidStranicy { id: u64, tx: Sender<Sobytie>, privatnaya:bool }
    impl DisplayHandler {
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
        fn on_title_change(&self, _browser: Option<&mut Browser>, title: Option<&CefString>) {
            if let Some(t) = title { let _ = self.tx.send(Sobytie::Zagolovok(self.id, t.to_string())); }
        }
        // ⛔ Без `mut` в объявлении: грамматика `wrap_*!` принимает только
        // шаблон `имя: тип`, иначе «no rules expected». Внутри берём `&mut`.
        fn on_address_change(&self, _browser: Option<&mut Browser>, frame: Option<&mut Frame>, url: Option<&CefString>) {
            if frame.as_ref().map(|f| f.is_main() != 0).unwrap_or(false) {
                if let Some(u) = url {
                    let u = u.to_string();
                    let _ = self.tx.send(Sobytie::Adres(self.id, u.clone()));

                }
            }
        }
    }
}

wrap_load_handler! {
    struct ZagruzkaStranicy { id: u64, tx: Sender<Sobytie>, privatnaya:bool }
    impl LoadHandler {
        fn on_load_end(&self, browser: Option<&mut Browser>, frame: Option<&mut Frame>, _http_status_code: i32) {
            if frame.as_ref().is_some_and(|f|f.is_main()!=0){if let Some(b)=browser.as_ref(){crate::password_cef::install(b,self.id,self.privatnaya,self.tx.clone());}}
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
            if !self.privatnaya && frame.map(|f| f.is_main() != 0).unwrap_or(false) {
                eprintln!("cef: вкладка {} load_error {:?}: {}", self.id, error_code, error_text.map(|t| t.to_string()).unwrap_or_default());
            }
        }
        fn on_loading_state_change(&self, _browser: Option<&mut Browser>, is_loading: i32, can_go_back: i32, can_go_forward: i32) {
            let _ = self.tx.send(Sobytie::Zagruzka(self.id, is_loading != 0, can_go_back != 0, can_go_forward != 0));
        }
    }
}

wrap_life_span_handler! {
    struct ZhiznStranicy { id: u64, tx: Sender<Sobytie>, privatnaya:bool }
    impl LifeSpanHandler {
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
        fn do_close(&self, _browser: Option<&mut Browser>) -> i32 {
            #[cfg(target_os = "windows")]
            {
                // CEF по умолчанию посылает WM_CLOSE верхнему родителю — Tauri!
                // Разрушаем только собственный HWND вкладки на CEF UI-потоке,
                // после возврата из callback, чтобы не войти в teardown рекурсивно.
                let _ = self.tx.send(Sobytie::ZakrytOkno(self.id));
                return 1;
            }
            #[cfg(not(target_os = "windows"))]
            { 0 }
        }
        fn on_before_close(&self, browser: Option<&mut Browser>) {
            if let Some(browser)=browser{crate::cef_interfeys::cancel_context(browser.identifier());}
            crate::password_cef::closed(self.id);
            let _ = self.tx.send(Sobytie::Zakryta(self.id));
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
    struct SboyStranicy { id: u64, privatnaya:bool, tx:Sender<Sobytie> }
    impl RequestHandler {
        fn on_before_browse(&self,browser:Option<&mut Browser>,frame:Option<&mut Frame>,_request:Option<&mut Request>,_user_gesture:i32,_is_redirect:i32)->i32{
            if frame.as_ref().is_some_and(|f|f.is_main()!=0){if let Some(browser)=browser{crate::cef_interfeys::cancel_context(browser.identifier());}}
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
    struct KlientVkladki { id: u64, tx: Sender<Sobytie>, privatnaya:bool }
    impl Client {
        fn download_handler(&self)->Option<DownloadHandler>{Some(crate::zagruzki::handler(self.id,self.tx.clone(),self.privatnaya))}
        fn find_handler(&self) -> Option<FindHandler> { Some(crate::poisk::handler(self.id, self.tx.clone())) }
        fn keyboard_handler(&self) -> Option<KeyboardHandler> { Some(crate::cef_interfeys::klaviatura(self.id, self.tx.clone())) }
        fn context_menu_handler(&self) -> Option<ContextMenuHandler> { Some(crate::cef_interfeys::menyu(self.id, self.tx.clone(), self.privatnaya)) }
        fn focus_handler(&self) -> Option<FocusHandler> { Some(FokusStranicy::new()) }
        fn request_handler(&self) -> Option<RequestHandler> { Some(SboyStranicy::new(self.id, self.privatnaya, self.tx.clone())) }
        fn display_handler(&self) -> Option<DisplayHandler> { Some(VidStranicy::new(self.id, self.tx.clone(), self.privatnaya)) }
        fn load_handler(&self) -> Option<LoadHandler> { Some(ZagruzkaStranicy::new(self.id, self.tx.clone(), self.privatnaya)) }
        fn life_span_handler(&self) -> Option<LifeSpanHandler> { Some(ZhiznStranicy::new(self.id, self.tx.clone(), self.privatnaya)) }
    }
}

// Идентификаторы сохраняются до закрытия всей приватной группы: загрузка
// может продолжаться после закрытия исходной вкладки, но не переживает группу.
thread_local! {static PRIVATE_IDS:std::cell::RefCell<std::collections::HashSet<u64>>=Default::default();}

struct Konteyner {
    okno: u32,
    private_context: std::rc::Rc<std::cell::RefCell<Option<RequestContext>>>,
    #[cfg(target_os = "linux")]
    svyaz: x11rb::rust_connection::RustConnection,
}

impl Konteyner {
    fn sozdat(roditel: u64, m: Mesto) -> anyhow::Result<Self> {
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
            Ok(Self { okno, svyaz, private_context:std::rc::Rc::new(std::cell::RefCell::new(None)) })
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
                private_context:std::rc::Rc::new(std::cell::RefCell::new(None)),
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
) -> anyhow::Result<Browser> {
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
    let mut context=if privatnaya {
        let mut slot=k.private_context.borrow_mut();
        if slot.is_none(){PRIVATE_IDS.with(|ids|ids.borrow_mut().clear());*slot=request_context_create_context(Some(&RequestContextSettings::default()),None);}
        PRIVATE_IDS.with(|ids|{ids.borrow_mut().insert(id);});
        Some(slot.clone().ok_or_else(||anyhow::anyhow!("Не создан приватный контекст"))?)
    }else{None};
    let b = browser_host_create_browser_sync(
        Some(&info),
        Some(&mut KlientVkladki::new(id, tx.clone(), privatnaya)),
        Some(&CefString::from(adres)),
        Some(&BrowserSettings::default()),
        None,
        context.as_mut(),
    )
    .ok_or_else(|| anyhow::anyhow!("не создалась обычная вкладка"))?;
    if let Some(h) = b.host() {
        h.set_zoom_level(
            (crate::nastrojki::zagruzit().masshtab_stranic as f64 / 100.0).ln() / 1.2_f64.ln(),
        );
    }
    Ok(b)
}

fn pokazat(s: &Sessiya, b: &HashMap<u64, Browser>) {
    for (id, b) in b {
        let _ = sistema::pokazat(handle(b), *id == s.aktivnaya);
    }
}

pub fn rabotat(
    roditel: u64,
    mesto: Mesto,
    profil: &std::path::Path,
    start: Option<&str>,
) -> anyhow::Result<()> {
    let k = Konteyner::sozdat(roditel, mesto)?;
    let initially_locked=std::env::args().any(|arg|arg=="--nb-browser-locked");
    let initial_lock_generation=std::env::args().find_map(|arg|arg.strip_prefix("--nb-lock-generation=").and_then(|value|value.parse::<u64>().ok())).unwrap_or(0);
    let mut s = start
        .map(Sessiya::novaya)
        .unwrap_or_else(|| Sessiya::zagruzit(profil));
    if crate::privatnost::process_privatnyj(){for v in &mut s.vkladki{v.privatnaya=true;}}
    let (tx, rx) = channel();
    #[cfg(target_os = "windows")]
    let (rx, event_relay) = crate::cef_events::relay(&tx, rx, Sobytie::Komanda(Komanda::ObnovitInterfeys), request_wake)?;
    let mut brauzery = HashMap::new();
    if initially_locked{for tab in &mut s.vkladki{tab.otlozhena=true;}let _=sistema::pokazat(k.okno,false);}
    for v in &s.vkladki {
        if !initially_locked && !v.otlozhena {
            brauzery.insert(v.id, sozdat_brauzer(&k, v.id, &v.adres, &tx, v.privatnaya)?);
        }
    }
    pokazat(&s, &brauzery);
    let tx_stdin = tx.clone();
    std::thread::spawn(move || {
        for l in std::io::stdin().lock().lines() {
            let Ok(l) = l else { break };
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
        brauzery,
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
            std::thread::sleep(std::time::Duration::from_millis(8));
        }
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

struct Rabota {
    k: Konteyner,
    s: Sessiya,
    tx: Sender<Sobytie>,
    rx: Receiver<Sobytie>,
    #[cfg(target_os = "windows")]
    _event_relay: crate::cef_events::Relay<Sobytie>,
    brauzery: HashMap<u64, Browser>,
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
            for browser in self.brauzery.values(){crate::cef_interfeys::cancel_context(browser.identifier());}
            for browser in self.brauzery.values(){
                if let Some(host)=browser.host(){host.set_focus(0);host.close_dev_tools();}
                let _=sistema::pokazat(handle(browser),false);
            }
            for hwnd in self.devtools.values(){let _=sistema::pokazat(*hwnd,false);}
            self.devtools_pending.clear();self.devtools.clear();
            let _=sistema::pokazat(self.k.okno,false);
        }
    }
    fn shag(&mut self) -> anyhow::Result<()> {
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
            && self.k.private_context.borrow().is_some(){
            crate::zagruzki::zakryt_privatnye();self.k.private_context.borrow_mut().take();PRIVATE_IDS.with(|ids|ids.borrow_mut().clear());
            let _=self.tx.send(Sobytie::Sluzhebnoe(serde_json::json!({"tip":"private-ended"})));
        }
        let Self {
            k,
            s,
            tx,
            rx,
            brauzery,
            razmer,
            devtools, devtools_pending, devtools_procent, devtools_pixels, ui_last, perevody, perevod_avto, perevod_due, perevod_ui, label, extras, closed_labels, closing, browser_locked, browser_lock_generation, force_private, ..
        } = self;
        let mut izmeneno = false;
        let mut layout_dirty=false;
        let mut fokus = false;
        let mut fokus_klaviatury = false;
        let mut fokus_posle_devtools=false;
        let mut handled_events = 0;
        for sobytie in rx.try_iter().take(128) {
            handled_events += 1;
            // A lock covers queued/in-flight UI actions too. Lifecycle events
            // still complete, while no key, popup or stale command can reopen a page.
            if *browser_locked && matches!(&sobytie,Sobytie::Komanda(command) if !matches!(command,
                Komanda::BrowserLocked{..}|Komanda::PasswordsEnabled{..}|Komanda::OknoKomanda{..}|
                Komanda::OknoZakryt{..}|Komanda::GlavnoeSkryto|Komanda::GlavnoePokazat)) {continue;}

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
                        brauzery.insert(id, sozdat_brauzer(k, id, &v.adres, tx, v.privatnaya)?);
                        v.otlozhena = false;
                        razbuzhen = Some(id);
                    }
                }
            }
            // Поиск, перевод и отчёты защиты не меняют сохранённую сессию.
            // Пробуждение отложенной страницы при этом обязательно публикуем.
            let session_dirty = razbuzhen.is_some() || !matches!(&sobytie,
                Sobytie::Sluzhebnoe(_) | Sobytie::PoiskVybratPervoe(..) | Sobytie::DevtoolsGotovy(..) |
                Sobytie::DevtoolsZakryty(_) | Sobytie::AdresnyiVvod |
                Sobytie::Komanda(Komanda::Nayti{..} | Komanda::PoiskZakryt{..} |
                    Komanda::Perevesti{..} | Komanda::PerevodOriginal{..} |
                    Komanda::PerevodOtvet{..} | Komanda::PerevodAvto{..} | Komanda::FonKonteynera{..} | Komanda::OchistitDannye{..} | Komanda::DannyeStatistika{..} | Komanda::PasswordForm{..} | Komanda::PasswordsEnabled{..} | Komanda::BrowserLocked{..} | Komanda::PageContextChoose{..} | Komanda::Instrumenty{..} |
                    Komanda::Issledovat{..} | Komanda::InstrumentyRazmer{..} |
                    Komanda::ZagruzkaUpravlenie{..} | Komanda::Masshtab{..}));
            let private_command=matches!(&sobytie,Sobytie::Komanda(Komanda::NovayaPrivatnaya{..}));
            match sobytie {
                Sobytie::Komanda(Komanda::OknoNovoe{label:new_label,parent,x,y,w,h,privatnaya,sessiya})=>{
                    if !label.is_empty()||extras.contains_key(&new_label){continue;}
                    if closed_labels.contains(&new_label){println!("NB-UI {}",serde_json::json!({"tip":"okno-closed","label":new_label}));std::io::stdout().flush()?;continue;}
                    let mut child_k=match Konteyner::sozdat(parent,Mesto{x,y,w,h}){Ok(k)=>k,Err(e)=>{eprintln!("cef: новое окно: {e}");println!("NB-UI {}",serde_json::json!({"tip":"okno-closed","label":new_label}));std::io::stdout().flush()?;continue;}};
                    child_k.private_context=k.private_context.clone();
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
                    for (i,v) in child_s.vkladki.iter_mut().enumerate(){v.id=base+i as u64;v.privatnaya|=privatnaya||crate::privatnost::process_privatnyj();v.zagruzka=false;v.otlozhena=restoring&&i!=active;}
                    child_s.aktivnaya=child_s.vkladki[active].id;child_s.sleduyushchaya=base+child_s.vkladki.len() as u64;
                    let mut child_b=HashMap::new();
                    let tab=&child_s.vkladki[active];
                    match sozdat_brauzer(&child_k,tab.id,&tab.adres,&child_tx,tab.privatnaya){Ok(b)=>{child_b.insert(tab.id,b);},Err(e)=>{eprintln!("cef: страница нового окна: {e}");println!("NB-UI {}",serde_json::json!({"tip":"okno-closed","label":new_label}));std::io::stdout().flush()?;continue;}}
                    let hwnd=child_k.okno;let size=child_k.razmer();
                    let session=child_s.clone();
                    extras.insert(new_label.clone(),Rabota{k:child_k,s:child_s,tx:child_tx,rx:child_rx,#[cfg(target_os = "windows")] _event_relay:child_event_relay,brauzery:child_b,razmer:size,devtools:HashMap::new(),devtools_pending:HashMap::new(),devtools_procent:40,devtools_pixels:None,ui_last:String::new(),perevody:HashMap::new(),perevod_avto:HashMap::new(),perevod_due:HashMap::new(),perevod_ui:HashMap::new(),label:new_label.clone(),extras:HashMap::new(),closed_labels:Default::default(),closing:false,browser_locked:*browser_locked,browser_lock_generation:*browser_lock_generation,force_private:privatnaya||crate::privatnost::process_privatnyj()});
                    println!("NB-UI {}",serde_json::json!({"tip":"okno-ready","label":new_label,"hwnd":hwnd,"sessiya":session}));std::io::stdout().flush()?;
                    continue;
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
                            if let Some(h)=brauzery.get(&tab.id).and_then(|b|b.host()){h.close_browser(1);}else{let _=tx.send(Sobytie::Zakryta(tab.id));}
                        }
                    }
                }
                Sobytie::Komanda(Komanda::PageContextChoose{token,command_id})=>{let ids=brauzery.values().map(|b|b.identifier()).collect::<Vec<_>>();crate::cef_interfeys::choose_context(&token,command_id,&ids);}
                Sobytie::Komanda(Komanda::OpenLink{opener_id,adres,privatnaya,background})=>{
                    let Some(opener)=s.vkladki.iter().find(|v|v.id==opener_id&&brauzery.contains_key(&opener_id))else{continue;};
                    let private=privatnaya||opener.privatnaya||*force_private;
                    let active=s.aktivnaya;
                    let mut next=s.clone();let id=next.dobavit(&adres);
                    next.vkladki.iter_mut().find(|v|v.id==id).unwrap().privatnaya=private;
                    if background{next.aktivnaya=active;}
                    match sozdat_brauzer(k,id,&adres,tx,private){
                        Ok(browser)=>{brauzery.insert(id,browser);*s=next;fokus=true;if !background{fokus_klaviatury=true;}}
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
                            brauzery.insert(id, b);
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
                    let target=s.vkladki.iter().find(|v|v.privatnaya==privatnaya&&brauzery.contains_key(&v.id)).map(|v|v.id);
                    let target=if let Some(id)=target{id}else{let id=s.dobavit("about:blank");s.vkladki.iter_mut().find(|v|v.id==id).unwrap().privatnaya=privatnaya;brauzery.insert(id,sozdat_brauzer(k,id,"about:blank",tx,privatnaya)?);fokus=true;id};
                    if let Some(h)=brauzery.get(&target).and_then(|b|b.host()){h.start_download(Some(&CefString::from(adres.as_str())));}
                }
                Sobytie::Komanda(Komanda::VosstanovitZakrytuyu) => {
                    if let Some(old)=s.zakrytye.pop(){
                        let id=s.dobavit(&old.adres);let current=s.vkladki.iter_mut().find(|v|v.id==id).unwrap();current.zagolovok=old.zagolovok;current.zakreplena=old.zakreplena;
                        brauzery.insert(id,sozdat_brauzer(k,id,&old.adres,tx,false)?);fokus=true;fokus_klaviatury=true;
                    }
                }
                Sobytie::Komanda(Komanda::Vybrat { id }) => {
                    if s.vkladki.iter().any(|v| v.id == id) {
                        s.aktivnaya = id;
                        fokus = true;
                        fokus_klaviatury = true;
                    }
                }
                Sobytie::Komanda(Komanda::Zakryt { id }) => {
                    fokus_klaviatury |= s.aktivnaya == id;
                    if let Some(h) = brauzery.get(&id).and_then(|b| b.host()) {
                        h.close_browser(0);
                    } else {
                        let _ = tx.send(Sobytie::Zakryta(id));
                    }
                }
                Sobytie::Komanda(Komanda::ZakrytDrugie { id, sprava }) => {
                    if let Some(poziciya) = s.vkladki.iter().position(|v| v.id == id) {
                        for (i, v) in s.vkladki.iter().enumerate() {
                            if v.id != id && !v.zakreplena && (!sprava || i > poziciya) {
                                if let Some(h) = brauzery.get(&v.id).and_then(|b| b.host()) {
                                    h.close_browser(0);
                                } else {
                                    let _ = tx.send(Sobytie::Zakryta(v.id));
                                }
                            }
                        }
                    }
                }
                #[cfg(target_os = "windows")]
                Sobytie::ZakrytOkno(id) => {
                    if let Some(b) = brauzery.get(&id) {
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
                Sobytie::Zakryta(id) => {
                    crate::poisk::ochistit(id);
                    if let Some(tab)=s.vkladki.iter().find(|v|v.id==id&&!v.privatnaya&&v.adres!="about:blank").cloned(){s.zakrytye.push(tab);if s.zakrytye.len()>30{s.zakrytye.remove(0);}}
                    if s.aktivnaya == id {
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
                        let private=v.privatnaya;
                        let n = next.dobavit(&adres);next.vkladki.iter_mut().find(|v|v.id==n).unwrap().privatnaya=private;
                        match sozdat_brauzer(k, n, &adres, tx, private) {
                            Ok(b) => {
                                brauzery.insert(n, b);
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
                        for browser in brauzery.values(){crate::cef_interfeys::cancel_context(browser.identifier());}
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
                    let ok=allowed&&brauzery.get(&id).is_some_and(|b|crate::password_cef::request(b,id,token.clone(),origin,payload,tx.clone()));
                    if !ok{let _=tx.send(Sobytie::Sluzhebnoe(serde_json::json!({"tip":"credential-result","token":token,"error":"Форма недоступна"})));}
                }
                Sobytie::Komanda(Komanda::DannyeStatistika{token})=>{
                    let mut contexts=Vec::new();if let Some(c)=request_context_get_global_context(){contexts.push(c);}if let Some(c)=k.private_context.borrow().as_ref(){contexts.push(c.clone());}
                    crate::dannye_statistika::izmerit(contexts,token,label.clone(),tx.clone());
                }
                Sobytie::Komanda(Komanda::OchistitDannye{token,cookies,cache})=>{
                    let mut contexts=Vec::new();if let Some(c)=request_context_get_global_context(){contexts.push(c);}if let Some(c)=k.private_context.borrow().as_ref(){contexts.push(c.clone());}
                    crate::dannye_saytov::ochistit(contexts,token,label.clone(),cookies,cache,tx.clone());
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
                        if k.private_context.borrow().is_none()||!source.is_some_and(|id|PRIVATE_IDS.with(|ids|ids.borrow().contains(&id))){continue;}
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
                Sobytie::Zagolovok(id, t) => {
                    if let Some(v) = s.vkladki.iter_mut().find(|v| v.id == id) {
                        v.zagolovok = t;
                    }
                }
                Sobytie::Adres(id, u) => {
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
                Sobytie::Zagruzka(id, l, n, vp) => {
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
        if s.vkladki.is_empty() && !*closing && !*browser_locked {
            let id = s.dobavit("about:blank");
            let private=*force_private;s.vkladki.iter_mut().find(|v|v.id==id).unwrap().privatnaya=private;
            brauzery.insert(id, sozdat_brauzer(&k, id, "about:blank", &tx, private)?);
            fokus = true;
            izmeneno = true;
        }
        if !s.vkladki.is_empty() && !s.vkladki.iter().any(|v| v.id == s.aktivnaya) {
            s.aktivnaya = s.vkladki[0].id;
            fokus = true;
        }
        if fokus && !*browser_locked {
            // Восстановленные страницы не создаются до явного выбора. Закрытие
            // соседней вкладки тоже не должно будить спящую автоматически.
            if fokus_klaviatury && !brauzery.contains_key(&s.aktivnaya) {
                if let Some(v) = s.vkladki.iter_mut().find(|v| v.id == s.aktivnaya) {
                    brauzery.insert(v.id, sozdat_brauzer(k, v.id, &v.adres, tx, v.privatnaya)?);
                    v.otlozhena = false;
                }
            }
            pokazat(&s, &brauzery);
        }
        if fokus_klaviatury && !*browser_locked
            && (fokus_posle_devtools||s.vkladki
                .iter()
                .any(|v| v.id == s.aktivnaya && v.adres != "about:blank"))
        {
            if let Some(h) = brauzery.get(&s.aktivnaya).and_then(|b| b.host()) {
                h.set_focus(1);
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
        Ok(())
    }
}

fn origin(url:&str)->Option<String>{reqwest::Url::parse(url).ok().filter(|u|matches!(u.scheme(),"http"|"https")).map(|u|u.origin().ascii_serialization())}
fn avto_dlya(modes:&HashMap<u64,String>,id:u64,url:&str)->bool{origin(url).is_some_and(|value|modes.get(&id)==Some(&value))}
fn bez_fragmenta(url:&str)->&str{url.split('#').next().unwrap_or(url)}
