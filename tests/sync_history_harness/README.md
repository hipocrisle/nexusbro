# Local encrypted sync history verification

Run on a GitHub runner: `cargo test --manifest-path tests/sync_history_harness/Cargo.toml`.

This uses the production bounded history implementation. It checks count/byte eviction,
account isolation, preservation of the previous route, retry deduplication and rejection
of corruption/oversized writes without discarding existing records.

These unit checks do not prove native undo. The integration suite must additionally
perform real cloud overwrite/delete, restart, explicit preview/restore, stale-preview
rejection, route preservation, no engine startup, and subsequent server exchange.
No user credentials or profile data belong in test fixtures.
