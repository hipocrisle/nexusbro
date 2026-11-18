// Adapted from NexuSSH, Copyright (c) 2026 hipocrisle, MIT; see ../LICENSE.
//! SQLite storage layer.
//!
//! The server stores ONLY opaque ciphertext and auth verifiers. It never sees
//! the master password, the derived encryption keys, or item plaintext.

use rusqlite::Connection;
use std::sync::{Arc, Mutex};

/// Thin wrapper around a single SQLite connection guarded by a mutex.
///
/// Phase 0 traffic is tiny (login/register/totp), so a single serialized
/// connection is more than enough and keeps the code dependency-light. If write
/// throughput ever matters we can swap in a real pool without touching callers.
#[derive(Clone)]
pub struct Db {
    pub conn: Arc<Mutex<Connection>>,
}

impl Db {
    pub fn open(path: &str) -> rusqlite::Result<Self> {
        #[cfg(unix)]
        if path != ":memory:" {
            use std::os::unix::fs::OpenOptionsExt;
            use std::os::unix::fs::PermissionsExt;
            std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .mode(0o600)
                .open(path)
                .map_err(|_| rusqlite::Error::InvalidPath(path.into()))?;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
                .map_err(|_| rusqlite::Error::InvalidPath(path.into()))?;
        }
        let conn = if path == ":memory:" {
            Connection::open_in_memory()?
        } else {
            Connection::open(path)?
        };
        conn.pragma_update(None, "journal_mode", "WAL").ok();
        conn.pragma_update(None, "foreign_keys", "ON").ok();
        let db = Db {
            conn: Arc::new(Mutex::new(conn)),
        };
        db.init_schema()?;
        Ok(db)
    }

    /// Create the schema if it does not already exist.
    fn init_schema(&self) -> rusqlite::Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS users (
                id                        TEXT PRIMARY KEY,
                username                  TEXT UNIQUE NOT NULL,
                server_hash               TEXT NOT NULL,
                account_salt              TEXT NOT NULL,
                kdf_params                TEXT NOT NULL,
                wrapped_user_key          TEXT NOT NULL,
                recovery_wrapped_user_key TEXT,
                -- argon2 PHC verifier for the recovery key (mirror of server_hash
                -- for the password). Lets a no-password "recovery login" prove
                -- possession of the recovery key. NULL for pre-recovery accounts.
                recovery_hash             TEXT,
                totp_secret               TEXT,
                totp_enabled              INTEGER NOT NULL DEFAULT 0,
                totp_last_step            INTEGER NOT NULL DEFAULT -1,
                created_at                INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS devices (
                id         TEXT PRIMARY KEY,
                user_id    TEXT NOT NULL,
                name       TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                last_seen  INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS sessions (
                token      TEXT PRIMARY KEY,
                user_id    TEXT NOT NULL,
                device_id  TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                expires_at INTEGER NOT NULL,
                recovery   INTEGER NOT NULL DEFAULT 0
            );

            -- One-time recovery codes for TOTP. Only hashes are stored.
            CREATE TABLE IF NOT EXISTS recovery_codes (
                user_id   TEXT NOT NULL,
                code_hash TEXT NOT NULL,
                used      INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY(user_id, code_hash)
            );

            -- Per-item encrypted store (endpoints arrive in Phase 1; schema now).
            CREATE TABLE IF NOT EXISTS items (
                user_id    TEXT NOT NULL,
                item_id    TEXT NOT NULL,
                type       TEXT NOT NULL,
                ciphertext BLOB NOT NULL,
                rev        INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                deleted    INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY(user_id, item_id)
            );

            -- Per-user monotonic revision source for the items table.
            CREATE TABLE IF NOT EXISTS user_rev (
                user_id TEXT PRIMARY KEY,
                rev     INTEGER NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_sessions_user   ON sessions(user_id);
            CREATE INDEX IF NOT EXISTS idx_devices_user    ON devices(user_id);
            CREATE INDEX IF NOT EXISTS idx_items_user_rev  ON items(user_id, rev);
            "#,
        )?;
        // Migration for DBs created before recovery_hash existed. ALTER TABLE ADD
        // COLUMN errors if the column is already there → ignore that specific case.
        match conn.execute("ALTER TABLE users ADD COLUMN recovery_hash TEXT", []) {
            Ok(_) => {}
            Err(rusqlite::Error::SqliteFailure(_, Some(msg)))
                if msg.contains("duplicate column") => {}
            Err(e) => return Err(e),
        }
        Ok(())
    }
}
