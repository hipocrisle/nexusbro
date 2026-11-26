//! Пользовательские действия страниц CEF. Сетевые иконки получает сам CEF,
//! поэтому они используют тот же профиль и VPN, а оболочка получает только PNG.
use crate::{cef_vkladki::Sobytie, sessiya::Komanda};
use base64::Engine;
use cef::*;
use std::sync::mpsc::Sender;

#[cfg(target_os = "windows")]
pub(crate) type NativeKeyEvent = cef::sys::MSG;
#[cfg(not(target_os = "windows"))]
pub(crate) type NativeKeyEvent = cef::sys::XEvent;

pub(crate) fn shortcut_code(e:&KeyEvent)->i32{
 #[cfg(target_os="windows")]let scan=(e.native_key_code>>16)&255;
 #[cfg(not(target_os="windows"))]let scan=e.native_key_code-8;
 match scan{16=>81,17=>87,18=>69,19=>82,20=>84,21=>89,22=>85,23=>73,24=>79,25=>80,30=>65,31=>83,32=>68,33=>70,34=>71,35=>72,36=>74,37=>75,38=>76,44=>90,45=>88,46=>67,47=>86,48=>66,49=>78,50=>77,_=>e.windows_key_code}
}

wrap_keyboard_handler! {
    struct Klaviatura { id: u64, tx: Sender<Sobytie> }
    impl KeyboardHandler {
        fn on_pre_key_event(&self, browser: Option<&mut Browser>, event: Option<&KeyEvent>,
            _os_event: Option<&mut NativeKeyEvent>, _is_keyboard_shortcut: Option<&mut i32>) -> i32 {
            let Some(e) = event else { return 0 };
            if e.type_==KeyEventType::CHAR{if let Some(b)=browser.as_ref(){crate::memory_guard::edited(b.identifier());}return 0;}
            if e.type_ != KeyEventType::RAWKEYDOWN && e.type_ != KeyEventType::KEYDOWN { return 0 }
            let ctrl = e.modifiers & (1 << 2) != 0;
            if ctrl && e.modifiers & (1 << 3) != 0 {if let Some(b)=browser.as_ref(){crate::memory_guard::edited(b.identifier());}return 0;}
            let key=shortcut_code(e);
            let shift = e.modifiers & (1 << 1) != 0;
            // Embedded browsers use Alloy: unlike Chrome runtime, Escape is
            // the embedder's responsibility (CefBrowserHost::ExitFullscreen).
            // A real unmodified key acts only on the event's native browser.
            if key==27 && !ctrl && !shift && e.modifiers & (1 << 3)==0 {
                if let Some(host)=browser.as_ref().and_then(|b|b.host()) {
                    if host.is_fullscreen()!=0 {host.exit_fullscreen(1);return 1;}
                }
            }
            if !ctrl&&shift&&key==27{let _=self.tx.send(Sobytie::Sluzhebnoe(serde_json::json!({"tip":"browser-tasks","id":self.id})));return 1;}
            if ctrl&&shift&&key==65{let _=self.tx.send(Sobytie::Sluzhebnoe(serde_json::json!({"tip":"tab-search","id":self.id})));return 1;}
            if ctrl&&shift&&key==76{let _=self.tx.send(Sobytie::Sluzhebnoe(serde_json::json!({"tip":"browser-lock","id":self.id})));return 1;}
            if ctrl&&shift&&key==46{let _=self.tx.send(Sobytie::Sluzhebnoe(serde_json::json!({"tip":"ochistit-dannye","id":self.id})));return 1;}
            if ctrl&&key==78{let _=self.tx.send(Sobytie::Sluzhebnoe(serde_json::json!({"tip":"novoe-okno","id":self.id,"privatnaya":shift})));return 1;}
            if ctrl&&shift&&key==66{let _=self.tx.send(Sobytie::Sluzhebnoe(serde_json::json!({"tip":"panel-zakladok","id":self.id})));return 1;}
            if ctrl && matches!(key,72|74|66) {
                let razdel=match key{72=>"istoriya",74=>"zagruzki",_=>"zakladki"};
                let _=self.tx.send(Sobytie::Sluzhebnoe(serde_json::json!({"tip":"panel","razdel":razdel,"id":self.id})));return 1;
            }
            if key==68&&ctrl{let _=self.tx.send(Sobytie::Sluzhebnoe(serde_json::json!({"tip":"zakladka","id":self.id})));return 1;}
            if key == 70 && ctrl {
                let _ = self.tx.send(Sobytie::Sluzhebnoe(serde_json::json!({"tip":"poisk","id":self.id,"open":true})));
                return 1;
            }
            let cmd = match key {
                116 => Some(if ctrl { Komanda::ObnovitBezKesha { id: self.id } } else { Komanda::Obnovit { id: self.id } }),
                123 => Some(Komanda::Instrumenty { id: self.id }),
                73 if ctrl && shift => Some(Komanda::Instrumenty { id: self.id }),
                83 if ctrl && !shift => Some(Komanda::SohranitStranitsu { id: self.id }),
                85 if ctrl => Some(Komanda::Iskhodnik { id: self.id }),
                87 if ctrl => Some(Komanda::Zakryt { id: self.id }),
                84 if ctrl && shift => Some(Komanda::VosstanovitZakrytuyu),
                84 if ctrl => Some(Komanda::Novaya { adres: crate::nastrojki::zagruzit().novaya_stranica }),
                82 if ctrl => Some(Komanda::Obnovit { id: self.id }),
                9 if ctrl => Some(Komanda::Sleduyushchaya { nazad: shift }),
                76 if ctrl => { let _ = self.tx.send(Sobytie::AdresnyiVvod); return 1; }
                _ => None,
            };
            if let Some(c) = cmd { let _ = self.tx.send(Sobytie::Komanda(c)); return 1 }
            // Conservative: protect any page-directed key, including IME.
            // Browser shortcuts above return before reaching this point.
            if !matches!(key,16|17|18|91|92){if let Some(browser)=browser{crate::memory_guard::edited(browser.identifier());}}
            0
        }
    }
}
pub fn klaviatura(id: u64, tx: Sender<Sobytie>) -> KeyboardHandler {
    Klaviatura::new(id, tx)
}

