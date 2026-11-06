//! Встраивание чужого окна внутрь нашего.
//!
//! ✅ Проверено опытом 06.11.2026 на X11: окно chromium становится дочерним
//! нашему, двигается и меняет размер под полосой вкладок.
//!
//! ⛔ Окно ищем ПО НОМЕРУ ПРОЦЕССА, а не по заголовку: заголовок окна
//! браузера — это имя открытой страницы, он меняется и не уникален.

#[allow(dead_code)]
#[derive(Clone, Copy, Debug)]
pub struct Mesto {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

// ───────────────────────────── Linux (X11) ─────────────────────────────
#[cfg(target_os = "linux")]
pub mod sistema {
    use super::Mesto;
    use anyhow::{anyhow, Result};
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::*;
    use x11rb::rust_connection::RustConnection;

    fn svyaz() -> Result<(RustConnection, usize)> {
        Ok(x11rb::connect(None)?)
    }

    /// Найти верхнеуровневое окно, принадлежащее процессу pid.
    pub fn najti_okno(pid: u32, zhdat_sek: u64) -> Result<u32> {
        let (conn, screen) = svyaz()?;
        let koren = conn.setup().roots[screen].root;
        let atom_pid = conn
            .intern_atom(false, b"_NET_WM_PID")?
            .reply()?
            .atom;
        let srok = std::time::Instant::now();
        loop {
            if let Some(w) = obojti(&conn, koren, atom_pid, pid)? {
                return Ok(w);
            }
            if srok.elapsed().as_secs() >= zhdat_sek {
                return Err(anyhow!("окно браузера не появилось за {} с", zhdat_sek));
            }
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
    }

    fn obojti(conn: &RustConnection, okno: u32, atom_pid: Atom, pid: u32) -> Result<Option<u32>> {
        let derevo = conn.query_tree(okno)?.reply()?;
        for ch in derevo.children {
            let svojstvo = conn
                .get_property(false, ch, atom_pid, AtomEnum::CARDINAL, 0, 1)?
                .reply();
            if let Ok(p) = svojstvo {
                if let Some(mut v) = p.value32() {
                    if v.next() == Some(pid) {
                        // Берём только видимые окна: у chromium есть служебные 1x1.
                        if let Ok(g) = conn.get_geometry(ch)?.reply() {
                            if g.width > 100 && g.height > 100 {
                                return Ok(Some(ch));
                            }
                        }
                    }
                }
            }
            if let Some(najdeno) = obojti(conn, ch, atom_pid, pid)? {
                return Ok(Some(najdeno));
            }
        }
        Ok(None)
    }

    pub fn vstroit(chuzhoe: u32, nashe: u64, m: Mesto) -> Result<()> {
        // ⛔ Запросы к X11 уходят БЕЗ подтверждения: reparent может не
        // состояться, а код решит, что всё хорошо. Поймали 06.11.2026 —
        // самопроверка рапортовала «ок», окно лежало сверху само по себе.
        // Поэтому встраиваем и ПРОВЕРЯЕМ, кто теперь родитель.
        let (conn, _) = svyaz()?;
        for popytka in 1..=6 {
            conn.reparent_window(chuzhoe, nashe as u32, m.x as i16, m.y as i16)?;
            conn.configure_window(
                chuzhoe,
                &ConfigureWindowAux::new().x(m.x).y(m.y).width(m.w).height(m.h),
            )?;
            conn.map_window(chuzhoe)?;
            conn.flush()?;
            std::thread::sleep(std::time::Duration::from_millis(180));
            if let Ok(d) = conn.query_tree(chuzhoe)?.reply() {
                if d.parent == nashe as u32 {
                    return Ok(());
                }
            }
            let _ = popytka;
        }
        Err(anyhow!("окно браузера не встало внутрь нашего за 6 попыток"))
    }

    pub fn peremestit(chuzhoe: u32, m: Mesto) -> Result<()> {
        let (conn, _) = svyaz()?;
        conn.configure_window(
            chuzhoe,
            &ConfigureWindowAux::new().x(m.x).y(m.y).width(m.w).height(m.h),
        )?;
        conn.flush()?;
        Ok(())
    }

    pub fn pokazat(chuzhoe: u32, vidno: bool) -> Result<()> {
        let (conn, _) = svyaz()?;
        if vidno {
            conn.map_window(chuzhoe)?;
            conn.configure_window(
                chuzhoe,
                &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE),
            )?;
        } else {
            conn.unmap_window(chuzhoe)?;
        }
        conn.flush()?;
        Ok(())
    }
}

// ──────────────────────────────── Windows ──────────────────────────────
#[cfg(target_os = "windows")]
pub mod sistema {
    use super::Mesto;
    use anyhow::{anyhow, Result};
    use windows::Win32::Foundation::{BOOL, HWND, LPARAM, RECT};
    use windows::Win32::UI::WindowsAndMessaging::*;

