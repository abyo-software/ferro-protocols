// SPDX-License-Identifier: Apache-2.0
//! `ferro-oci-server` — a runnable OCI Distribution Spec v1.1 registry.
//!
//! Boots the [`ferro_oci_server::router`] over a filesystem-backed blob
//! store and the in-memory metadata plane, merges in the Kubernetes
//! health probes (`/live`, `/healthz`, `/ready`), and serves it with
//! graceful `SIGTERM`/`SIGINT` shutdown.
//!
//! # Configuration
//!
//! All configuration is environment-driven (see [`Config`]):
//!
//! | Variable                  | Default          | Meaning                                   |
//! |---------------------------|------------------|-------------------------------------------|
//! | `FERRO_OCI_LISTEN`        | `0.0.0.0:8080`   | host:port the HTTP server binds to        |
//! | `FERRO_OCI_STORAGE_DIR`   | *(in-memory)*    | filesystem dir for blob bytes; unset → RAM|
//! | `RUST_LOG`                | `info`           | `tracing-subscriber` env filter           |
//!
//! # Example
//!
//! ```bash
//! FERRO_OCI_LISTEN=127.0.0.1:5000 \
//! FERRO_OCI_STORAGE_DIR=/var/lib/oci-registry \
//!   cargo run --bin ferro-oci-server -p ferro-oci-server
//!
//! curl -s localhost:5000/healthz        # {"status":"ok"}
//! curl -s localhost:5000/v2/            # {}
//! docker push localhost:5000/alpine:latest
//! ```

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use ferro_blob_store::{FsBlobStore, InMemoryBlobStore, SharedBlobStore};
use ferro_oci_server::{AppState, InMemoryRegistryMeta, Metrics, instrument, probe_routes, router};

/// Server configuration, sourced from the environment.
#[derive(Debug, Clone)]
struct Config {
    /// `host:port` the HTTP server binds to (`FERRO_OCI_LISTEN`).
    listen: String,
    /// Optional filesystem directory for blob bytes
    /// (`FERRO_OCI_STORAGE_DIR`). When `None`, an in-memory blob store
    /// is used — convenient for smoke tests and conformance runs, but
    /// non-durable.
    storage_dir: Option<PathBuf>,
}

impl Config {
    /// Read the configuration from the process environment, applying
    /// defaults for anything unset.
    fn from_env() -> Self {
        let listen = std::env::var("FERRO_OCI_LISTEN").unwrap_or_else(|_| "0.0.0.0:8080".to_owned());
        let storage_dir = std::env::var_os("FERRO_OCI_STORAGE_DIR")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from);
        Self {
            listen,
            storage_dir,
        }
    }

    /// Build the [`SharedBlobStore`] this config describes.
    fn blob_store(&self) -> Result<SharedBlobStore, Box<dyn std::error::Error>> {
        match &self.storage_dir {
            Some(dir) => {
                std::fs::create_dir_all(dir)?;
                let store = FsBlobStore::new(dir.clone())?;
                tracing::info!(path = %dir.display(), "using filesystem blob store");
                Ok(Arc::new(store))
            }
            None => {
                tracing::warn!(
                    "FERRO_OCI_STORAGE_DIR unset — using a non-durable in-memory blob store"
                );
                Ok(Arc::new(InMemoryBlobStore::new()))
            }
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    init_tracing();

    let config = Config::from_env();
    tracing::info!(?config, "ferro-oci-server starting");

    let blob_store = config.blob_store()?;
    let registry = Arc::new(InMemoryRegistryMeta::new());
    let state = AppState::new(blob_store.clone(), registry);

    // OCI `/v2/**` surface + Kubernetes health probes, wrapped in the
    // Prometheus instrumentation middleware and `/metrics` endpoint.
    let app = instrument(router(state).merge(probe_routes()), Metrics::new(), blob_store);

    let addr: SocketAddr = config
        .listen
        .parse()
        .map_err(|e| format!("invalid FERRO_OCI_LISTEN `{}`: {e}", config.listen))?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(%addr, "listening");

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    tracing::info!("ferro-oci-server stopped");
    Ok(())
}

/// Install a `tracing-subscriber` honouring `RUST_LOG` (default `info`).
fn init_tracing() {
    use tracing_subscriber::EnvFilter;

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();
}

/// Resolve when either `SIGINT` (Ctrl-C) or `SIGTERM` (container stop)
/// is received, so `axum::serve` can drain in-flight requests before
/// the process exits.
async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("install Ctrl-C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => tracing::info!("received SIGINT — shutting down"),
        () = terminate => tracing::info!("received SIGTERM — shutting down"),
    }
}
