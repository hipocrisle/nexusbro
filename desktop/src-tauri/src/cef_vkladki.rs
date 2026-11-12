use crate::okna::{sistema, Mesto};
use crate::sessiya::{Komanda, Sessiya};
use cef::*;
use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::sync::mpsc::{channel, Sender};

enum Sobytie {
    Komanda(Komanda),
    Zagolovok(u64, String),
    Adres(u64, String),
    Zagruzka(u64, bool, bool, bool),
    Zakryta(u64),
}

wrap_display_handler! {
    struct VidStranicy { id: u64, tx: Sender<Sobytie> }
    impl DisplayHandler {
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
        fn on_before_close(&self, _browser: Option<&mut Browser>) {
            let _ = self.tx.send(Sobytie::Zakryta(self.id));
        }
    }
}

wrap_client! {
    struct KlientVkladki { id: u64, tx: Sender<Sobytie> }
    impl Client {
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
    let info = WindowInfo::default().set_as_child(
        roditel,
        &Rect {
            x: 0,
            y: 0,
            width: w as i32,
            height: h as i32,
        },
    );
    browser_host_create_browser_sync(
        Some(&info),
        Some(&mut KlientVkladki::new(id, tx.clone())),
        Some(&CefString::from(adres)),
        Some(&BrowserSettings::default()),
        None,
        None,
    )
    .ok_or_else(|| anyhow::anyhow!("не создалась обычная вкладка"))
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
    let mut s = start
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
    println!("NB-OKNO {}", k.okno);
    println!("NB-VKLADKI {}", serde_json::to_string(&s)?);
    std::io::stdout().flush()?;
    let mut razmer = k.razmer();
    loop {
        do_message_loop_work();
        let mut izmeneno = false;
        let mut fokus = false;
        for sobytie in rx.try_iter().take(128) {
            match sobytie {
                Sobytie::Komanda(Komanda::Novaya { adres }) => {
                    let mut novaya = s.clone();
                    let id = novaya.dobavit(&adres);
                    match sozdat_brauzer(&k, id, &adres, &tx) {
                        Ok(b) => {
                            brauzery.insert(id, b);
                            s = novaya;
                            fokus = true;
                        }
                        Err(e) => eprintln!("cef: {e}"),
                    }
                }
                Sobytie::Komanda(Komanda::Vybrat { id }) => {
                    if brauzery.contains_key(&id) {
                        s.aktivnaya = id;
                        fokus = true;
                    }
                }
                Sobytie::Komanda(Komanda::Zakryt { id }) => {
                    if let Some(h) = brauzery.get(&id).and_then(|b| b.host()) {
                        h.close_browser(0);
                    }
                }
                Sobytie::Zakryta(id) => {
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
        let novyj = k.razmer();
        if novyj != razmer {
            if let Some((w, h)) = novyj {
                for b in brauzery.values() {
                    let _ = sistema::peremestit(handle(b), Mesto { x: 0, y: 0, w, h });
                }
            }
            razmer = novyj;
        }
        if izmeneno {
            println!("NB-VKLADKI {}", serde_json::to_string(&s)?);
            std::io::stdout().flush()?;
        }
        std::thread::sleep(std::time::Duration::from_millis(8));
    }
}
