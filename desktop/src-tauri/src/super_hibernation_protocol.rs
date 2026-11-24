//! Private parent/CEF hibernation frames. Never persisted as ordinary session/UI.
use crate::super_cookie_escrow::{Scope, Sealed};
use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

#[derive(Clone, Serialize, Deserialize)]
#[serde(transparent)]
pub struct WireKey(String);
impl std::fmt::Debug for WireKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("WireKey([redacted])")
    }
}
impl Drop for WireKey {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}
impl WireKey {
    pub fn new(value: String) -> Self {
        Self(value)
    }
    pub fn expose(&self) -> &str {
        &self.0
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Prepare {
    pub scope: Scope,
    pub revision: u64,
    pub key: WireKey,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Token {
    pub generation: u64,
    pub attempt: u64,
    pub revision: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scroll {
    pub id: u64,
    pub route_generation: u64,
    pub x: f64,
    pub y: f64,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    pub token: Token,
    pub scope: Scope,
    pub sealed: Sealed,
    pub session: crate::sessiya::Sessiya,
    pub scroll: Vec<Scroll>,
}
impl std::fmt::Debug for Snapshot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Snapshot([redacted])")
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Restore {
    pub key: WireKey,
    pub snapshot: Snapshot,
}
impl std::fmt::Debug for Restore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Restore([redacted])")
    }
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Frame {
    Prepared { snapshot: Snapshot },
    FinalSnapshot { snapshot: Snapshot },
    Refused { token: Token, reason: String },
    AbortAck { token: Token },
    ShutdownAck { token: Token },
    Restored { generation: u64 },
    RestoreRefused { generation: u64 },
}
