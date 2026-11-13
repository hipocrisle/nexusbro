//! Пользовательские действия страниц CEF. Сетевые иконки получает сам CEF,
//! поэтому они используют тот же профиль и VPN, а оболочка получает только PNG.
use cef::*;
use crate::{cef_vkladki::Sobytie, sessiya::Komanda};
use std::sync::mpsc::Sender;
use base64::Engine;

#[cfg(target_os = "windows")]
type NativeKeyEvent = cef::sys::MSG;
#[cfg(not(target_os = "windows"))]
type NativeKeyEvent = cef::sys::XEvent;

wrap_keyboard_handler! {
    struct Klaviatura { id: u64, tx: Sender<Sobytie> }
    impl KeyboardHandler {
        fn on_pre_key_event(&self, _browser: Option<&mut Browser>, event: Option<&KeyEvent>,
            _os_event: Option<&mut NativeKeyEvent>, _is_keyboard_shortcut: Option<&mut i32>) -> i32 {
            let Some(e) = event else { return 0 };
            if e.type_ != KeyEventType::RAWKEYDOWN && e.type_ != KeyEventType::KEYDOWN { return 0 }
            let ctrl = e.modifiers & (1 << 2) != 0;
            let shift = e.modifiers & (1 << 1) != 0;
            let cmd = match e.windows_key_code {
                123 => Some(Komanda::Instrumenty { id: self.id }),
                73 if ctrl && shift => Some(Komanda::Instrumenty { id: self.id }),
                85 if ctrl => Some(Komanda::Iskhodnik { id: self.id }),
                87 if ctrl => Some(Komanda::Zakryt { id: self.id }),
                84 if ctrl => Some(Komanda::Novaya { adres: crate::nastrojki::zagruzit().novaya_stranica }),
                82 if ctrl => Some(Komanda::Obnovit { id: self.id }),
                9 if ctrl => Some(Komanda::Sleduyushchaya { nazad: shift }),
                76 if ctrl => { let _ = self.tx.send(Sobytie::AdresnyiVvod); return 1; }
                _ => None,
            };
            if let Some(c) = cmd { let _ = self.tx.send(Sobytie::Komanda(c)); return 1 }
            0
        }
    }
}
pub fn klaviatura(id: u64, tx: Sender<Sobytie>) -> KeyboardHandler { Klaviatura::new(id, tx) }

wrap_client! { struct InstrumentyClient; impl Client {} }
pub fn instrumenty(b: &Browser, inspect: Option<Point>, toggle: bool) {
    let Some(h) = b.host() else { return };
    if toggle && h.has_dev_tools() != 0 { h.close_dev_tools(); return }
    // DevTools с CEF 128 работают только в Chrome style, даже у Alloy-страницы.
    let info = WindowInfo { runtime_style: RuntimeStyle::CHROME,
        window_name: CefString::from("NexusBro — инструменты разработчика"),
        bounds: Rect { x: 80, y: 80, width: 1000, height: 720 }, ..WindowInfo::default() };
    #[cfg(target_os = "windows")]
    let info = info.set_as_popup(Default::default(), "NexusBro — инструменты разработчика");
    h.show_dev_tools(Some(&info), Some(&mut InstrumentyClient::new()), Some(&BrowserSettings::default()), inspect.as_ref());
}

