//! IO-thread request privacy for native committed internal-player media only.
//! No signed URLs retained in scopes, serialized, logged or granted by pages.
use cef::*;
use zeroize::Zeroizing;
#[derive(Clone)]
pub struct Scope(Kind);
#[derive(Clone)]
enum Kind {
    Normal,
    Blocked,
    Player {
        browser: i32,
        frame: String,
        operation: u64,
    },
}
fn native_identity(
    browser: Option<&Browser>,
    frame: Option<&Frame>,
) -> Option<(i32, String, String)> {
    let (browser, frame) = (browser?, frame?);
    if browser.is_valid() == 0 || frame.is_main() == 0 {
        return None;
    }
    let id = CefString::from(&frame.identifier()).to_string();
    if id.is_empty() || id.len() > 256 {
        return None;
    }
    Some((
        browser.identifier(),
        id,
        CefString::from(&frame.url()).to_string(),
    ))
}
impl Scope {
    pub fn capture(browser: Option<&Browser>, frame: Option<&Frame>, initiator: &str) -> Self {
        let mut internal_document = false;
        if let Some((browser, frame, document)) = native_identity(browser, frame) {
            internal_document = document.starts_with("nb-lite://player/");
            if let Some(operation) = crate::cef_youtube::privacy_binding(browser, &frame, &document)
            {
                return Self(Kind::Player {
                    browser,
                    frame,
                    operation,
                });
            }
        }
        Self(if initiator == "nb-lite://player" || internal_document {
            Kind::Blocked
        } else {
            Kind::Normal
        })
    }
}
fn anonymous_flags(old: i32) -> i32 {
    use cef::sys::cef_urlrequest_flags_t as F;
    (old & !(F::UR_FLAG_ALLOW_STORED_CREDENTIALS.0 as i32 | F::UR_FLAG_ONLY_FROM_CACHE.0 as i32))
        | F::UR_FLAG_DISABLE_CACHE.0 as i32
}
fn secret_header(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "cookie" | "authorization" | "proxy-authorization"
    )
}
/// Invoke BEFORE route/ad-block admission. CONTINUE only means this privacy
/// guard permits proceeding; it never overrides the existing route gate.
pub fn before_resource(
    scope: &Scope,
    browser: Option<&Browser>,
    frame: Option<&Frame>,
    request: Option<&Request>,
) -> ReturnValue {
    if matches!(&scope.0, Kind::Normal) {
        return ReturnValue::CONTINUE;
    }
    let Some(request) = request else {
        return ReturnValue::CANCEL;
    };
    let url = Zeroizing::new(CefString::from(&request.url()).to_string());
    // These public resources have their own nonce/browser/main-frame factory.
    if url.starts_with("nb-lite://player/") {
        return ReturnValue::CONTINUE;
    }
    let Kind::Player {
        browser: expected,
        frame: expected_frame,
        operation,
    } = &scope.0
    else {
        return ReturnValue::CANCEL;
    };
    let Some((browser, frame, document)) = native_identity(browser, frame) else {
        return ReturnValue::CANCEL;
    };
    if browser != *expected
        || frame != *expected_frame
        || crate::cef_youtube::privacy_binding(browser, &frame, &document) != Some(*operation)
        || !crate::cef_youtube::privacy_media_url(browser, &frame, *operation, &url)
        || request.is_read_only() != 0
    {
        return ReturnValue::CANCEL;
    }
    let flags = anonymous_flags(request.flags());
    request.set_flags(flags);
    if request.flags() != flags {
        return ReturnValue::CANCEL;
    }
    let mut original = CefStringMultimap::new();
    request.header_map(Some(&mut original));
    let mut clean = CefStringMultimap::new();
    let mut count = 0;
    let mut bytes = 0;
    for (name, values) in original {
        for value in values {
            let value = Zeroizing::new(value);
            count += 1;
            bytes += name.len() + value.len();
            if count > 128 || bytes > 65536 {
                return ReturnValue::CANCEL;
            }
            if secret_header(&name)
                || name.eq_ignore_ascii_case("cache-control")
                || name.eq_ignore_ascii_case("pragma")
            {
                continue;
            }
            if !clean.append(&name, &value) {
                return ReturnValue::CANCEL;
            }
        }
    }
    if !clean.append("Cache-Control", "no-store") || !clean.append("Pragma", "no-cache") {
        return ReturnValue::CANCEL;
    }
    request.set_header_map(Some(&mut clean));
    for name in ["Cookie", "Authorization", "Proxy-Authorization"] {
        let value = Zeroizing::new(
            CefString::from(&request.header_by_name(Some(&CefString::from(name)))).to_string(),
        );
        if !value.is_empty() {
            return ReturnValue::CANCEL;
        }
    }
    if CefString::from(&request.header_by_name(Some(&CefString::from("Cache-Control")))).to_string()
        != "no-store"
        || CefString::from(&request.header_by_name(Some(&CefString::from("Pragma")))).to_string()
            != "no-cache"
    {
        return ReturnValue::CANCEL;
    }
    if crate::cef_youtube::privacy_binding(browser, &frame, &document) != Some(*operation) {
        return ReturnValue::CANCEL;
    }
    ReturnValue::CONTINUE
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn flags_disable_storage_not_just_validation_and_remove_credentials() {
        use cef::sys::cef_urlrequest_flags_t as F;
        let input = F::UR_FLAG_ONLY_FROM_CACHE.0 as i32
            | F::UR_FLAG_ALLOW_STORED_CREDENTIALS.0 as i32
            | F::UR_FLAG_REPORT_UPLOAD_PROGRESS.0 as i32;
        let output = anonymous_flags(input);
        assert_ne!(output & F::UR_FLAG_DISABLE_CACHE.0 as i32, 0);
        assert_eq!(output & F::UR_FLAG_ONLY_FROM_CACHE.0 as i32, 0);
        assert_eq!(output & F::UR_FLAG_ALLOW_STORED_CREDENTIALS.0 as i32, 0);
        assert_ne!(output & F::UR_FLAG_REPORT_UPLOAD_PROGRESS.0 as i32, 0);
    }
    #[test]
    fn stale_player_fails_closed_normal_request_unchanged() {
        let stale = Scope::capture(None, None, "nb-lite://player");
        assert_eq!(
            before_resource(&stale, None, None, None),
            ReturnValue::CANCEL
        );
        let normal = Scope::capture(None, None, "https://www.youtube.com");
        assert_eq!(
            before_resource(&normal, None, None, None),
            ReturnValue::CONTINUE
        );
        assert!(secret_header("cOoKiE"));
        assert!(secret_header("AUTHORIZATION"));
        assert!(secret_header("Proxy-Authorization"));
        assert!(!secret_header("Range"));
    }
}