pub fn instrumenty(b: &Browser, inspect: Option<Point>, toggle: bool, id:u64, tx:Sender<Sobytie>) {
    let Some(h) = b.host() else { return };
    if toggle && h.has_dev_tools() != 0 {
        h.close_dev_tools();
        return;
    }
    // DevTools с CEF 128 работают только в Chrome style, даже у Alloy-страницы.
    let info = WindowInfo {
        runtime_style: RuntimeStyle::CHROME,
        window_name: CefString::from("NexusBro — инструменты разработчика"),
        bounds: Rect {
            x: 80,
            y: 80,
            width: 1000,
            height: 720,
        },
        ..WindowInfo::default()
    };
    #[cfg(target_os = "windows")]
    let info = info.set_as_popup(Default::default(), "NexusBro — инструменты разработчика");
    h.show_dev_tools(
        Some(&info),
        Some(&mut crate::devtools::klient(id,tx)),
        Some(&BrowserSettings::default()),
        inspect.as_ref(),
    );
}

// Callback handles and URLs never cross into the popup renderer. Only an opaque
// request token and enabled command IDs return through the owner-bound channel.
struct ContextPending {
    browser_id: u64,
    native_id: i32,
    callback: RunContextMenuCallback,
    allowed: std::collections::HashSet<i32>,
    expires: std::time::Instant,
    tx: Sender<Sobytie>,
}
thread_local! {
    static CONTEXTS: std::cell::RefCell<std::collections::HashMap<String,ContextPending>> = Default::default();
}
fn cancel_pending(token: String, pending: ContextPending) {
    pending.callback.cancel();
    let _=pending.tx.send(Sobytie::Sluzhebnoe(serde_json::json!({"tip":"page-context-menu-cancel","id":pending.browser_id,"token":token})));
}
pub fn cancel_context(native_id:i32) {
    let pending=CONTEXTS.with(|menus|{
        let mut menus=menus.borrow_mut();
        let tokens=menus.iter().filter(|(_,p)|p.native_id==native_id).map(|(token,_)|token.clone()).collect::<Vec<_>>();
        tokens.into_iter().filter_map(|token|menus.remove(&token).map(|p|(token,p))).collect::<Vec<_>>()
    });
    for(token,pending)in pending{cancel_pending(token,pending);}
}
pub fn prune_contexts() {
    let pending=CONTEXTS.with(|menus|{
        let mut menus=menus.borrow_mut();
        let tokens=menus.iter().filter(|(_,p)|p.expires<=std::time::Instant::now()).map(|(token,_)|token.clone()).collect::<Vec<_>>();
        tokens.into_iter().filter_map(|token|menus.remove(&token).map(|p|(token,p))).collect::<Vec<_>>()
    });
    for(token,pending)in pending{cancel_pending(token,pending);}
}
pub fn choose_context(token:&str, command_id:Option<i32>, native_ids:&[i32]) {
    let pending=CONTEXTS.with(|menus|{
        let mut menus=menus.borrow_mut();
        if menus.get(token).is_some_and(|pending|native_ids.contains(&pending.native_id)){menus.remove(token)}else{None}
    });
    if let Some(pending)=pending{
        if let Some(command)=command_id.filter(|id|pending.allowed.contains(id)).filter(|_|pending.expires>std::time::Instant::now()){
            pending.callback.cont(command,EventFlags::default());
        }else{cancel_pending(token.into(),pending);}
    }
}
fn context_items(model:&MenuModel,prefix:&str,enabled:bool,depth:usize,items:&mut Vec<serde_json::Value>,allowed:&mut std::collections::HashSet<i32>){
    if depth>3{return;}
    for i in 0..model.count(){
        if items.len()>=80{break;}
        if model.is_visible_at(i)==0{continue;}
        let kind=model.type_at(i);
        if kind==MenuItemType::SEPARATOR{items.push(serde_json::Value::Null);continue;}
        let text=CefString::from(&model.label_at(i)).to_string().chars().filter(|c|!c.is_control()||*c=='\t').take(240).collect::<String>();
        if text.is_empty(){continue;}
        let text=if prefix.is_empty(){text}else{format!("{prefix} / {text}")};
        let enabled=enabled&&model.is_enabled_at(i)!=0;
        if kind==MenuItemType::SUBMENU{if let Some(sub)=model.sub_menu_at(i){context_items(&sub,&text,enabled,depth+1,items,allowed);}continue;}
        let command=model.command_id_at(i);
        if enabled{allowed.insert(command);}
        let text=if model.is_checked_at(i)!=0{format!("✓ {text}")}else{text};
        let text=text.chars().take(280).collect::<String>();
        items.push(serde_json::json!({"command_id":command,"text":text,"enabled":enabled}));
    }
}
pub fn open_link(id:u64,url:&str,private:bool,background:bool,tx:&Sender<Sobytie>)->bool{
    let url=url.trim();
    if url.is_empty()||url.len()>2*1024*1024{return false;}
    tx.send(Sobytie::Komanda(Komanda::OpenLink{opener_id:id,adres:url.into(),privatnaya:private,background})).is_ok()
}

