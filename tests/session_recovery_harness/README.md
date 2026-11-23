# Session recovery verification

`cargo test --manifest-path tests/session_recovery_harness/Cargo.toml`

This small harness imports the real session, route intent and window recovery storage modules. It does not build CEF/Tauri. Unrelated command-payload `password_cef::Secret` and private-ID predicate have minimal stubs.

Checks actual filesystem writes, validated previous-snapshot fallback after truncation, refusal to promote uncommitted temporary files, failed-publication cleanup, ordinary route intent/lazy restore, privacy sanitization of both primary and backup, independent window sessions and explicit-close empty snapshots.

This is not native application acceptance. The final build must test abrupt process termination and real window/tab/route restoration on Windows and Linux. Additional-window storage is inactive until the parent integrates `mod window_recovery;` and the lifecycle hooks. The store itself never starts an engine.

Integration contract:

- `window_recovery::save(root, Vec<WindowSession>)` saves the complete current set. Capture active `Okno.sessiya` and `Okno.others` as `SuperSession`s. Exclude private windows AND definitions marked private (legacy IDs need not have a private prefix).
- Serialize saves from a consistent current window-map snapshot; keep pending restore entries until their shell/CEF views have attached. Explicit close removes that window from the saved collection. App-wide exit must retain open windows.
- `window_recovery::load(root)` returns sanitized, lazy sessions with no native handles/runtime route ACKs. Validate every referenced Super against current definitions before using it.
- Seed the new ordinary window's session and cached views before creating its WebView, so its initial `okno_gotovo` cannot replace restored tabs with a fresh blank session. A fresh runtime label may differ from the persisted key.
- Restore only after vault unlock. Preserve independent window sessions and effective route choices; do not call ordinary Super creation or copy cookies/auth between profiles.
