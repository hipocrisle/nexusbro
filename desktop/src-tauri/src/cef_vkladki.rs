use crate::okna::{sistema, Mesto};
use crate::sessiya::{Komanda, Sessiya};
use cef::*;
use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::sync::mpsc::{channel, Receiver, Sender};

pub(crate) enum Sobytie {
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
    struct VidStranicy { id: u64, tx: Sender<Sobytie> }
    impl DisplayHandler {
        fn on_favicon_urlchange(&self, browser: Option<&mut Browser>, icon_urls: Option<&mut CefStringList>) {
            crate::cef_interfeys::ikonka(self.id, self.tx.clone(), browser, icon_urls);
        }
        fn on_title_change(&self, _browser: Option<&mut Browser>, title: Option<&CefString>) {
            if let Some(t) = title { let _ = self.tx.send(Sobytie::Zagolovok(self.id, t.to_string())); }
        }
        fn on_address_change(&self, _browser: Option<&mut Browser>, frame: Option<&mut Frame>, url: Option<&CefString>) {
            if frame.map(|f| f.is_main() != 0).unwrap_or(false) {
                if let Some(u) = url { let _ = self.tx.send(Sobytie::Adres(self.id, u.to_string())); }
            }
        }
    }
}

wrap_load_handler! {
    struct ZagruzkaStranicy { id: u64, tx: Sender<Sobytie> }
    impl LoadHandler {
        fn on_load_error(&self, _browser: Option<&mut Browser>, frame: Option<&mut Frame>, error_code: Errorcode,
            error_text: Option<&CefString>, _failed_url: Option<&CefString>) {
            if frame.map(|f| f.is_main() != 0).unwrap_or(false) {
                eprintln!("cef: вкладка {} load_error {:?}: {}", self.id, error_code, error_text.map(|t| t.to_string()).unwrap_or_default());
            }
        }
        fn on_loading_state_change(&self, _browser: Option<&mut Browser>, is_loading: i32, can_go_back: i32, can_go_forward: i32) {
            let _ = self.tx.send(Sobytie::Zagruzka(self.id, is_loading != 0, can_go_back != 0, can_go_forward != 0));
        }
    }
}