const OPEN_LINK: i32 = 26501;
const OPEN_IMAGE: i32 = 26502;
const DUPLICATE: i32 = 26503;
const INSPECT: i32 = 26504;
const NEW_TAB: i32 = 26505;
const PEREVOD: i32 = 26506;
wrap_context_menu_handler! {
    struct MenyuStranicy { id: u64, tx: Sender<Sobytie>, privatnaya:bool }
    impl ContextMenuHandler {
        fn on_before_context_menu(&self, browser: Option<&mut Browser>, _frame: Option<&mut Frame>,
            params: Option<&mut ContextMenuParams>, model: Option<&mut MenuModel>) {
            let (Some(p), Some(m)) = (params, model) else { return };
            if p.is_editable()!=0{if let Some(browser)=browser{crate::memory_guard::edited(browser.identifier());}}
            for (cmd, label) in [(MenuId::BACK, "Назад"), (MenuId::FORWARD, "Вперёд"), (MenuId::RELOAD, "Обновить"),
                (MenuId::STOPLOAD, "Остановить"), (MenuId::CUT, "Вырезать"), (MenuId::COPY, "Копировать"),
                (MenuId::PASTE, "Вставить"), (MenuId::SELECT_ALL, "Выделить всё"), (MenuId::PRINT, "Печать…"),
                (MenuId::VIEW_SOURCE, "Исходный код страницы\tCtrl+U")] {
                m.set_label(cmd.get_raw() as i32, Some(&CefString::from(label)));
            }
            if !CefString::from(&p.link_url()).to_string().is_empty() {
                m.add_item(OPEN_LINK, Some(&CefString::from("Открыть ссылку в новой вкладке")));
            }
            if p.has_image_contents() != 0 && !CefString::from(&p.source_url()).to_string().is_empty() {
                m.add_item(OPEN_IMAGE, Some(&CefString::from("Открыть изображение в новой вкладке")));
            }
            m.add_separator();
            m.add_item(NEW_TAB, Some(&CefString::from("Новая вкладка\tCtrl+T")));
            m.add_item(DUPLICATE, Some(&CefString::from("Дублировать вкладку")));
            if m.index_of(MenuId::VIEW_SOURCE.get_raw() as i32) < 0 {
                m.add_item(MenuId::VIEW_SOURCE.get_raw() as i32, Some(&CefString::from("Исходный код страницы\tCtrl+U")));
            }
            let stranica = CefString::from(&p.page_url()).to_string();
            if reqwest::Url::parse(&stranica).ok().is_some_and(|u|matches!(u.scheme(),"http"|"https")) {
                m.add_separator();
                m.add_item(PEREVOD, Some(&CefString::from("Перевести страницу на русский")));
                m.add_item(PEREVOD+1, Some(&CefString::from("Показать оригинал страницы")));
            }
            m.add_item(INSPECT, Some(&CefString::from("Исследовать элемент\tF12")));
        }
        fn run_context_menu(&self,browser:Option<&mut Browser>,_frame:Option<&mut Frame>,params:Option<&mut ContextMenuParams>,model:Option<&mut MenuModel>,callback:Option<&mut RunContextMenuCallback>)->i32{
            let Some(callback)=callback else{return 1;};
            let(Some(params),Some(model),Some(browser))=(params,model,browser)else{callback.cancel();return 1;};
            let native_id=browser.identifier();cancel_context(native_id);prune_contexts();
            let mut items=Vec::new();let mut allowed=std::collections::HashSet::new();
            context_items(model,"",true,0,&mut items,&mut allowed);
            if items.iter().all(serde_json::Value::is_null){callback.cancel();return 1;}
            use rand_core::RngCore;
            let mut random=[0_u8;24];rand_core::OsRng.fill_bytes(&mut random);
            let token=random.iter().map(|b|format!("{b:02x}")).collect::<String>();
            let pending=ContextPending{browser_id:self.id,native_id,callback:callback.clone(),allowed,expires:std::time::Instant::now()+std::time::Duration::from_secs(120),tx:self.tx.clone()};
            let room=CONTEXTS.with(|menus|{let mut menus=menus.borrow_mut();if menus.len()>=32{false}else{menus.insert(token.clone(),pending);true}});
            if !room{callback.cancel();return 1;}
            let result=self.tx.send(Sobytie::Sluzhebnoe(serde_json::json!({"tip":"page-context-menu","id":self.id,"token":token,"x":params.xcoord(),"y":params.ycoord(),"items":items,"privatnaya":self.privatnaya})));
            if result.is_err(){cancel_context(native_id);}
            1
        }
        fn on_context_menu_dismissed(&self,browser:Option<&mut Browser>,_frame:Option<&mut Frame>){if let Some(browser)=browser{cancel_context(browser.identifier());}}
        fn on_context_menu_command(&self, _browser: Option<&mut Browser>, frame: Option<&mut Frame>,
            params: Option<&mut ContextMenuParams>, command_id: i32, _event_flags: EventFlags) -> i32 {
            let cmd = match command_id {
                PEREVOD => Some(Komanda::Perevesti{id:self.id}),
                n if n==PEREVOD+1 => Some(Komanda::PerevodOriginal{id:self.id}),
                OPEN_LINK => params.as_ref().map(|p| Komanda::OpenLink { opener_id:self.id,adres:CefString::from(&p.link_url()).to_string(),privatnaya:self.privatnaya,background:true }),
                OPEN_IMAGE => params.as_ref().map(|p| Komanda::OpenLink { opener_id:self.id,adres:CefString::from(&p.source_url()).to_string(),privatnaya:self.privatnaya,background:true }),
                DUPLICATE => frame.as_ref().map(|f| Komanda::Novaya { adres: CefString::from(&f.url()).to_string() }),
                NEW_TAB => Some(Komanda::Novaya { adres: crate::nastrojki::zagruzit().novaya_stranica }),
                INSPECT => params.as_ref().map(|p| Komanda::Issledovat { id: self.id, x: p.xcoord(), y: p.ycoord() }),
                n if n == MenuId::VIEW_SOURCE.get_raw() as i32 => Some(Komanda::Iskhodnik { id: self.id }),
                _ => None,
            };
            if let Some(c) = cmd { let c=match c{Komanda::Novaya{adres} if self.privatnaya=>Komanda::NovayaPrivatnaya{adres},c=>c};let _ = self.tx.send(Sobytie::Komanda(c)); return 1 }
            0
        }
    }
}
pub fn menyu(id: u64, tx: Sender<Sobytie>, privatnaya:bool) -> ContextMenuHandler {
    MenyuStranicy::new(id, tx, privatnaya)
}