    struct Poisk {
        pid: u32,
        najdeno: HWND,
    }

    unsafe extern "system" fn proverit(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let p = &mut *(lparam.0 as *mut Poisk);
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid == p.pid && IsWindowVisible(hwnd).as_bool() {
            let mut r = RECT::default();
            if GetWindowRect(hwnd, &mut r).is_ok()
                && (r.right - r.left) > 100
                && (r.bottom - r.top) > 100
            {
                p.najdeno = hwnd;
                return BOOL(0); // нашли, останавливаем обход
            }
        }
        BOOL(1)
    }

    pub fn najti_okno(pid: u32, zhdat_sek: u64) -> Result<u32> {
        let srok = std::time::Instant::now();
        loop {
            let mut p = Poisk { pid, najdeno: HWND(std::ptr::null_mut()) };
            unsafe {
                let _ = EnumWindows(Some(proverit), LPARAM(&mut p as *mut _ as isize));
            }
            if !p.najdeno.0.is_null() {
                return Ok(p.najdeno.0 as usize as u32);
            }
            if srok.elapsed().as_secs() >= zhdat_sek {
                return Err(anyhow!("окно браузера не появилось за {} с", zhdat_sek));
            }
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
    }

    fn hwnd(v: u32) -> HWND {
        HWND(v as usize as *mut core::ffi::c_void)
    }

    pub fn vstroit(chuzhoe: u32, nashe: u64, m: Mesto) -> Result<()> {
        unsafe {
            let ch = hwnd(chuzhoe);
            // ⛔ Снимаем рамку и заголовок: внутри нашего окна они не нужны
            // и мешают — иначе получается «окно в окне».
            let stil = GetWindowLongPtrW(ch, GWL_STYLE) as u32;
            let novyj = (stil & !(WS_POPUP.0 | WS_CAPTION.0 | WS_THICKFRAME.0)) | WS_CHILD.0;
            SetWindowLongPtrW(ch, GWL_STYLE, novyj as isize);
            SetParent(ch, HWND(nashe as usize as *mut core::ffi::c_void))?;
            peremestit(chuzhoe, m)?;
            let _ = ShowWindow(ch, SW_SHOW);
        }
        Ok(())
    }

    pub fn peremestit(chuzhoe: u32, m: Mesto) -> Result<()> {
        unsafe {
            SetWindowPos(
                hwnd(chuzhoe),
                None,
                m.x,
                m.y,
                m.w as i32,
                m.h as i32,
                SWP_NOZORDER | SWP_NOACTIVATE,
            )?;
        }
        Ok(())
    }

    pub fn pokazat(chuzhoe: u32, vidno: bool) -> Result<()> {
        unsafe {
            let _ = ShowWindow(hwnd(chuzhoe), if vidno { SW_SHOW } else { SW_HIDE });
        }
        Ok(())
    }
}

// ──────────────────────── остальные системы ────────────────────────────
#[cfg(not(any(target_os = "linux", target_os = "windows")))]
pub mod sistema {
    use super::Mesto;
    use anyhow::{anyhow, Result};
    pub fn najti_okno(_pid: u32, _zhdat: u64) -> Result<u32> {
        Err(anyhow!("встраивание на этой системе пока не сделано"))
    }
    pub fn vstroit(_c: u32, _n: u64, _m: Mesto) -> Result<()> {
        Err(anyhow!("встраивание на этой системе пока не сделано"))
    }
    pub fn peremestit(_c: u32, _m: Mesto) -> Result<()> {
        Ok(())
    }
    pub fn pokazat(_c: u32, _v: bool) -> Result<()> {
        Ok(())
    }
}
