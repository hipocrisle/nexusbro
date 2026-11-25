//! Normal application quit, separate from hibernation/cookie escrow.
//! Parent keeps ownership/proxies alive through exact ACK AND native tree exit.
//! Engine preserves saved tabs, drains browsers/helpers, drops CEF references,
//! calls CefShutdown, and only then emits this ACK on the existing native pipe.
//! Forced fallback is never a clean acknowledgement.
use rand_core::{OsRng, RngCore};
use serde::{Deserialize, Serialize};

pub const ACK_TIP: &str = "app-shutdown-ack";
pub const MAX_WIRE_BYTES: usize = 512;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub nonce: String,
    pub engine: u64,
}
impl Request {
    pub fn fresh(engine: u64) -> Result<Self, &'static str> {
        if engine == 0 { return Err("shutdown_generation_invalid"); }
        let mut bytes = [0u8; 16];
        OsRng.try_fill_bytes(&mut bytes).map_err(|_| "shutdown_nonce_unavailable")?;
        Ok(Self { nonce: bytes.iter().map(|b| format!("{b:02x}")).collect(), engine })
    }
    pub fn valid(&self) -> bool {
        self.engine != 0 && self.nonce.len() == 32
            && self.nonce.bytes().all(|b| b.is_ascii_hexdigit())
    }
}
impl std::fmt::Debug for Request {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ShutdownRequest(<redacted>)")
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Ack {
    pub request: Request,
    pub browsers_closed: bool,
    pub helpers_reaped: bool,
    pub cef_shutdown_completed: bool,
}
impl Ack {
    /// Caller must already have drained browsers/helpers and returned from CEF.
    pub fn after_cef_shutdown(request: Request) -> Self {
        Self { request, browsers_closed: true, helpers_reaped: true, cef_shutdown_completed: true }
    }
    pub fn matches(&self, expected: &Request) -> bool {
        expected.valid() && self.request.valid() && self.request == *expected
            && self.browsers_closed && self.helpers_reaped && self.cef_shutdown_completed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn request() -> Request { Request { nonce: "ab".repeat(16), engine: 4 } }
    #[test]
    fn stale_partial_and_cross_engine_acknowledgements_are_refused() {
        let expected=request(); let mut ack=Ack::after_cef_shutdown(expected.clone());
        assert!(ack.matches(&expected));
        ack.request.engine=5; assert!(!ack.matches(&expected));
        ack.request=expected.clone(); ack.request.nonce="cd".repeat(16); assert!(!ack.matches(&expected));
        ack.request=expected.clone(); ack.helpers_reaped=false; assert!(!ack.matches(&expected));
        ack.helpers_reaped=true; ack.cef_shutdown_completed=false; assert!(!ack.matches(&expected));
        ack.cef_shutdown_completed=true; ack.browsers_closed=false; assert!(!ack.matches(&expected));
    }
    #[test]
    fn wire_is_bounded_and_nonce_is_not_debugged() {
        let req=request(); assert!(req.valid());
        assert!(!format!("{req:?}").contains(&req.nonce));
        assert!(serde_json::to_vec(&Ack::after_cef_shutdown(req)).unwrap().len() < MAX_WIRE_BYTES);
        assert!(serde_json::from_str::<Request>(r#"{"nonce":"x","engine":1,"other":true}"#).is_err());
        assert!(!Request{nonce:"not-a-nonce".into(),engine:4}.valid());
        assert!(!Request{nonce:"ab".repeat(16),engine:0}.valid());
    }
}
