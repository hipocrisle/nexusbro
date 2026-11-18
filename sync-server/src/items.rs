// Adapted from NexuSSH, Copyright (c) 2026 hipocrisle, MIT; see ../LICENSE.
//! Phase-1 per-item encrypted store: delta pull + batch push with optimistic
//! concurrency (CAS on `rev`) and tombstones.
//!
//! The server is end-to-end-encrypted: `ciphertext` is an opaque, client-encrypted
//! blob. The server NEVER decrypts it. It only stores blobs and assigns a strictly
//! monotonic per-user revision (`rev`) to each accepted write, so other devices can
//! pull just the changes they have not seen yet (`?since=<rev>`).

use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};

use crate::handlers::{db_err, ApiError, AuthUser};
use crate::AppState;

// ---------------------------------------------------------------------------
// Limits (documented in README). Violations → 400 / 413.
// ---------------------------------------------------------------------------

/// Max items returned in a single pull page. If the user has more unseen
/// changes, the client re-pulls with `since` = the returned `latest_rev`.
const PULL_PAGE_CAP: usize = 8;
/// Max changes accepted in one push batch.
const PUSH_BATCH_CAP: usize = 8;
/// Max decoded ciphertext size per item (1 MiB).
const MAX_CIPHERTEXT_BYTES: usize = 256 * 1024;

/// Known item types the client may store. The server treats ciphertext as
/// opaque, but it validates the `type` tag so a buggy/hostile client can't fill
/// the table with arbitrary kinds.
const KNOWN_TYPES: &[&str] = &["browser-item"];

fn type_is_known(t: &str) -> bool {
    KNOWN_TYPES.contains(&t)
}

type ApiResult<T> = Result<T, ApiError>;

// ---------------------------------------------------------------------------
// GET /v1/items?since=<rev>  — delta pull (tombstones included).
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct PullQuery {
    /// Return items with `rev > since`. `since=0` (default) = full pull.
    #[serde(default)]
    pub since: i64,
}

#[derive(Serialize)]
pub struct PulledItem {
    pub item_id: String,
    #[serde(rename = "type")]
    pub item_type: String,
    /// base64 ciphertext; "" for tombstones (deleted=true).
    pub ciphertext: String,
    pub rev: i64,
    pub updated_at: i64,
    pub deleted: bool,
}

#[derive(Serialize)]
pub struct PullResp {
    pub items: Vec<PulledItem>,
    pub latest_rev: i64,
}

pub async fn pull_items(
    State(state): State<AppState>,
    user: axum::Extension<AuthUser>,
    Query(q): Query<PullQuery>,
) -> ApiResult<impl IntoResponse> {
    let since = q.since.max(0);
    let conn = state.db.conn.lock().unwrap();

    // Tombstones store an empty ciphertext blob; non-deleted rows carry the
    // opaque client ciphertext which we base64-encode for the JSON response.
    let mut stmt = conn
        .prepare(
            "SELECT item_id, type, ciphertext, rev, updated_at, deleted
             FROM items
             WHERE user_id = ?1 AND rev > ?2
             ORDER BY rev ASC
             LIMIT ?3",
        )
        .map_err(db_err)?;

    let rows = stmt
        .query_map(
            rusqlite::params![user.user_id, since, PULL_PAGE_CAP as i64],
            |r| {
                let deleted = r.get::<_, i64>(5)? != 0;
                let blob: Vec<u8> = r.get(2)?;
                let ciphertext = B64.encode(&blob);
                Ok(PulledItem {
                    item_id: r.get(0)?,
                    item_type: r.get(1)?,
                    ciphertext,
                    rev: r.get(3)?,
                    updated_at: r.get(4)?,
                    deleted,
                })
            },
        )
        .map_err(db_err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_err)?;
    drop(stmt);

    // `latest_rev` is the user's current high-water mark (NOT just this page's
    // max), so a client that received a capped page knows there is more and can
    // re-pull from the last rev it actually saw.
    let latest_rev = current_user_rev(&conn, &user.user_id).map_err(db_err)?;

    Ok(Json(PullResp {
        items: rows,
        latest_rev,
    }))
}