wrap_download_image_callback! {
    struct Ikonka { id: u64, stranica: String, tx: Sender<Sobytie> }
    impl DownloadImageCallback {
        fn on_download_image_finished(&self, _image_url: Option<&CefString>, _http_status_code: i32, image: Option<&mut Image>) {
            let (mut width, mut height) = (0, 0);
            let Some(png) = image.and_then(|i| i.as_png(1.0, 1, Some(&mut width), Some(&mut height))) else { return };
            let size = png.size();
            if size == 0 || size > 128 * 1024 { return }
            let mut data = vec![0; size];
            if png.data(Some(&mut data), 0) != size { return }
            let src = format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(data));
            let _ = self.tx.send(Sobytie::Ikonka(self.id, self.stranica.clone(), src));
        }
    }
}
pub fn ikonka(
    id: u64,
    tx: Sender<Sobytie>,
    browser: Option<&mut Browser>,
    urls: Option<&mut CefStringList>,
) {
    let (Some(b), Some(urls)) = (browser, urls) else {
        return;
    };
    // BorrowedMut сохраняет исходный opaque handle. From<*const> копирует
    // opaque value и не поддерживает IntoIterator (возвращает пустой список).
    let list = CefStringList::from(<*mut sys::_cef_string_list_t>::from(&mut *urls));
    let Some(url) = list.into_iter().next() else {
        return;
    };
    let Some(h) = b.host() else { return };
    let stranica = b
        .main_frame()
        .map(|f| CefString::from(&f.url()).to_string())
        .unwrap_or_default();
    h.download_image(
        Some(&CefString::from(url.as_str())),
        1,
        32,
        0,
        Some(&mut Ikonka::new(id, stranica, tx)),
    );
}
