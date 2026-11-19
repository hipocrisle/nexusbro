//! Match native Alloy visibility to the actual tab/window visibility.
//!
//! CEF's public CefBrowserWindow HWND wraps a separate Aura/Views Widget.
//! Hiding an ancestor does not change that Widget's own WS_VISIBLE flag, so
//! Chromium may continue treating the document as foreground. Only operate on
//! the single verified Widget child, never its renderer descendants.
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicUsize, Ordering};
use windows::core::w;
use windows::Win32::{
    Foundation::{HANDLE, HWND},
    System::Threading::GetCurrentThreadId,
    UI::WindowsAndMessaging::{
        GetAncestor, GetClassNameW, GetParent, GetPropW, GetWindow, GetWindowLongPtrW,
        GetWindowThreadProcessId, IsIconic, IsWindow, IsWindowVisible, RemovePropW, SetPropW,
        ShowWindow, GA_ROOT, GWL_STYLE, GW_CHILD, GW_HWNDNEXT, SW_HIDE, SW_SHOWNOACTIVATE,
        WS_CHILD, WS_VISIBLE,
    },
};

#[derive(Default)]
pub(crate) struct Visibility {
    entries: HashMap<i32, Entry>,
    diagnostics: u8,
}

struct Entry {
    public: HWND,
    widget: HWND,
    container: HWND,
    generation: usize,
    // Restore only the WS_VISIBLE bit that this helper itself removed. Never
    // restore an old full window style or make a CEF-hidden window visible.
    hidden_by_us: bool,
}

static NEXT_GENERATION: AtomicUsize = AtomicUsize::new(1);

impl Drop for Entry {
    fn drop(&mut self) {
        unsafe {
            // HWND properties disappear when the native window is destroyed.
            // A reused numeric HWND cannot carry this old lifetime marker.
            if GetPropW(self.widget, w!("NexusBro.CefVisibility.Generation")).0 as usize
                == self.generation
            {
                let _ = RemovePropW(self.widget, w!("NexusBro.CefVisibility.Generation"));
            }
        }
    }
}

fn hwnd(value: u64) -> HWND {
    HWND(value as usize as *mut core::ffi::c_void)
}

unsafe fn owned_ui_window(window: HWND, thread: u32) -> bool {
    if !IsWindow(window).as_bool() {
        return false;
    }
    let mut pid = 0;
    GetWindowThreadProcessId(window, Some(&mut pid)) == thread && pid == std::process::id()
}

unsafe fn widget_matches(window: HWND, public: HWND, thread: u32) -> bool {
    if !owned_ui_window(window, thread)
        || GetParent(window).ok() != Some(public)
        || GetWindowLongPtrW(window, GWL_STYLE) as u32 & WS_CHILD.0 == 0
    {
        return false;
    }
    let mut name = [0u16; 96];
    let size = GetClassNameW(window, &mut name);
    // Chromium appends a registration counter, not a fixed zero. The native
    // Windows probe observes Chrome_WidgetWin_1 for the bundled CEF 156.
    // Keep the structural/owner checks above and accept only a numeric suffix;
    // renderer HWND classes and unrelated children never match this prefix.
    let prefix = "Chrome_WidgetWin_";
    let size = size.max(0) as usize;
    size > prefix.len()
        && prefix.encode_utf16().eq(name[..prefix.len()].iter().copied())
        && name[prefix.len()..size]
            .iter()
            .all(|c| (b'0' as u16..=b'9' as u16).contains(c))
}

unsafe fn find_widget(public: HWND, thread: u32) -> Option<HWND> {
    let mut child = GetWindow(public, GW_CHILD).ok();
    let mut found = None;
    let mut examined = 0;
    while let Some(window) = child {
        examined += 1;
        if examined > 64 {
            return None;
        }
        if widget_matches(window, public, thread) {
            if found.is_some() {
                return None;
            }
            found = Some(window);
        }
        child = GetWindow(window, GW_HWNDNEXT).ok();
    }
    found
}

