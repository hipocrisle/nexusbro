//! CI-only observations. Never changes focus, input, visibility, or navigation.
use crate::cef_vkladki::Sobytie;
use cef::Browser;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    mpsc::Sender,
};

static EMITTED: AtomicUsize = AtomicUsize::new(0);
const LIMIT: usize = 32;

#[derive(Clone, Copy)]
pub(crate) enum Stage {
    System,
    Navigation,
    UnknownSource,
    Got,
    TakeForward,
    TakeReverse,
}
impl Stage {
    fn name(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Navigation => "navigation",
            Self::UnknownSource => "unknown_source",
            Self::Got => "got",
            Self::TakeForward => "take_forward",
            Self::TakeReverse => "take_reverse",
        }
    }
}
fn enabled() -> bool {
    std::env::var("GITHUB_ACTIONS").as_deref() == Ok("true")
        && std::env::var_os("NEXUSBRO_TEST_DATA").is_some()
}
fn claim(counter: &AtomicUsize) -> bool {
    counter
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| {
            (n < LIMIT).then_some(n + 1)
        })
        .is_ok()
}

pub(crate) fn trace(id: u64, browser: Option<&Browser>, sender: &Sender<Sobytie>, stage: Stage) {
    if !enabled() || !claim(&EMITTED) {
        return;
    }
    // No mutex/RefCell is held by this module during CEF or X11 calls.
    let role = focus_role(browser);
    let _ = sender.send(Sobytie::Sluzhebnoe(serde_json::json!({
        "tip": "cef-focus-trace", "id": id, "stage": stage.name(), "role": role,
    })));
}

// These names deliberately describe topology, NOT authority: this callback
// cannot establish that its Browser is selected, or that a sibling is WebKit.
fn topology(focus: u32, browser: u32, parent: u32, ancestors: &[u32]) -> &'static str {
    if focus <= 1 {
        "none"
    } else if focus == browser {
        "browser_window"
    } else if ancestors.contains(&browser) {
        "browser_descendant"
    } else if focus == parent {
        "browser_parent"
    } else if parent > 1 && ancestors.contains(&parent) {
        "parent_descendant"
    } else {
        "other"
    }
}

#[cfg(target_os = "linux")]
fn focus_role(browser: Option<&Browser>) -> &'static str {
    use cef::*;
    use x11rb::protocol::xproto::ConnectionExt;
    let Some(browser) = browser else {
        return "unknown";
    };
    let Some(host) = browser.host() else {
        return "unknown";
    };
    let window = host.window_handle() as u32;
    if window <= 1 {
        return "unknown";
    }
    // A short-lived read-only connection; never selects events or grabs input.
    let Ok((connection, _)) = x11rb::connect(None) else {
        return "unknown";
    };
    let Ok(cookie) = connection.get_input_focus() else {
        return "unknown";
    };
    let Ok(reply) = cookie.reply() else {
        return "unknown";
    };
    let focus = reply.focus;
    if focus <= 1 {
        return "none";
    }
    let Ok(cookie) = connection.query_tree(window) else {
        return "unknown";
    };
    let Ok(tree) = cookie.reply() else {
        return "unknown";
    };
    let parent = tree.parent;
    let mut ancestors = Vec::with_capacity(16);
    let mut next = focus;
    // Bounded ancestry; an inaccessible or deeper tree is unknown, not shell.
    for _ in 0..16 {
        if next == window || next == parent {
            return topology(focus, window, parent, &ancestors);
        }
        let Ok(cookie) = connection.query_tree(next) else {
            return "unknown";
        };
        let Ok(node) = cookie.reply() else {
            return "unknown";
        };
        if node.parent == 0 {
            return topology(focus, window, parent, &ancestors);
        }
        if node.parent == next || ancestors.contains(&node.parent) {
            return "unknown";
        }
        next = node.parent;
        ancestors.push(next);
    }
    "unknown"
}
#[cfg(not(target_os = "linux"))]
fn focus_role(_browser: Option<&Browser>) -> &'static str {
    "unsupported"
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn callback_names_are_fixed() {
        assert_eq!(Stage::System.name(), "system");
        assert_eq!(Stage::Navigation.name(), "navigation");
        assert_eq!(Stage::UnknownSource.name(), "unknown_source");
        assert_eq!(Stage::Got.name(), "got");
        assert_eq!(Stage::TakeForward.name(), "take_forward");
        assert_eq!(Stage::TakeReverse.name(), "take_reverse");
    }
    #[test]
    fn process_budget_is_saturating() {
        let count = AtomicUsize::new(0);
        for _ in 0..LIMIT {
            assert!(claim(&count));
        }
        for _ in 0..100 {
            assert!(!claim(&count));
        }
        assert_eq!(count.load(Ordering::Relaxed), LIMIT);
    }
    #[test]
    fn topology_does_not_call_siblings_shell_or_selected() {
        assert_eq!(topology(20, 20, 10, &[]), "browser_window");
        assert_eq!(topology(21, 20, 10, &[20]), "browser_descendant");
        assert_eq!(topology(10, 20, 10, &[]), "browser_parent");
        assert_eq!(topology(30, 20, 10, &[10]), "parent_descendant");
        assert_eq!(topology(30, 20, 10, &[40]), "other");
        assert_eq!(topology(1, 20, 10, &[]), "none");
    }
}
