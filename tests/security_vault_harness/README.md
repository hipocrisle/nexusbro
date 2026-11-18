Run on GitHub's Windows and Linux runners:

```sh
cargo test --manifest-path tests/security_vault_harness/Cargo.toml
```

The harness imports the actual production vault module, without Tauri/CEF or
mock crypto. Tests verify persistence across lock/unlock, rejection of wrong
passwords and hostile headers, ciphertext freshness, no plaintext in backups,
failed-write rollback in memory and on disk, idle lock, profile isolation,
input/corruption handling and Unix file permissions/symlink rejection.

The local agent only formatted/parses the module; no Rust build was run on DE-1.
Dependency feature `argon2/zeroize` is intentional: clear Argon2 memory blocks
in addition to key/serialized-secret buffers controlled by the vault.
