// SPDX-License-Identifier: Apache-2.0
//! Runnable-server assembly: environment configuration, app wiring, and
//! the bind+serve loop with graceful shutdown.
//!
//! The `ferro-oci-server` binary is a thin shim over this module — it
//! calls [`Config::from_env`] then [`serve`]. Keeping the logic here
//! (rather than in `fn main`) makes the configuration parser, the blob
//! store selection, and the app assembly directly unit-testable.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use ferro_blob_store::{FsBlobStore, InMemoryBlobStore, SharedBlobStore};

use crate::metrics::{Metrics, instrument};
use crate::registry::InMemoryRegistryMeta;
use crate::router::{AppState, probe_routes, router};

/// Environment variable naming the listen socket address.
pub const ENV_LISTEN: &str = "FERRO_OCI_LISTEN";
/// Environment variable naming the filesystem blob-store directory.
pub const ENV_STORAGE_DIR: &str = "FERRO_OCI_STORAGE_DIR";

/// Default listen socket address.
pub const DEFAULT_LISTEN: &str = "0.0.0.0:8080";

/// Server configuration, sourced from the process environment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// `host:port` the HTTP server binds to (`FERRO_OCI_LISTEN`).
    pub listen: String,
    /// Optional filesystem directory for blob bytes
    /// (`FERRO_OCI_STORAGE_DIR`). When `None`, an in-memory blob store
    /// is used — convenient for smoke tests and conformance runs, but
    /// non-durable.
    pub storage_dir: Option<PathBuf>,
}

impl Config {
    /// Read the configuration from the process environment, applying
    /// defaults for anything unset.
    #[must_use]
    pub fn from_env() -> Self {
        let listen = std::env::var(ENV_LISTEN).ok();
        let storage_dir = std::env::var_os(ENV_STORAGE_DIR).filter(|v| !v.is_empty());
        Self::from_raw(listen, storage_dir.map(PathBuf::from))
    }

    /// Build a [`Config`] from already-resolved listen / storage-dir
    /// values, applying defaults for `None`.
    ///
    /// Factored out of [`from_env`](Self::from_env) so the parsing and
    /// default rules are unit-testable without mutating the process
    /// environment (which `unsafe_code = forbid` disallows here). An
    /// empty `storage_dir` path is normalised to "in-memory".
    #[must_use]
    pub fn from_raw(listen: Option<String>, storage_dir: Option<PathBuf>) -> Self {
        let listen = listen.unwrap_or_else(|| DEFAULT_LISTEN.to_owned());
        let storage_dir = storage_dir.filter(|p| !p.as_os_str().is_empty());
        Self {
            listen,
            storage_dir,
        }
    }

    /// Parse and validate the listen address.
    ///
    /// # Errors
    ///
    /// Returns an error string when `listen` is not a valid
    /// `host:port` socket address.
    pub fn socket_addr(&self) -> Result<SocketAddr, String> {
        self.listen
            .parse::<SocketAddr>()
            .map_err(|e| format!("invalid {ENV_LISTEN} `{}`: {e}", self.listen))
    }

    /// Build the [`SharedBlobStore`] this config describes.
    ///
    /// # Errors
    ///
    /// Returns an error when a filesystem store is requested but its
    /// directory cannot be created or opened.
    pub fn blob_store(&self) -> Result<SharedBlobStore, Box<dyn std::error::Error>> {
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

/// Assemble the full application router from a blob store: the OCI
/// `/v2/**` surface + Kubernetes health probes, wrapped in the
/// Prometheus instrumentation middleware and `/metrics` endpoint.
pub fn build_app(blob_store: SharedBlobStore) -> axum::Router {
    let registry = Arc::new(InMemoryRegistryMeta::new());
    let state = AppState::new(blob_store, registry);
    let blob_count = state.blob_count_handle();
    instrument(
        router(state).merge(probe_routes()),
        Metrics::new(),
        blob_count,
    )
}

/// Boot the server described by `config` and serve until a shutdown
/// signal (`SIGINT` / `SIGTERM`) arrives.
///
/// # Errors
///
/// Returns an error when the listen address is invalid, the blob store
/// cannot be opened, the socket cannot be bound, or the server loop
/// fails.
pub async fn serve(config: &Config) -> Result<(), Box<dyn std::error::Error>> {
    tracing::info!(?config, "ferro-oci-server starting");
    let addr = config.socket_addr()?;
    let blob_store = config.blob_store()?;
    let app = build_app(blob_store);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(%addr, "listening");

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    tracing::info!("ferro-oci-server stopped");
    Ok(())
}

/// Install a `tracing-subscriber` honouring `RUST_LOG` (default `info`).
///
/// A failure to install (for instance, a global subscriber already
/// present in a test harness) is ignored so the server still boots.
pub fn init_tracing() {
    use tracing_subscriber::EnvFilter;

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt().with_env_filter(filter).try_init();
}

/// Resolve when either `SIGINT` (Ctrl-C) or `SIGTERM` (container stop)
/// is received, so `axum::serve` can drain in-flight requests before
/// the process exits.
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
        () = ctrl_c => tracing::info!("received SIGINT — shutting down"),
        () = terminate => tracing::info!("received SIGTERM — shutting down"),
    }
}