const OPEN_LINK: i32 = 26501;
const OPEN_IMAGE: i32 = 26502;
const DUPLICATE: i32 = 26503;
const INSPECT: i32 = 26504;
const NEW_TAB: i32 = 26505;
wrap_context_menu_handler! {
    struct MenyuStranicy { id: u64, tx: Sender<Sobytie> }
    impl ContextMenuHandler {
        fn on_before_context_menu(&self, _browser: Option<&mut Browser>, _frame: Option<&mut Frame>,
            params: Option<&mut ContextMenuParams>, model: Option<&mut MenuModel>) {
            let (Some(p), Some(m)) = (params, model) else { return };
            for (cmd, label) in [(MenuId::BACK, "Назад"), (MenuId::FORWARD, "Вперёд"), (MenuId::RELOAD, "Обновить"),
                (MenuId::STOPLOAD, "Остановить"), (MenuId::CUT, "Вырезать"), (MenuId::COPY, "Копировать"),
                (MenuId::PASTE, "Вставить"), (MenuId::SELECT_ALL, "Выделить всё"), (MenuId::PRINT, "Печать…"),
                (MenuId::VIEW_SOURCE, "Исходный код страницы    Ctrl+U")] {
                m.set_label(cmd.get_raw() as i32, Some(&CefString::from(label)));
            }
            if !CefString::from(&p.link_url()).to_string().is_empty() {
                m.add_item(OPEN_LINK, Some(&CefString::from("Открыть ссылку в новой вкладке")));
            }
            if p.has_image_contents() != 0 && !CefString::from(&p.source_url()).to_string().is_empty() {
                m.add_item(OPEN_IMAGE, Some(&CefString::from("Открыть изображение в новой вкладке")));
            }
            m.add_separator();
            m.add_item(NEW_TAB, Some(&CefString::from("Новая вкладка    Ctrl+T")));
            m.add_item(DUPLICATE, Some(&CefString::from("Дублировать вкладку")));
            if m.index_of(MenuId::VIEW_SOURCE.get_raw() as i32) < 0 {
                m.add_item(MenuId::VIEW_SOURCE.get_raw() as i32, Some(&CefString::from("Исходный код страницы    Ctrl+U")));
            }
            m.add_item(INSPECT, Some(&CefString::from("Исследовать элемент    F12")));
        }
        fn on_context_menu_command(&self, _browser: Option<&mut Browser>, frame: Option<&mut Frame>,
            params: Option<&mut ContextMenuParams>, command_id: i32, _event_flags: EventFlags) -> i32 {
            let cmd = match command_id {
                OPEN_LINK => params.map(|p| Komanda::Novaya { adres: CefString::from(&p.link_url()).to_string() }),
                OPEN_IMAGE => params.map(|p| Komanda::Novaya { adres: CefString::from(&p.source_url()).to_string() }),
                DUPLICATE => frame.map(|f| Komanda::Novaya { adres: CefString::from(&f.url()).to_string() }),
                NEW_TAB => Some(Komanda::Novaya { adres: crate::nastrojki::zagruzit().novaya_stranica }),
                INSPECT => params.map(|p| Komanda::Issledovat { id: self.id, x: p.xcoord(), y: p.ycoord() }),
                n if n == MenuId::VIEW_SOURCE.get_raw() as i32 => Some(Komanda::Iskhodnik { id: self.id }),
                _ => None,
            };
            if let Some(c) = cmd { let _ = self.tx.send(Sobytie::Komanda(c)); return 1 }
            0
        }
    }
}
pub fn menyu(id: u64, tx: Sender<Sobytie>) -> ContextMenuHandler { MenyuStranicy::new(id, tx) }

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
pub fn ikonka(id: u64, tx: Sender<Sobytie>, browser: Option<&mut Browser>, urls: Option<&mut CefStringList>) {
    let (Some(b), Some(urls)) = (browser, urls) else { return };
    // BorrowedMut сохраняет исходный opaque handle. From<*const> копирует
    // opaque value и не поддерживает IntoIterator (возвращает пустой список).
    let list = CefStringList::from(<*mut sys::_cef_string_list_t>::from(&mut *urls));
    let Some(url) = list.into_iter().next() else { return };
    let Some(h) = b.host() else { return };
    let stranica = b.main_frame().map(|f| CefString::from(&f.url()).to_string()).unwrap_or_default();
    h.download_image(Some(&CefString::from(url.as_str())), 1, 32, 0, Some(&mut Ikonka::new(id, stranica, tx)));
}