// ---------------------------------------------------------------------------
// POST /v1/items  — batch push with optimistic concurrency (CAS on rev).
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct Change {
    pub item_id: String,
    #[serde(rename = "type")]
    pub item_type: String,
    /// base64 ciphertext (ignored / may be "" when deleted=true).
    #[serde(default)]
    pub ciphertext: String,
    pub updated_at: i64,
    #[serde(default)]
    pub deleted: bool,
    /// The rev the client last saw for this item; 0 for a brand-new item.
    #[serde(default)]
    pub base_rev: i64,
}

#[derive(Deserialize)]
pub struct PushReq {
    pub changes: Vec<Change>,
}

/// Server's authoritative copy of an item, echoed back on a conflict so the
/// client can resolve (last-writer-wins by `updated_at`) and retry.
#[derive(Serialize)]
pub struct ServerItem {
    pub item_id: String,
    #[serde(rename = "type")]
    pub item_type: String,
    pub ciphertext: String,
    pub rev: i64,
    pub updated_at: i64,
    pub deleted: bool,
}

#[derive(Serialize)]
pub struct PushResult {
    pub item_id: String,
    /// "ok" | "conflict"
    pub status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rev: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server: Option<ServerItem>,
}

#[derive(Serialize)]
pub struct PushResp {
    pub results: Vec<PushResult>,
    pub latest_rev: i64,
}

pub async fn push_items(
    State(state): State<AppState>,
    user: axum::Extension<AuthUser>,
    Json(req): Json<PushReq>,
) -> ApiResult<impl IntoResponse> {
    // --- Validate the whole batch BEFORE touching the DB. ---
    if req.changes.len() > PUSH_BATCH_CAP {
        return Err(ApiError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "too many changes in batch",
        ));
    }

    // Decode + validate each change up front so a bad item rejects the request
    // rather than half-applying the batch.
    struct Decoded {
        item_id: String,
        item_type: String,
        ciphertext: Vec<u8>,
        updated_at: i64,
        deleted: bool,
        base_rev: i64,
    }
    let mut decoded: Vec<Decoded> = Vec::with_capacity(req.changes.len());
    for c in &req.changes {
        if !c.item_id.starts_with("nexusbro.v1/")
            || c.item_id.len() > 256
            || c.item_id.chars().any(char::is_control)
        {
            return Err(ApiError::new(StatusCode::BAD_REQUEST, "invalid item_id"));
        }
        if !type_is_known(&c.item_type) {
            return Err(ApiError::new(StatusCode::BAD_REQUEST, "unknown item type"));
        }
        if c.base_rev < 0 {
            return Err(ApiError::new(StatusCode::BAD_REQUEST, "invalid base_rev"));
        }
        // A tombstone carries no ciphertext; otherwise decode + size-check it.
        let ciphertext = {
            let bytes = B64
                .decode(c.ciphertext.as_bytes())
                .map_err(|_| ApiError::new(StatusCode::BAD_REQUEST, "ciphertext not base64"))?;
            if bytes.len() < 40 || bytes.len() > MAX_CIPHERTEXT_BYTES {
                return Err(ApiError::new(
                    StatusCode::PAYLOAD_TOO_LARGE,
                    "ciphertext too large",
                ));
            }
            bytes
        };
        decoded.push(Decoded {
            item_id: c.item_id.clone(),
            item_type: c.item_type.clone(),
            ciphertext,
            updated_at: c.updated_at,
            deleted: c.deleted,
            base_rev: c.base_rev,
        });
    }

    // --- Apply atomically per user. ---
    let mut conn = state.db.conn.lock().unwrap();
    let tx = conn.transaction().map_err(db_err)?;

    let mut results: Vec<PushResult> = Vec::with_capacity(decoded.len());
    for d in &decoded {
        // Current stored state for this item, if any.
        let stored: Option<(i64,)> = tx
            .query_row(
                "SELECT rev FROM items WHERE user_id = ?1 AND item_id = ?2",
                rusqlite::params![user.user_id, d.item_id],
                |r| Ok((r.get::<_, i64>(0)?,)),
            )
            .optional()
            .map_err(db_err)?;

        let accept = match stored {
            None => d.base_rev == 0,           // new item only if client expects none
            Some((cur,)) => cur == d.base_rev, // CAS: must match what client last saw
        };

        if !accept {
            // Conflict: do NOT write. Echo the server's authoritative version so
            // the client can resolve (last-writer-wins by updated_at) and retry.
            let server = load_server_item(&tx, &user.user_id, &d.item_id).map_err(db_err)?;
            results.push(PushResult {
                item_id: d.item_id.clone(),
                status: "conflict",
                rev: None,
                server,
            });
            continue;
        }

        // Accepted: bump the per-user monotonic rev inside the same tx so
        // concurrent pushes can never get the same rev.
        let new_rev = bump_user_rev(&tx, &user.user_id).map_err(db_err)?;

        // INSERT-or-replace the row with the freshly minted rev. A tombstone is
        // just a row with deleted=1 and an empty ciphertext blob.
        tx.execute(
            "INSERT INTO items (user_id, item_id, type, ciphertext, rev, updated_at, deleted)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(user_id, item_id) DO UPDATE SET
                 type = excluded.type,
                 ciphertext = excluded.ciphertext,
                 rev = excluded.rev,
                 updated_at = excluded.updated_at,
                 deleted = excluded.deleted",
            rusqlite::params![
                user.user_id,
                d.item_id,
                d.item_type,
                d.ciphertext,
                new_rev,
                d.updated_at,
                d.deleted as i64,
            ],
        )
        .map_err(db_err)?;

        results.push(PushResult {
            item_id: d.item_id.clone(),
            status: "ok",
            rev: Some(new_rev),
            server: None,
        });
    }

    let latest_rev = current_user_rev(&tx, &user.user_id).map_err(db_err)?;
    tx.commit().map_err(db_err)?;

    Ok(Json(PushResp {
        results,
        latest_rev,
    }))
}