#[cfg(test)]
mod tests {
    use super::{Config, DEFAULT_LISTEN, ENV_LISTEN, build_app, init_tracing};
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use std::net::SocketAddr;
    use std::path::PathBuf;
    use tower::ServiceExt;

    #[test]
    fn from_raw_applies_defaults_when_unset() {
        let cfg = Config::from_raw(None, None);
        assert_eq!(cfg.listen, DEFAULT_LISTEN);
        assert_eq!(cfg.storage_dir, None);
    }

    #[test]
    fn from_raw_reads_overrides() {
        let cfg = Config::from_raw(
            Some("127.0.0.1:0".to_owned()),
            Some(PathBuf::from("/var/lib/oci-xyz")),
        );
        assert_eq!(cfg.listen, "127.0.0.1:0");
        assert_eq!(cfg.storage_dir, Some(PathBuf::from("/var/lib/oci-xyz")));
    }

    #[test]
    fn from_raw_treats_empty_storage_dir_as_inmemory() {
        let cfg = Config::from_raw(None, Some(PathBuf::new()));
        assert_eq!(cfg.storage_dir, None);
    }

    #[test]
    fn from_env_smoke() {
        let cfg = Config::from_env();
        assert!(!cfg.listen.is_empty());
    }

    #[test]
    fn socket_addr_parses_valid_listen() {
        let cfg = Config {
            listen: "0.0.0.0:8080".to_owned(),
            storage_dir: None,
        };
        assert_eq!(
            cfg.socket_addr().expect("addr"),
            "0.0.0.0:8080".parse::<SocketAddr>().unwrap()
        );
    }

    #[test]
    fn socket_addr_rejects_garbage() {
        let cfg = Config {
            listen: "nope".to_owned(),
            storage_dir: None,
        };
        let err = cfg.socket_addr().expect_err("should fail");
        assert!(err.contains(ENV_LISTEN), "error names the env var: {err}");
    }

    #[test]
    fn blob_store_in_memory_when_unset() {
        let cfg = Config {
            listen: DEFAULT_LISTEN.to_owned(),
            storage_dir: None,
        };
        assert!(cfg.blob_store().is_ok());
    }

    #[test]
    fn blob_store_creates_fs_dir() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let dir = tmp.path().join("nested/blobs");
        assert!(!dir.exists());
        let cfg = Config {
            listen: DEFAULT_LISTEN.to_owned(),
            storage_dir: Some(dir.clone()),
        };
        assert!(cfg.blob_store().is_ok());
        assert!(dir.is_dir(), "fs blob dir created");
    }

    #[tokio::test]
    async fn build_app_serves_probes_v2_and_metrics() {
        let app = build_app(std::sync::Arc::new(
            ferro_blob_store::InMemoryBlobStore::new(),
        ));
        for (uri, expected) in [
            ("/live", StatusCode::OK),
            ("/ready", StatusCode::OK),
            ("/healthz", StatusCode::OK),
            ("/v2/", StatusCode::OK),
            ("/metrics", StatusCode::OK),
        ] {
            let resp = app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri(uri)
                        .body(Body::empty())
                        .expect("req"),
                )
                .await
                .expect("resp");
            assert_eq!(resp.status(), expected, "GET {uri}");
        }
    }

    #[tokio::test]
    async fn serve_rejects_invalid_listen_before_binding() {
        let cfg = Config {
            listen: "definitely-not-an-addr".to_owned(),
            storage_dir: None,
        };
        let err = super::serve(&cfg).await.expect_err("invalid addr fails");
        assert!(err.to_string().contains(ENV_LISTEN));
    }

    #[test]
    fn init_tracing_is_idempotent() {
        init_tracing();
        init_tracing();
    }
}
