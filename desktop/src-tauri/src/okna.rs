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
            let mut kandidaty: Vec<(u32, i64)> = Vec::new();
            sobrat(&conn, koren, atom_pid, pid, &mut kandidaty)?;
            if let Some((okno, _)) = kandidaty.iter().max_by_key(|(_, a)| *a) {
                return Ok(*okno);
            }
            if srok.elapsed().as_secs() >= zhdat_sek {
                return Err(anyhow!("окно браузера не появилось за {} с", zhdat_sek));
            }
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
    }

    /// Обойти дерево и собрать видимые окна (>100×100) с площадью:
    /// берём САМОЕ БОЛЬШОЕ — иначе вместо браузера можно поймать
    /// пузырь или служебное окно (10.11.2026).
    fn sobrat(
        conn: &RustConnection,
        okno: u32,
        atom_pid: Atom,
        pid: u32,
        kuda: &mut Vec<(u32, i64)>,
    ) -> Result<()> {
        let derevo = conn.query_tree(okno)?.reply()?;
        for ch in derevo.children {
            let svojstvo = conn
                .get_property(false, ch, atom_pid, AtomEnum::CARDINAL, 0, 1)?
                .reply();
            if let Ok(p) = svojstvo {
                if let Some(mut v) = p.value32() {
                    if v.next() == Some(pid) {
                        if let Ok(g) = conn.get_geometry(ch)?.reply() {
                            if g.width > 100 && g.height > 100 {
                                kuda.push((ch, g.width as i64 * g.height as i64));
                            }
                        }
                    }
                }
            }
            sobrat(conn, ch, atom_pid, pid, kuda)?;
        }
        Ok(())
    }

    /// Все видимые окна (>200×200) процессов, крупнейшее первым
    /// (см. vse_okna для Windows — сессия движка бывает многооконной).
    pub fn vse_okna(korni: &[u32]) -> Vec<u32> {
        let Ok((conn, screen)) = svyaz() else { return Vec::new() };
        let atom_pid = match conn.intern_atom(false, b"_NET_WM_PID") {
            Ok(c) => match c.reply() {
                Ok(r) => r.atom,
                Err(_) => return Vec::new(),
            },
            Err(_) => return Vec::new(),
        };
        let koren = conn.setup().roots[screen].root;
        let mut kandidaty: Vec<(u32, i64)> = Vec::new();
        for &pid in korni {
            let _ = sobrat(&conn, koren, atom_pid, pid, &mut kandidaty);
        }
        kandidaty.retain(|&(_, a)| a > 200 * 200);
        kandidaty.sort_by_key(|&(_, a)| std::cmp::Reverse(a));
        kandidaty.into_iter().map(|(o, _)| o).collect()
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

    /// Вежливо попросить окно закрыться (как клик по крестику):
    /// WM_DELETE_WINDOW по протоколу ICCCM. Chromium так СОХРАНЯЕТ
    /// сессию сам, и при следующем запуске не лезет с «восстановить».
    pub fn zakryt_vezhlivo(chuzhoe: u32) -> Result<()> {
        let (conn, _) = svyaz()?;
        let protokoly = conn.intern_atom(false, b"WM_PROTOCOLS")?.reply()?.atom;
        let udalenie = conn.intern_atom(false, b"WM_DELETE_WINDOW")?.reply()?.atom;
        let sobytie = ClientMessageEvent {
            response_type: CLIENT_MESSAGE_EVENT,
            format: 32,
            sequence: 0,
            window: chuzhoe,
            type_: protokoly,
            data: ClientMessageData::from([udalenie, 0, 0, 0, 0]),
        };
        conn.send_event(false, chuzhoe, EventMask::NO_EVENT, &sobytie)?;
        conn.flush()?;
        Ok(())
    }

    /// Живо ли встроенное окно и осталось ли нашим потомком: Chromium
    /// иногда ПЕРЕСОЗДАЁТ своё окно — новое рождается чужим, с рамкой
    /// и своей жизнью (10.11.2026, поймал user).
    pub fn nash_li(chuzhoe: u32, nashe: u64) -> bool {
        let Ok((conn, _)) = svyaz() else { return false };
        let Ok(derevo) = conn.query_tree(chuzhoe) else { return false };
        let Ok(d) = derevo.reply() else { return false };
        d.parent == nashe as u32
    }

    // На X11 ввод до встроенного окна доходит сам — заглушки.
    pub fn prisoedinit_vvod(_chuzhoe: u32) {}
    pub fn otsoedinit_vvod(_chuzhoe: u32) {}
    pub fn fokus_dvizhku(_chuzhoe: u32) {}
    pub fn kto_v_peredne() -> String {
        String::new()
    }
    pub fn pid_okna(_c: u32) -> Option<u32> {
        None
    }
    pub fn okno_zhivo(_c: u32) -> bool {
        true
    }
    pub fn pid_zhiv(pid: u32) -> bool {
        std::path::Path::new("/proc").join(pid.to_string()).exists()
    }
    pub fn ubit_po_pid(_p: u32) {}
    pub fn spryatat_chuzhie(_korni: &[u32]) {}
}