// ---------------------------------------------------------------------------
// rev source helpers
// ---------------------------------------------------------------------------

/// The user's current high-water revision (0 if they've never written).
fn current_user_rev(conn: &rusqlite::Connection, user_id: &str) -> rusqlite::Result<i64> {
    conn.query_row(
        "SELECT rev FROM user_rev WHERE user_id = ?1",
        [user_id],
        |r| r.get::<_, i64>(0),
    )
    .optional()
    .map(|o| o.unwrap_or(0))
}

/// Atomically allocate the next per-user rev and return it. Done inside the
/// caller's transaction so concurrent pushes are serialized and can never
/// collide on a rev. `UPSERT ... RETURNING` makes this a single statement.
fn bump_user_rev(conn: &rusqlite::Connection, user_id: &str) -> rusqlite::Result<i64> {
    conn.query_row(
        "INSERT INTO user_rev (user_id, rev) VALUES (?1, 1)
         ON CONFLICT(user_id) DO UPDATE SET rev = rev + 1
         RETURNING rev",
        [user_id],
        |r| r.get::<_, i64>(0),
    )
}

/// Load the server's authoritative copy of an item for a conflict response.
fn load_server_item(
    conn: &rusqlite::Connection,
    user_id: &str,
    item_id: &str,
) -> rusqlite::Result<Option<ServerItem>> {
    conn.query_row(
        "SELECT item_id, type, ciphertext, rev, updated_at, deleted
         FROM items WHERE user_id = ?1 AND item_id = ?2",
        rusqlite::params![user_id, item_id],
        |r| {
            let deleted = r.get::<_, i64>(5)? != 0;
            let blob: Vec<u8> = r.get(2)?;
            let ciphertext = B64.encode(&blob);
            Ok(ServerItem {
                item_id: r.get(0)?,
                item_type: r.get(1)?,
                ciphertext,
                rev: r.get(3)?,
                updated_at: r.get(4)?,
                deleted,
            })
        },
    )
    .optional()
}
