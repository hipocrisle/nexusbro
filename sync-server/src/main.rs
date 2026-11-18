// Adapted from NexuSSH, Copyright (c) 2026 hipocrisle, MIT; see ../LICENSE.
//! Binary entrypoint for the NexusBro sync server.
//!
//! Config via env:
//!   NEXUSBRO_SYNC_BIND  (default 127.0.0.1:8796)
//!   NEXUSBRO_SYNC_DB    (default ./nexusbro-sync.db)
//!
//! Listens on a high port behind nginx (TLS terminated by nginx).

use std::net::SocketAddr;

use nexusbro_sync_server::{app, build_state};

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let bind = std::env::var("NEXUSBRO_SYNC_BIND").unwrap_or_else(|_| "127.0.0.1:8796".to_string());
    let db_path =
        std::env::var("NEXUSBRO_SYNC_DB").unwrap_or_else(|_| "./nexusbro-sync.db".to_string());

    let state = build_state(&db_path).expect("failed to open/init database");
    let router = app(state);

    let addr: SocketAddr = bind.parse().expect("invalid NEXUSBRO_SYNC_BIND address");
    tracing::info!(
        "nexusbro-sync-server listening on {} (db: {})",
        addr,
        db_path
    );

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .expect("failed to bind");
    // into_make_service_with_connect_info gives handlers ConnectInfo<SocketAddr>
    // for per-IP rate limiting.
    axum::serve(
        listener,
        router.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await
    .expect("server error");
}
