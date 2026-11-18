# NexusBro isolated encrypted sync backend

This is a separate application, copied and adapted from the MIT-licensed NexuSSH source (copyright 2026 hipocrisle). It has no AI, bot, SSH, VPN or media routes/dependencies. It must have its own process, DB, hostname, TLS endpoint and deployment key. Do not connect it to the NexuSSH DB or share tokens/keys/accounts. No deployment is performed by this repository.

Browser clients start with **no configured endpoint**. A configured endpoint must use HTTPS and return `{"service":"nexusbro-sync","protocol":1}` at `/v1/health`; a legacy SSH sync server is rejected. Only loopback under explicit test mode can use HTTP. The example hostname below is a placeholder, not a live service.

## Contract and safeguards

- Argon2id client KDF (19 MiB / 2 passes), browser-specific HKDF domains, random per-account data key, XChaCha20-Poly1305 authenticated items, fresh nonces and encrypted password/recovery wrappers. Local master and account passwords remain independent. The server receives a derived login verifier, never the account password/data key or item plaintext.
- Auth verifiers use a second random-salt Argon2id hash. Session tokens are random and only their hashes enter the DB. TOTP seeds are encrypted under an independent deployment key, recovery codes are hashed and consumed once; accepted login TOTP time steps cannot be replayed.
- Enabling 2FA requires verification; reenrollment cannot disable an enabled factor. Credential changes and account deletion require a fresh password proof and enabled TOTP. A recovery-key session is restricted to completing recovery/logout; completing recovery revokes other sessions and resets lost 2FA while preserving encrypted items.
- Items must be `browser-item`, IDs begin `nexusbro.v1/`. SSH types/IDs are rejected. Updates use compare-and-swap revisions. Tombstones retain authenticated ciphertext; a forged server `deleted` flag fails client validation. Client conflicts preserve both versions instead of silently choosing the latest timestamp.
- Uploading only supplies selected local records; omission never deletes a remote record. Root browser adapters own opt-in filtering before upload/application. Full paged pulls ensure enabling a previously disabled category does not lose data behind an old cursor. Known-record loss or revision regression fails closed.
- Payload/ciphertext sizes, pagination, KDF cost, response size, retries, attempt buckets and client timeouts are bounded. HTTP redirects and TLS verification bypasses are forbidden. Forwarded-IP headers are trusted only from loopback and must be overwritten by the TLS proxy.

## GitHub CI (no local Rust compilation on the live transit node)

```
cargo test --manifest-path sync-server/Cargo.toml --locked
```

`tests/client.rs` imports the actual production vault/crypto/account modules and runs actual HTTP against the Axum server with two independent local encrypted vaults. It tests encrypted round trips, conflict preservation, authenticated deletion, TOTP enrollment/replay, protected DB seeds, recovery, password rotation/session revocation, namespace rejection, HTTPS policy and locked-vault errors. No production account, paid API or deployed service is used. Pure crypto and vault regression tests run in the same test harness. Until CI has completed, these are prepared tests, not reported successes.

## Deployment handoff (operator only)

1. Choose a new browser-sync hostname and a node/loopback port that does not conflict with existing services. The sample uses `127.0.0.1:8796`; confirm availability before installation. Build the release binary in GitHub CI for the target OS/glibc and verify its commit/checksum. Do not compile on the VPN transit node.
2. Generate an independent random 32-byte TOTP deployment key and save its base64 encoding in the approved secret manager. Do not reuse any SSH key, put it in Git, print it in logs, or write a plaintext `.env`. Supply it directly on stdin from the secret manager to:

   ```
   systemd-creds encrypt --name=totp-key - /etc/credstore.encrypted/nexusbro-sync-totp-key.cred
   ```

   Keep an encrypted backup in the secret manager. Losing this key loses access to enabled authenticator seeds; changing it blindly breaks 2FA. The systemd credential file is machine-encrypted; the launcher reads it without logging and passes the key only to the dedicated server process.
3. Install the verified binary and `deploy/run-with-credential.py` under `/opt/nexusbro-sync/`, owned by root, not writable by the service. Install the new `deploy/nexusbro-sync.service` unit, unchanged existing units. The unit creates its own `0700` state directory and `0600` SQLite DB; DynamicUser, private tmp, filesystem hardening and resource limits isolate it.
4. Adapt `deploy/nginx.conf.example` to the new hostname/certificate as a new virtual host. Obtain the certificate through the operator's normal procedure; do not repurpose existing public/VPN proxy config. Test the complete proxy config before any reload. Bind the application only to loopback, terminate TLS externally, and leave CORS disabled.
5. After operator approval/startup, call only the new `/v1/health`; verify the marker over HTTPS and certificate validation. Then configure that URL in a test browser profile, create a browser-only account and exercise two devices, enable TOTP, save recovery material offline, and verify a recovery/password-rotation rehearsal. Production readiness requires this acceptance, not merely a green build.

Persistent database: `/var/lib/nexusbro-sync/nexusbro-sync.db` (with SQLite WAL). Take consistent SQLite backups and preserve the encrypted deployment key together under distinct protection. Do not copy the existing NexuSSH DB, application profile or secrets into this directory.

Only the operator responsible for services/proxy should install/reload/start these artifacts. They are not an instruction to change the live environment autonomously.
