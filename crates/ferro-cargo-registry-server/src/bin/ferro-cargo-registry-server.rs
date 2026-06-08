// SPDX-License-Identifier: Apache-2.0
//! Runnable Cargo registry server.
//!
//! Boots the [`ferro_cargo_registry_server::router`] over a
//! filesystem-backed [`ferro_blob_store::FsBlobStore`] and serves the
//! Cargo Alternative Registry Protocol (sparse-index variant). See
//! [RFC 2789] / the [Cargo registry reference] for the wire protocol.
//!
//! [RFC 2789]: https://rust-lang.github.io/rfcs/2789-sparse-index.html
//! [Cargo registry reference]: https://doc.rust-lang.org/cargo/reference/registries.html
//!
//! ## Configuration (environment)
//!
//! | Variable | Default | Purpose |
//! |---|---|---|
//! | `FERRO_CARGO_REGISTRY_DATA`   | `./registry-data` | Blob-store data directory |
//! | `FERRO_CARGO_REGISTRY_LISTEN` | `0.0.0.0:8081`    | Listen socket address |
//! | `FERRO_CARGO_REGISTRY_API`    | `http://<listen>` | API host advertised in `/config.json` |
//!
//! ## Probe routes
//!
//! In addition to the protocol routes from
//! [`ferro_cargo_registry_server::router`], the binary mounts
//! Kubernetes-style probes:
//!
//! - `GET /live`    — liveness, returns `200 OK`.
//! - `GET /ready`   — readiness, returns `200 OK`.
//! - `GET /healthz` — health, returns `200 OK` with `{"status":"ok"}`.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use axum::Json;
use axum::Router;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::get;
use ferro_blob_store::FsBlobStore;
use ferro_cargo_registry_server::{CargoState, router};
use serde_json::json;

/// Environment variable naming the blob-store data directory.
const ENV_DATA: &str = "FERRO_CARGO_REGISTRY_DATA";
/// Environment variable naming the listen socket address.
const ENV_LISTEN: &str = "FERRO_CARGO_REGISTRY_LISTEN";
/// Environment variable naming the advertised API host.
const ENV_API: &str = "FERRO_CARGO_REGISTRY_API";

/// Default blob-store data directory.
const DEFAULT_DATA: &str = "./registry-data";
/// Default listen socket address.
const DEFAULT_LISTEN: &str = "0.0.0.0:8081";

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("ferro-cargo-registry-server: fatal: {err}");
            ExitCode::FAILURE
        }
    }
}

/// Boot and serve until a shutdown signal arrives.
async fn run() -> Result<(), Box<dyn std::error::Error>> {
    init_tracing();

    let data_dir = PathBuf::from(
        std::env::var(ENV_DATA).unwrap_or_else(|_| DEFAULT_DATA.to_owned()),
    );
    let listen = std::env::var(ENV_LISTEN).unwrap_or_else(|_| DEFAULT_LISTEN.to_owned());
    let addr: SocketAddr = listen
        .parse()
        .map_err(|e| format!("invalid {ENV_LISTEN} {listen:?}: {e}"))?;
    let api_host =
        std::env::var(ENV_API).unwrap_or_else(|_| format!("http://{addr}"));

    std::fs::create_dir_all(&data_dir)
        .map_err(|e| format!("create data dir {}: {e}", data_dir.display()))?;
    let store = Arc::new(FsBlobStore::new(&data_dir)?);
    let state = CargoState::new(store, api_host.clone());

    let app = router(state).merge(probe_routes());

    let listener = tokio::net::TcpListener::bind(addr).await?;
    let bound = listener.local_addr()?;
    tracing::info!(
        %bound,
        data_dir = %data_dir.display(),
        api_host = %api_host,
        "ferro-cargo-registry-server listening"
    );

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    tracing::info!("ferro-cargo-registry-server shut down cleanly");
    Ok(())
}

/// Probe routes mounted alongside the protocol router.
fn probe_routes() -> Router {
    Router::new()
        .route("/live", get(live))
        .route("/ready", get(ready))
        .route("/healthz", get(healthz))
}

/// Liveness probe.
async fn live() -> impl IntoResponse {
    StatusCode::OK
}

/// Readiness probe.
async fn ready() -> impl IntoResponse {
    StatusCode::OK
}

/// Health probe — returns `{"status":"ok"}`.
async fn healthz() -> impl IntoResponse {
    (StatusCode::OK, Json(json!({ "status": "ok" })))
}

/// Initialise a best-effort tracing subscriber.
///
/// Honours `RUST_LOG`; falls back to `info`. A failure to install (for
/// instance, a global subscriber already present in a test harness) is
/// ignored so the server still boots.
fn init_tracing() {
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .try_init();
}

/// Resolve once either `SIGINT` (Ctrl-C) or `SIGTERM` is received.
async fn shutdown_signal() {
    let ctrl_c = async {
        if let Err(err) = tokio::signal::ctrl_c().await {
            tracing::error!(%err, "failed to install Ctrl-C handler");
        }
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut sig) => {
                sig.recv().await;
            }
            Err(err) => tracing::error!(%err, "failed to install SIGTERM handler"),
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => tracing::info!("received Ctrl-C, shutting down"),
        () = terminate => tracing::info!("received SIGTERM, shutting down"),
    }
}
