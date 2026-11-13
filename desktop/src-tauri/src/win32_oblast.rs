//! Оболочка WebView2 не рисует поверх нативной страницы CEF.
//! Только собственные непосредственные child HWND; чужие потоки не трогаем.
use crate::okna::Mesto;
use windows::Win32::{
    Foundation::{BOOL, HWND, LPARAM, POINT, RECT},
    Graphics::Gdi::*,
    System::Threading::GetCurrentProcessId,
    UI::WindowsAndMessaging::*,
};

struct Oblast(HRGN);
impl Oblast {
    unsafe fn rect(x1: i32, y1: i32, x2: i32, y2: i32) -> anyhow::Result<Self> {
        let h = CreateRectRgn(x1, y1, x2, y2);
        anyhow::ensure!(!h.0.is_null(), "CreateRectRgn не создал область");
        Ok(Self(h))
    }
}
impl Drop for Oblast {
    fn drop(&mut self) { unsafe { let _ = DeleteObject(self.0); } }
}

pub fn primenit(roditel: HWND, mesto: Option<Mesto>) -> anyhow::Result<()> {
    struct Dannye { roditel: HWND, mesto: Option<Mesto>, oshibki: Vec<String> }
    unsafe fn okno(h: HWND, d: &Dannye) -> anyhow::Result<()> {
        let mut pid = 0;
        GetWindowThreadProcessId(h, Some(&mut pid));
        if pid != GetCurrentProcessId() || GetParent(h)? != d.roditel { return Ok(()) }
        let staraya = Oblast::rect(0, 0, 0, 0)?;
        let est = GetWindowRgn(h, staraya.0).0 != 0;
        let Some(m) = d.mesto else {
            // NULL возвращает полную область для диалогов и пустого состояния.
            if est { anyhow::ensure!(SetWindowRgn(h, HRGN::default(), true) != 0, "SetWindowRgn reset"); }
            return Ok(())
        };
        let mut r = RECT::default();
        GetWindowRect(h, &mut r)?;
        let mut origin = POINT { x: m.x, y: m.y };
        ClientToScreen(d.roditel, &mut origin).ok()?;
        let x = origin.x - r.left;
        let y = origin.y - r.top;
        let oblast = Oblast::rect(0, 0, r.right-r.left, r.bottom-r.top)?;
        let dyra = Oblast::rect(x, y, x.saturating_add(m.w as i32), y.saturating_add(m.h as i32))?;
        anyhow::ensure!(CombineRgn(oblast.0, oblast.0, dyra.0, RGN_DIFF).0 != 0, "CombineRgn");
        // Не генерировать бесконечные WM_WINDOWPOSCHANGED при восстановлении фокуса.
        if !est || !EqualRgn(staraya.0, oblast.0).as_bool() {
            anyhow::ensure!(SetWindowRgn(h, oblast.0, true) != 0, "SetWindowRgn apply");
            // После успеха регион принадлежит Windows, удалять его нельзя.
            std::mem::forget(oblast);
        }
        Ok(())
    }
    unsafe extern "system" fn potomok(h: HWND, p: LPARAM) -> BOOL {
        let d = &mut *(p.0 as *mut Dannye);
        if let Err(e) = okno(h, d) { d.oshibki.push(e.to_string()); }
        BOOL(1)
    }
    let mut d = Dannye { roditel, mesto, oshibki: Vec::new() };
    unsafe { EnumChildWindows(roditel, Some(potomok), LPARAM(&mut d as *mut _ as isize)).ok()?; }
    anyhow::ensure!(d.oshibki.is_empty(), "{}", d.oshibki.join("; "));
    Ok(())
}