impl Visibility {
    fn diagnostic(&mut self, reason: &str) {
        if self.diagnostics < 8 {
            eprintln!("cef: native visibility: {reason}");
            self.diagnostics += 1;
        }
    }

    /// Caller is the owning CEF UI thread; inputs contain no page data. The
    /// desired flag includes model activity, lock and closing checks. Native
    /// ancestry is rechecked here before *every* possible restore.
    pub(crate) fn sync(&mut self, container: u64, windows: &[(i32, u64, bool)]) {
        let live: HashSet<_> = windows.iter().map(|(id, _, _)| *id).collect();
        self.entries.retain(|id, _| live.contains(id));
        let container = hwnd(container);
        unsafe {
            let thread = GetCurrentThreadId();
            if !owned_ui_window(container, thread) {
                self.entries.clear();
                self.diagnostic("container identity changed; skipped");
                return;
            }
            for &(id, public, desired) in windows {
                let public = hwnd(public);
                if !owned_ui_window(public, thread) || GetParent(public).ok() != Some(container) {
                    self.entries.remove(&id);
                    self.diagnostic("browser parent/owner changed; skipped");
                    continue;
                }
                let valid = self.entries.get(&id).is_some_and(|entry| {
                    entry.public == public
                        && entry.container == container
                        && widget_matches(entry.widget, public, thread)
                        && GetPropW(entry.widget, w!("NexusBro.CefVisibility.Generation")).0
                            as usize
                            == entry.generation
                });
                if !valid {
                    // A destroyed/replaced HWND must never inherit restoration
                    // ownership from an old browser/widget generation.
                    self.entries.remove(&id);
                    let Some(widget) = find_widget(public, thread) else {
                        self.diagnostic("unique native Widget not ready; skipped");
                        continue;
                    };
                    let Ok(generation) =
                        NEXT_GENERATION.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| {
                            n.checked_add(1).filter(|v| *v <= isize::MAX as usize)
                        })
                    else {
                        continue;
                    };
                    if !GetPropW(widget, w!("NexusBro.CefVisibility.Generation"))
                        .0
                        .is_null()
                        || SetPropW(
                            widget,
                            w!("NexusBro.CefVisibility.Generation"),
                            HANDLE(generation as *mut core::ffi::c_void),
                        )
                        .is_err()
                    {
                        self.diagnostic("Widget lifetime marker unavailable; skipped");
                        continue;
                    }
                    self.entries.insert(
                        id,
                        Entry {
                            public,
                            widget,
                            container,
                            generation,
                            hidden_by_us: false,
                        },
                    );
                }
                let entry = self.entries.get_mut(&id).unwrap();
                let root = GetAncestor(container, GA_ROOT);
                let visible = desired
                    && !root.0.is_null()
                    && IsWindowVisible(container).as_bool()
                    && IsWindowVisible(public).as_bool()
                    && !IsIconic(root).as_bool();
                let own_visible =
                    GetWindowLongPtrW(entry.widget, GWL_STYLE) as u32 & WS_VISIBLE.0 != 0;
                if !visible && own_visible {
                    // Same-thread ShowWindow synchronously delivers the Widget's
                    // visibility notification. Never call from Tauri/worker UI.
                    let _ = ShowWindow(entry.widget, SW_HIDE);
                    entry.hidden_by_us =
                        GetWindowLongPtrW(entry.widget, GWL_STYLE) as u32 & WS_VISIBLE.0 == 0;
                } else if visible && entry.hidden_by_us {
                    let _ = ShowWindow(entry.widget, SW_SHOWNOACTIVATE);
                    if GetWindowLongPtrW(entry.widget, GWL_STYLE) as u32 & WS_VISIBLE.0 != 0 {
                        entry.hidden_by_us = false;
                    }
                }
            }
        }
    }
}
