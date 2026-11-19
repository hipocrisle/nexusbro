//! Run on GitHub runners, never compile Rust on the DE-1 transit machine.
//! cargo test --manifest-path tests/security_vault_harness/Cargo.toml
#[path = "../../../desktop/src-tauri/src/secure_vault.rs"]
mod secure_vault;

#[path = "../../../desktop/src-tauri/src/cef_events.rs"]
mod cef_events;
