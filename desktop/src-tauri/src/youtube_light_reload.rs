//! Bounded native reload intent, not a player admission/capability store.
//! RAM only: no signed URLs, media metadata, serialization or Debug history.
use crate::youtube_light::VideoId;
use std::collections::HashMap;
const CAP: usize = 128;
struct Hint {
    document: String,
    video: VideoId,
}
#[derive(Default)]
pub struct ReloadHints {
    hints: HashMap<i32, Hint>,
}
fn internal_document(document: &str) -> bool {
    document
        .strip_prefix("nb-lite://player/")
        .and_then(|s| s.strip_suffix("/player.html"))
        .is_some_and(|nonce| {
            nonce.len() == 48
                && nonce
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        })
}
impl ReloadHints {
    pub fn remember(&mut self, browser: i32, document: &str, video: VideoId) -> bool {
        if browser <= 0
            || !internal_document(document)
            || (!self.hints.contains_key(&browser) && self.hints.len() >= CAP)
        {
            return false;
        }
        self.hints.insert(
            browser,
            Hint {
                document: document.into(),
                video,
            },
        );
        true
    }
    pub fn video(&self, browser: i32, actual_document: &str) -> Option<VideoId> {
        self.hints
            .get(&browser)
            .filter(|hint| hint.document == actual_document)
            .map(|hint| hint.video.clone())
    }
    pub fn navigated(&mut self, browser: i32, target: &str) {
        if self
            .hints
            .get(&browser)
            .is_some_and(|hint| hint.document != target)
        {
            self.forget(browser);
        }
    }
    pub fn forget(&mut self, browser: i32) {
        self.hints.remove(&browser);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn document(n: usize) -> String {
        format!("nb-lite://player/{n:048x}/player.html")
    }
    fn video() -> VideoId {
        VideoId::parse("BaW_jenozKc").unwrap()
    }
    #[test]
    fn exact_document_and_browser_only_navigation_or_close_revokes_hint() {
        let mut hints = ReloadHints::default();
        let doc = document(1);
        assert!(hints.remember(1, &doc, video()));
        assert_eq!(hints.video(1, &doc), Some(video()));
        assert!(hints.video(2, &doc).is_none());
        assert!(hints.video(1, &document(2)).is_none());
        hints.navigated(1, &doc);
        assert!(hints.video(1, &doc).is_some());
        hints.navigated(1, "about:blank");
        assert!(hints.video(1, &doc).is_none());
        assert!(hints.remember(1, &doc, video()));
        hints.forget(1);
        assert!(hints.video(1, &doc).is_none());
    }
    #[test]
    fn bounded_replacement_never_retains_prior_capability_document() {
        let mut hints = ReloadHints::default();
        for n in 1..=CAP {
            assert!(hints.remember(n as i32, &document(n), video()));
        }
        assert!(!hints.remember(999, &document(999), video()));
        assert!(hints.remember(1, &document(1000), video()));
        assert!(hints.video(1, &document(1)).is_none());
        assert!(hints.video(1, &document(1000)).is_some());
        assert!(!hints.remember(1, "https://private.invalid/token", video()));
        assert!(!hints.remember(0, &document(1), video()));
        hints.forget(1);
        assert!(hints.remember(999, &document(999), video()));
    }
}