wrap_life_span_handler! {
    struct ZhiznStranicy { id: u64, tx: Sender<Sobytie> }
    impl LifeSpanHandler {
        fn on_before_popup(&self, _browser: Option<&mut Browser>, _frame: Option<&mut Frame>, _popup_id: i32,
            target_url: Option<&CefString>, _target_frame_name: Option<&CefString>, _target_disposition: WindowOpenDisposition,
            _user_gesture: i32, _popup_features: Option<&PopupFeatures>, _window_info: Option<&mut WindowInfo>,
            _client: Option<&mut Option<Client>>, _settings: Option<&mut BrowserSettings>,
            _extra_info: Option<&mut Option<DictionaryValue>>, _no_javascript_access: Option<&mut i32>) -> i32 {
            if let Some(u) = target_url {
                let _ = self.tx.send(Sobytie::Komanda(Komanda::Novaya { adres: u.to_string() }));
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
        fn on_before_close(&self, _browser: Option<&mut Browser>) {
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
    struct SboyStranicy { id: u64 }
    impl RequestHandler {
        fn on_render_process_terminated(&self, _browser: Option<&mut Browser>, status: TerminationStatus,
            error_code: i32, error_string: Option<&CefString>) {
            eprintln!("cef: вкладка {} renderer terminated {:?}, code={}: {}", self.id, status, error_code,
                error_string.map(|s| s.to_string()).unwrap_or_default());
        }
    }
}

wrap_client! {
    struct KlientVkladki { id: u64, tx: Sender<Sobytie> }
    impl Client {
        fn keyboard_handler(&self) -> Option<KeyboardHandler> { Some(crate::cef_interfeys::klaviatura(self.id, self.tx.clone())) }
        fn context_menu_handler(&self) -> Option<ContextMenuHandler> { Some(crate::cef_interfeys::menyu(self.id, self.tx.clone())) }
        fn focus_handler(&self) -> Option<FocusHandler> { Some(FokusStranicy::new()) }
        fn request_handler(&self) -> Option<RequestHandler> { Some(SboyStranicy::new(self.id)) }
        fn display_handler(&self) -> Option<DisplayHandler> { Some(VidStranicy::new(self.id, self.tx.clone())) }
        fn load_handler(&self) -> Option<LoadHandler> { Some(ZagruzkaStranicy::new(self.id, self.tx.clone())) }
        fn life_span_handler(&self) -> Option<LifeSpanHandler> { Some(ZhiznStranicy::new(self.id, self.tx.clone())) }
    }
}

struct Konteyner {
    okno: u32,
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
                    &CreateWindowAux::new().colormap(ekran.default_colormap),
                )?
                .check()?;
            Ok(Self { okno, svyaz })
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
            Ok(Self {
                okno: okno.0 as usize as u32,
            })
        }
        #[cfg(not(any(target_os = "windows", target_os = "linux")))]
        {
            let _ = (roditel, m);
            anyhow::bail!("система не поддерживается")
        }
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

fn handle(b: &Browser) -> u32 {
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
    let b = browser_host_create_browser_sync(
        Some(&info),
        Some(&mut KlientVkladki::new(id, tx.clone())),
        Some(&CefString::from(adres)),
        Some(&BrowserSettings::default()),
        None,
        None,
    )
    .ok_or_else(|| anyhow::anyhow!("не создалась обычная вкладка"))?;
    if let Some(h) = b.host() {
        h.set_zoom_level((crate::nastrojki::zagruzit().masshtab_stranic as f64 / 100.0).ln() / 1.2_f64.ln());
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
    let s = start
        .map(Sessiya::novaya)
        .unwrap_or_else(|| Sessiya::zagruzit(profil));
    let (tx, rx) = channel();
    let mut brauzery = HashMap::new();
    for v in &s.vkladki {
        brauzery.insert(v.id, sozdat_brauzer(&k, v.id, &v.adres, &tx)?);
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
        brauzery,
        razmer,
    };
    #[cfg(target_os = "windows")]
    {
        // Настоящая Win32-петля обслуживает WM_PAINT, ввод и compositor.
        // Все браузеры и контейнер остаются на исходном CEF UI-потоке.
        let rabota = std::rc::Rc::new(std::cell::RefCell::new(rabota));
        if post_delayed_task(ThreadId::UI, Some(&mut Obrabotat::new(rabota)), 8) != 1 {
            anyhow::bail!("не удалось запланировать обработку команд CEF");
        }
        run_message_loop();
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

#[cfg(target_os = "windows")]
wrap_task! {
    struct Obrabotat { rabota: std::rc::Rc<std::cell::RefCell<Rabota>> }
    impl Task {
        fn execute(&self) {
            if let Err(e) = self.rabota.borrow_mut().shag() {
                eprintln!("cef: обработка команд: {e}");
                quit_message_loop();
                return;
            }
            let _ = post_delayed_task(ThreadId::UI, Some(&mut Obrabotat::new(self.rabota.clone())), 8);
        }
    }
}

struct Rabota {
    k: Konteyner,
    s: Sessiya,
    tx: Sender<Sobytie>,
    rx: Receiver<Sobytie>,
    brauzery: HashMap<u64, Browser>,
    razmer: Option<(u32, u32)>,
}

impl Rabota {
    fn shag(&mut self) -> anyhow::Result<()> {
        let Self {
            k,
            s,
            tx,
            rx,
            brauzery,
            razmer,
        } = self;
        let mut izmeneno = false;
        let mut fokus = false;
        let mut fokus_klaviatury = false;
        for sobytie in rx.try_iter().take(128) {
            match sobytie {
                Sobytie::Komanda(Komanda::Novaya { adres }) => {
                    let mut novaya = s.clone();
                    let id = novaya.dobavit(&adres);
                    match sozdat_brauzer(&k, id, &adres, &tx) {
                        Ok(b) => {
                            brauzery.insert(id, b);
                            *s = novaya;
                            fokus = true; fokus_klaviatury = true;
                        }
                        Err(e) => eprintln!("cef: {e}"),
                    }
                }
                Sobytie::Komanda(Komanda::Vybrat { id }) => {
                    if brauzery.contains_key(&id) {
                        s.aktivnaya = id;
                        fokus = true; fokus_klaviatury = true;
                    }
                }
                Sobytie::Komanda(Komanda::Zakryt { id }) => {
                    fokus_klaviatury |= s.aktivnaya == id;
                    if let Some(h) = brauzery.get(&id).and_then(|b| b.host()) {
                        h.close_browser(0);
                    }
                }
                Sobytie::Komanda(Komanda::ZakrytDrugie { id, sprava }) => {
                    if let Some(poziciya) = s.vkladki.iter().position(|v| v.id == id) {
                        for (i, v) in s.vkladki.iter().enumerate() {
                            if v.id != id && !v.zakreplena && (!sprava || i > poziciya) {
                                if let Some(h) = brauzery.get(&v.id).and_then(|b| b.host()) {
                                    h.close_browser(0);
                                }
                            }
                        }
                    }
                }
                #[cfg(target_os = "windows")]
                Sobytie::ZakrytOkno(id) => {
                    if let Some(b) = brauzery.get(&id) {
                        use windows::Win32::{Foundation::HWND, System::Threading::{GetCurrentProcessId, GetCurrentThreadId}, UI::WindowsAndMessaging::*};
                        unsafe {
                            let h = HWND(handle(b) as usize as _);
                            let mut pid = 0;
                            let thread = GetWindowThreadProcessId(h, Some(&mut pid));
                            if pid == GetCurrentProcessId() && thread == GetCurrentThreadId()
                                && GetParent(h).ok().map(|p| p.0 as usize as u32) == Some(k.okno) {
                                if let Err(e) = DestroyWindow(h) { eprintln!("cef: закрытие окна вкладки {id}: {e}"); }
                            } else { eprintln!("cef: отказ закрытия чужого окна вкладки {id}"); }
                        }
                    }
                }
                Sobytie::Zakryta(id) => {
                    if s.aktivnaya == id {
                        if let Some(i) = s.vkladki.iter().position(|v| v.id == id) {
                            if let Some(v) = s.vkladki.get(if i > 0 { i - 1 } else { i + 1 }) { s.aktivnaya = v.id; }
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
                            f.load_url(Some(&CefString::from(adres.as_str())));
                        }
                        if let Some(v) = s.vkladki.iter_mut().find(|v| v.id == id) {
                            v.ikonka = None;
                            v.adres = adres;
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
                    if let Some(b) = brauzery.get(&id) { crate::cef_interfeys::instrumenty(b, None, true); }
                }
                Sobytie::Komanda(Komanda::Issledovat { id, x, y }) => {
                    if let Some(b) = brauzery.get(&id) { crate::cef_interfeys::instrumenty(b, Some(Point { x, y }), false); }
                }
                Sobytie::Komanda(Komanda::Iskhodnik { id }) => {
                    if let Some(v) = s.vkladki.iter().find(|v| v.id == id) {
                        let adres = format!("view-source:{}", v.adres.strip_prefix("view-source:").unwrap_or(&v.adres));
                        let mut next = s.clone(); let n = next.dobavit(&adres);
                        match sozdat_brauzer(k, n, &adres, tx) {
                            Ok(b) => { brauzery.insert(n, b); *s = next; fokus = true; fokus_klaviatury = true; }
                            Err(e) => eprintln!("cef: исходный код: {e}"),
                        }
                    }
                }
                Sobytie::Komanda(Komanda::Peremestit { id, pered }) => s.peremestit(id, pered),
                Sobytie::Komanda(Komanda::Sleduyushchaya { nazad }) => {
                    if let Some(i) = s.vkladki.iter().position(|v| v.id == s.aktivnaya) {
                        let n = s.vkladki.len();
                        s.aktivnaya = s.vkladki[(i + if nazad { n - 1 } else { 1 }) % n].id; fokus = true; fokus_klaviatury = true;
                    }
                }
                Sobytie::Komanda(Komanda::Masshtab { procent }) => {
                    let level = (procent.clamp(50, 200) as f64 / 100.0).ln() / 1.2_f64.ln();
                    for b in brauzery.values() { if let Some(h) = b.host() { h.set_zoom_level(level); } }
                }
                Sobytie::AdresnyiVvod => { println!("NB-ADRES"); std::io::stdout().flush()?; }
                Sobytie::Ikonka(id, stranica, ikonka) => {
                    if let Some(v) = s.vkladki.iter_mut().find(|v| v.id == id && v.adres == stranica) { v.ikonka = Some(ikonka); }
                }
                Sobytie::Komanda(Komanda::Obnovit { id }) => {
                    if let Some(b) = brauzery.get(&id) {
                        b.reload();
                    }
                }
                Sobytie::Zagolovok(id, t) => {
                    if let Some(v) = s.vkladki.iter_mut().find(|v| v.id == id) {
                        v.zagolovok = t;
                    }
                }
                Sobytie::Adres(id, u) => {
                    if let Some(v) = s.vkladki.iter_mut().find(|v| v.id == id) {
                        // CEF сообщает underlying URL даже для view-source. Режим
                        // сохраняем для адресной строки и восстановления сессии;
                        // явный Pereyti выше предварительно меняет v.adres.
                        let u = if v.adres.starts_with("view-source:") && !u.starts_with("view-source:") {
                            format!("view-source:{u}")
                        } else { u };
                        if v.adres != u { v.ikonka = None; }
                        v.adres = u;
                    }
                }
                Sobytie::Zagruzka(id, l, n, vp) => {
                    if let Some(v) = s.vkladki.iter_mut().find(|v| v.id == id) {
                        v.zagruzka = l;
                        v.nazad = n;
                        v.vpered = vp;
                    }
                }
            }
            izmeneno = true;
        }
        if s.vkladki.is_empty() {
            let id = s.dobavit("about:blank");
            brauzery.insert(id, sozdat_brauzer(&k, id, "about:blank", &tx)?);
            fokus = true;
            izmeneno = true;
        }
        if !brauzery.contains_key(&s.aktivnaya) {
            s.aktivnaya = s.vkladki[0].id;
            fokus = true;
        }
        if fokus {
            pokazat(&s, &brauzery);
        }
        if fokus_klaviatury && s.vkladki.iter().any(|v| v.id == s.aktivnaya && v.adres != "about:blank") {
            if let Some(h) = brauzery.get(&s.aktivnaya).and_then(|b| b.host()) { h.set_focus(1); }
        }
        let novyj = k.razmer();
        if novyj != *razmer {
            if let Some((w, h)) = novyj {
                for b in brauzery.values() {
                    let _ = sistema::peremestit(handle(b), Mesto { x: 0, y: 0, w, h });
                }
            }
            *razmer = novyj;
        }
        if izmeneno {
            println!("NB-VKLADKI {}", serde_json::to_string(&s)?);
            std::io::stdout().flush()?;
        }
        Ok(())
    }
}