// ──────────────────────────────── Windows ──────────────────────────────
#[cfg(target_os = "windows")]
pub mod sistema {
    use super::Mesto;
    use anyhow::{anyhow, Result};
    use windows::Win32::Foundation::{BOOL, CloseHandle, HWND, LPARAM, RECT, WPARAM};
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32First, Process32Next,
        PROCESSENTRY32, TH32CS_SNAPPROCESS,
    };
    use windows::Win32::UI::WindowsAndMessaging::*;

    pub fn derevo_processov(koren: u32) -> Vec<u32> {
        let mut ids = vec![koren];
        unsafe {
            let Ok(snimok) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else {
                return ids;
            };
            let mut e = PROCESSENTRY32::default();
            e.dwSize = std::mem::size_of::<PROCESSENTRY32>() as u32;
            let mut vse = Vec::new();
            if Process32First(snimok, &mut e).is_ok() {
                loop {
                    vse.push((e.th32ProcessID, e.th32ParentProcessID));
                    if Process32Next(snimok, &mut e).is_err() {
                        break;
                    }
                }
            }
            let _ = CloseHandle(snimok);
            let mut roslo = true;
            while roslo {
                roslo = false;
                for (pid, roditel) in &vse {
                    if ids.contains(roditel) && !ids.contains(pid) {
                        ids.push(*pid);
                        roslo = true;
                    }
                }
            }
        }
        ids
    }

    struct Poisk {
        pids: Vec<u32>,
        naideno: Vec<(HWND, i64)>,
    }

    unsafe extern "system" fn proverit(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let p = &mut *(lparam.0 as *mut Poisk);
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if p.pids.contains(&pid) && IsWindowVisible(hwnd).as_bool() {
            // ⛔ Свёрнутое окно (160x28) фильтр размера не пропустит —
            // возвращаем на место и берём на следующем проходе.
            if IsIconic(hwnd).as_bool() {
                let _ = ShowWindow(hwnd, SW_RESTORE);
                return BOOL(1);
            }
            let mut r = RECT::default();
            if GetWindowRect(hwnd, &mut r).is_ok() {
                let ploshad = (r.right - r.left) as i64 * (r.bottom - r.top) as i64;
                if (r.right - r.left) > 100 && (r.bottom - r.top) > 100 {
                    p.naideno.push((hwnd, ploshad));
                }
            }
        }
        BOOL(1)
    }

    pub fn najti_okno(pid: u32, zhdat_sek: u64) -> Result<u32> {
        let srok = std::time::Instant::now();
        loop {
            let mut p = Poisk { pids: derevo_processov(pid), naideno: Vec::new() };
            unsafe {
                let _ = EnumWindows(Some(proverit), LPARAM(&mut p as *mut _ as isize));
            }
            // ⛔ Берём САМОЕ БОЛЬШОЕ окно: пузырь «Восстановить
            // страницы?» (334x180, тот же класс Chrome_WidgetWin_1)
            // иначе может быть встроен вместо браузера
            // (проверено на машине user'а 10.11.2026).
            if let Some((okno, _)) = p.naideno.iter().max_by_key(|(_, a)| *a) {
                return Ok(okno.0 as usize as u32);
            }
            if srok.elapsed().as_secs() >= zhdat_sek {
                return Err(anyhow!("окно браузера не появилось за {} с", zhdat_sek));
            }
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
    }

    /// Все видимые окна (>200×200) всех деревьев, крупнейшее первым.
    /// ⛔ Сессия движка бывает многооконной (восстановление сессии,
    /// попапы): встраивать надо КАЖДОЕ чужое окно, иначе лишние висят
    /// поверх нашего интерфейса с рамкой и кнопками — «окно хрома
    /// двигается, можно закрыть, закрывает меню» (12.11.2026, user).
    pub fn vse_okna(korni: &[u32]) -> Vec<u32> {
        let vse: std::collections::HashSet<u32> =
            korni.iter().flat_map(|&k| derevo_processov(k)).collect();
        struct P {
            pids: std::collections::HashSet<u32>,
            okna: Vec<(u32, i64)>,
        }
        unsafe extern "system" fn proverit(hwnd: HWND, lparam: LPARAM) -> BOOL {
            let p = &mut *(lparam.0 as *mut P);
            let mut pid = 0u32;
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
            if p.pids.contains(&pid)
                && IsWindowVisible(hwnd).as_bool()
                && !IsIconic(hwnd).as_bool()
            {
                let mut r = RECT::default();
                if GetWindowRect(hwnd, &mut r).is_ok()
                    && (r.right - r.left) > 200
                    && (r.bottom - r.top) > 200
                {
                    let ploshad = (r.right - r.left) as i64 * (r.bottom - r.top) as i64;
                    p.okna.push((hwnd.0 as usize as u32, ploshad));
                }
            }
            BOOL(1)
        }
        let mut p = P { pids: vse, okna: Vec::new() };
        unsafe {
            let _ = EnumWindows(Some(proverit), LPARAM(&mut p as *mut _ as isize));
        }
        p.okna.sort_by_key(|&(_, a)| std::cmp::Reverse(a));
        p.okna.into_iter().map(|(o, _)| o).collect()
    }

    fn hwnd(v: u32) -> HWND {
        HWND(v as usize as *mut core::ffi::c_void)
    }

    /// Подключить ввод ко встроенному окну. ⛔ ГРАБЛЬ SetParent: чужое
    /// окно живёт в СВОЕЙ очереди ввода, и клавиатура до него не доходит,
    /// пока очереди не прикреплены. Вызывать с потока, владеющего НАШИМ
    /// окном (иначе прикрепится не тот поток, и ввода всё равно не будет).
    pub fn prisoedinit_vvod(chuzhoe: u32) {
        use windows::Win32::Foundation::BOOL;
        use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
        unsafe {
            let ch = hwnd(chuzhoe);
            let potok_dvizhka = GetWindowThreadProcessId(ch, None);
            let nash = GetCurrentThreadId();
            if potok_dvizhka != 0 && potok_dvizhka != nash {
                let _ = AttachThreadInput(nash, potok_dvizhka, BOOL(1));
            }
        }
    }

    /// Открепить очередь ввода — при остановке/удалении вкладки.
    pub fn otsoedinit_vvod(chuzhoe: u32) {
        use windows::Win32::Foundation::BOOL;
        use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
        unsafe {
            let ch = hwnd(chuzhoe);
            let potok_dvizhka = GetWindowThreadProcessId(ch, None);
            let nash = GetCurrentThreadId();
            if potok_dvizhka != 0 && potok_dvizhka != nash {
                let _ = AttachThreadInput(nash, potok_dvizhka, BOOL(0));
            }
        }
    }

    /// Отдать клавиатуру встроенному окну. Работает, только когда очереди
    /// прикреплены (см. prisoedinit_vvod) и вызов идёт с нашего UI-потока.
    pub fn fokus_dvizhku(chuzhoe: u32) {
        use windows::Win32::UI::Input::KeyboardAndMouse::SetFocus;
        unsafe {
            let _ = SetFocus(hwnd(chuzhoe));
        }
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

    /// Живо ли встроенное окно и осталось ли нашим потомком: Chromium
    /// иногда ПЕРЕСОЗДАЁТ своё окно — новое рождается чужим, с рамкой
    /// и своей жизнью (10.11.2026, поймал user).
    pub fn nash_li(chuzhoe: u32, nashe: u64) -> bool {
        unsafe {
            let ch = hwnd(chuzhoe);
            if !IsWindow(ch).as_bool() {
                return false;
            }
            let Ok(roditel) = GetParent(ch) else {
                return false;
            };
            roditel.0 == nashe as usize as *mut core::ffi::c_void
        }
    }

    /// Вежливо попросить чужое окно закрыться (как клик по крестику):
    /// Chromium так сохраняет сессию и не считает это крахом. Без этого
    /// после убийства процесса он лез с «восстановить сессию».
    pub fn zakryt_vezhlivo(chuzhoe: u32) {
        use windows::Win32::Foundation::LRESULT;
        unsafe {
            let mut otvet: usize = 0;
            let _ = SendMessageTimeoutW(
                hwnd(chuzhoe),
                WM_CLOSE,
                WPARAM(0),
                LPARAM(0),
                SMTO_ABORTIFHUNG | SMTO_NOTIMEOUTIFNOTHUNG,
                3000,
                Some(std::ptr::from_mut(&mut otvet)),
            );
        }
    }

    /// Кто сейчас впереди (для диагностики): pid и заголовок.
    pub fn kto_v_peredne() -> String {
        unsafe {
            let fg = GetForegroundWindow();
            if fg.0.is_null() {
                return "нет".into();
            }
            let mut pid = 0u32;
            GetWindowThreadProcessId(fg, Some(&mut pid));
            let mut zaglavie = [0u16; 256];
            let n = GetWindowTextW(fg, &mut zaglavie);
            format!("pid {pid} «{}»", String::from_utf16_lossy(&zaglavie[..n as usize]))
        }
    }

    /// Pid процесса, владеющего окном: когда Chromium перезапускает
    /// себя (уходит из job object), наш дочерний процесс мёртв, а окно
    /// живёт у нового pid (10.11.2026, пойман сменой pid на машине).
    pub fn pid_okna(chuzhoe: u32) -> Option<u32> {
        unsafe {
            let mut pid = 0u32;
            GetWindowThreadProcessId(hwnd(chuzhoe), Some(&mut pid));
            (pid != 0).then_some(pid)
        }
    }

    /// Окно ещё на месте (владелец не уничтожил его).
    pub fn okno_zhivo(chuzhoe: u32) -> bool {
        unsafe { IsWindow(hwnd(chuzhoe)).as_bool() }
    }

    /// Жив ли процесс. Движок на Windows перезапускает себя: наш
    /// дочерний процесс мёртв, а браузер живёт своим pid — живость
    /// проверяем по нему (12.11.2026).
    pub fn pid_zhiv(pid: u32) -> bool {
        use windows::Win32::System::Threading::{
            GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
        };
        unsafe {
            let Ok(h) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
                return false;
            };
            let mut kod = 0u32;
            let ok = GetExitCodeProcess(h, &mut kod).is_ok();
            let _ = CloseHandle(h);
            ok && kod == 259 // STILL_ACTIVE
        }
    }

    /// Добить процесс по pid (запасной движок, ушедший из job object).
    pub fn ubit_po_pid(pid: u32) {
        use windows::Win32::System::Threading::{OpenProcess, TerminateProcess, PROCESS_TERMINATE};
        unsafe {
            if let Ok(h) = OpenProcess(PROCESS_TERMINATE, false, pid) {
                let _ = TerminateProcess(h, 1);
                let _ = CloseHandle(h);
            }
        }
    }

    /// Спрятать ВСЕ окна движков — не только известные нам хэндлы.
    /// Chromium пересоздаёт окно, и старый хэндл его уже не видит:
    /// незнакомое окно лежит поверх и ест клики (10.11.2026, поймал
    /// user: «кнопки Сохранить/Не сохранять/Отмена не работают»).
    pub fn spryatat_chuzhie(korni: &[u32]) {
        if korni.is_empty() {
            return;
        }
        let vse: std::collections::HashSet<u32> = korni
            .iter()
            .flat_map(|&k| derevo_processov(k))
            .collect();
        unsafe {
            struct P { pids: std::collections::HashSet<u32> }
            unsafe extern "system" fn proverit(hwnd: HWND, lparam: LPARAM) -> BOOL {
                let p = &mut *(lparam.0 as *mut P);
                let mut pid = 0u32;
                GetWindowThreadProcessId(hwnd, Some(&mut pid));
                if p.pids.contains(&pid) {
                    let _ = ShowWindow(hwnd, SW_HIDE);
                }
                BOOL(1)
            }
            let mut p = P { pids: vse };
            let _ = EnumWindows(Some(proverit), LPARAM(&mut p as *mut _ as isize));
        }
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
    pub fn zakryt_vezhlivo(_c: u32) {}
    pub fn nash_li(_c: u32, _n: u64) -> bool {
        true
    }
    pub fn kto_v_peredne() -> String {
        String::new()
    }
    pub fn pid_okna(_c: u32) -> Option<u32> {
        None
    }
    pub fn okno_zhivo(_c: u32) -> bool {
        false
    }
    pub fn pid_zhiv(_p: u32) -> bool {
        false
    }
    pub fn ubit_po_pid(_p: u32) {}
    pub fn spryatat_chuzhie(_k: &[u32]) {}
    pub fn vse_okna(_korni: &[u32]) -> Vec<u32> {
        Vec::new()
    }
    pub fn prisoedinit_vvod(_c: u32) {}
    pub fn otsoedinit_vvod(_c: u32) {}
    pub fn fokus_dvizhku(_c: u32) {}
}
