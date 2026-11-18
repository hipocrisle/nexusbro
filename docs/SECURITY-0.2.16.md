# NexusBro: passwords and isolated sync

Implementation contracts: contracts/SECURITY-SYNC.md. Separate browser-only vault format, keys, accounts, SQLite database, service user, environment, health marker and protocol namespace. NexuSSH is only a reviewed MIT source reference, never a live backend. A foreign SQLite database is rejected before opening/chmod/WAL changes.

Local vault: random DEK envelope; bounded Argon2id and XChaCha20-Poly1305; authenticated version/header; atomic ciphertext writes and encrypted backup; zeroized key/value buffers and inactivity lock. Ordinary lists contain credential summaries, never passwords. Secrets are revealed only on explicit owner action. The local master password and account password are separate.

CEF: native DevTools responses/binding events, not console messages. Main-frame isolated world; unique execution-context identifiers; origin and top-frame guards; private contexts excluded. Saving requires consent, filling requires explicit selection and exact Super+origin. Pending form values expire and clear on lock. No automatic form submission. The native acceptance test must scan synthetic passwords out of persistent logs/ordinary files and verify actual form fields.

Sync: all categories off by default. Settings are a portable allowlist; bookmarks per-record; Super and password records selected individually or explicit all/new wildcard. Proxy/VPN credentials, cookies, HTTP cache, history, downloads, live sessions, private tabs and private Super definitions are excluded. Existing device-local network settings remain local. Cloud payloads use per-item AEAD bound to the full namespaced ID. Tokens/remembered keys are stored inside the vault.

Per-account selection/tracking is bound to server+user ID. Disabling sync does not delete records elsewhere. Deletion propagation requires an explicit durable owner intent; missing/corrupt files do not generate cloud deletions. Intent generations cannot be consumed by an older in-flight snapshot. Imports use freshness checks and native-storage CAS/atomic portable patching. Conflicts retain both versions and require a choice; stale choices fail without reverting fresh edits.

CI gates: standalone production crypto/vault modules against an actual isolated server and two clients; native Windows menus must keep CEF visible; native Linux acceptance launches two real Tauri+CEF apps with independent D-Bus/Xvfb/data directories and a real ephemeral server. Fixture-only Chromium UI checks are supplementary.

Deployment: `sync-server/README.md` and `sync-server/deploy/`. Backend defaults to an unconfigured client endpoint; no production availability is claimed until a separate instance is deployed and verified. No production deployment/service/proxy modifications were performed as part of implementation. A GitHub-built backend package is prepared separately for operator deployment.
