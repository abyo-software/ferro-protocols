// SPDX-License-Identifier: Apache-2.0
//! Runnable-server assembly: environment configuration, app wiring, and
//! the bind+serve loop with graceful shutdown.
//!
//! The `ferro-cargo-registry-server` binary is a thin shim over this
//! module — it calls [`Config::from_env`] then [`serve`]. Keeping the
//! logic here (rather than in `fn main`) makes the configuration parser,
//! the probe routes, and the app assembly directly unit-testable.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::Json;
use axum::Router;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::get;
use ferro_blob_store::FsBlobStore;
use serde_json::json;

use crate::metrics::{Metrics, instrument};
use crate::router::{CargoState, router};

/// Environment variable naming the blob-store data directory.
pub const ENV_DATA: &str = "FERRO_CARGO_REGISTRY_DATA";
/// Environment variable naming the listen socket address.
pub const ENV_LISTEN: &str = "FERRO_CARGO_REGISTRY_LISTEN";
/// Environment variable naming the advertised API host.
pub const ENV_API: &str = "FERRO_CARGO_REGISTRY_API";

/// Default blob-store data directory.
pub const DEFAULT_DATA: &str = "./registry-data";
/// Default listen socket address.
pub const DEFAULT_LISTEN: &str = "0.0.0.0:8081";

/// Server configuration, sourced from the process environment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// Blob-store data directory (`FERRO_CARGO_REGISTRY_DATA`).
    pub data_dir: PathBuf,
    /// `host:port` the HTTP server binds to (`FERRO_CARGO_REGISTRY_LISTEN`).
    pub listen: String,
    /// API host advertised in `/config.json` (`FERRO_CARGO_REGISTRY_API`).
    /// `None` means "derive `http://<resolved-listen-addr>`" at bind time.
    pub api_host: Option<String>,
}

impl Config {
    /// Read the configuration from the process environment, applying
    /// defaults for anything unset.
    #[must_use]
    pub fn from_env() -> Self {
        Self::from_vars(|k| std::env::var(k).ok())
    }

    /// Build a [`Config`] from an arbitrary variable lookup, applying
    /// defaults for anything the lookup returns `None` for.
    ///
    /// Factored out of [`from_env`](Self::from_env) so the parsing and
    /// default rules are unit-testable without mutating the process
    /// environment (which `unsafe_code = forbid` disallows here).
    #[must_use]
    pub fn from_vars(lookup: impl Fn(&str) -> Option<String>) -> Self {
        let data_dir = PathBuf::from(lookup(ENV_DATA).unwrap_or_else(|| DEFAULT_DATA.to_owned()));
        let listen = lookup(ENV_LISTEN).unwrap_or_else(|| DEFAULT_LISTEN.to_owned());
        let api_host = lookup(ENV_API).filter(|s| !s.is_empty());
        Self {
            data_dir,
            listen,
            api_host,
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
            .map_err(|e| format!("invalid {ENV_LISTEN} {:?}: {e}", self.listen))
    }

    /// Resolve the API host advertised in `/config.json`.
    ///
    /// Uses the explicit `FERRO_CARGO_REGISTRY_API` value when set,
    /// otherwise derives `http://<addr>` from the bound socket address.
    ///
    /// Prefer [`resolve_api_host_checked`](Self::resolve_api_host_checked),
    /// which refuses to advertise an unusable wildcard / port-0 origin;
    /// this infallible variant is kept for callers (and tests) that have
    /// already validated the address.
    #[must_use]
    pub fn resolve_api_host(&self, addr: SocketAddr) -> String {
        self.api_host
            .clone()
            .unwrap_or_else(|| format!("http://{addr}"))
    }

    /// Resolve the advertised API host, refusing to advertise an origin a
    /// remote cargo client cannot reach.
    ///
    /// An explicit `FERRO_CARGO_REGISTRY_API` value is always honoured.
    /// Otherwise the origin is derived from the bound socket address — but
    /// a wildcard listen host (`0.0.0.0` / `::`) or an ephemeral port `0`
    /// would advertise `http://0.0.0.0:8081/...` (or a `:0` port) in
    /// `config.json`, which no remote client can fetch from. In that case
    /// this returns an error instructing the operator to set
    /// `FERRO_CARGO_REGISTRY_API` to the externally reachable origin.
    ///
    /// # Errors
    ///
    /// Returns an error string when the listen address is a wildcard host
    /// or port `0` and no explicit `FERRO_CARGO_REGISTRY_API` is set.
    pub fn resolve_api_host_checked(&self, addr: SocketAddr) -> Result<String, String> {
        if let Some(explicit) = &self.api_host {
            return Ok(explicit.clone());
        }
        if addr.ip().is_unspecified() || addr.port() == 0 {
            return Err(format!(
                "cannot advertise a usable registry origin: the listen address {addr} is a \
                 wildcard host or port 0, which remote cargo clients cannot fetch from. Set \
                 {ENV_API} to the externally reachable origin (for example \
                 `http://registry.example:8081`)."
            ));
        }
        Ok(format!("http://{addr}"))
    }
}

/// Build the [`CargoState`] for a filesystem-backed registry.
///
/// Creates the data directory if it does not yet exist, then enables
/// durable index persistence (DD R2-6): any existing `index-state.json`
/// snapshot is loaded, and every subsequent publish / yank / owner change
/// is written through to it. A restart therefore re-serves every
/// previously published crate, version, `yanked` flag, and owner — not
/// just the tarballs the blob store already kept.
///
/// # Errors
///
/// Returns an error when the data directory cannot be created or the
/// blob store cannot be opened. A missing or corrupt snapshot is **not**
/// an error: it starts the index empty (logged).
pub fn build_state(
    data_dir: &Path,
    api_host: impl Into<String>,
) -> Result<CargoState, Box<dyn std::error::Error>> {
    std::fs::create_dir_all(data_dir)
        .map_err(|e| format!("create data dir {}: {e}", data_dir.display()))?;
    let store = Arc::new(FsBlobStore::new(data_dir)?);
    Ok(CargoState::with_persistence(
        store,
        api_host,
        data_dir.to_path_buf(),
    ))
}

/// Assemble the full application router: protocol surface + Kubernetes
/// probes, wrapped in the Prometheus instrumentation middleware and the
/// `/metrics` endpoint.
pub fn build_app(state: CargoState) -> Router {
    instrument(
        router(state.clone()).merge(probe_routes()),
        Metrics::new(),
        state,
    )
}

/// Probe routes mounted alongside the protocol router.
pub fn probe_routes() -> Router {
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

/// Boot the server described by `config` and serve until a shutdown
/// signal (`SIGINT` / `SIGTERM`) arrives.
///
/// # Errors
///
/// Returns an error when the listen address is invalid, the data
/// directory or blob store cannot be opened, the socket cannot be
/// bound, or the server loop fails.
pub async fn serve(config: &Config) -> Result<(), Box<dyn std::error::Error>> {
    let addr = config.socket_addr()?;
    let api_host = config.resolve_api_host_checked(addr)?;
    let state = build_state(&config.data_dir, api_host.clone())?;
    let app = build_app(state);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    let bound = listener.local_addr()?;
    tracing::info!(
        %bound,
        data_dir = %config.data_dir.display(),
        api_host = %api_host,
        "ferro-cargo-registry-server listening"
    );

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    tracing::info!("ferro-cargo-registry-server shut down cleanly");
    Ok(())
}

/// Initialise a best-effort tracing subscriber.
///
/// Honours `RUST_LOG`; falls back to `info`. A failure to install (for
/// instance, a global subscriber already present in a test harness) is
/// ignored so the server still boots.
pub fn init_tracing() {
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt().with_env_filter(filter).try_init();
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

#[cfg(test)]
mod tests {
    use super::{
        Config, DEFAULT_DATA, DEFAULT_LISTEN, ENV_API, ENV_DATA, ENV_LISTEN, build_app,
        build_state, init_tracing, probe_routes,
    };
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode};
    use std::collections::HashMap;
    use std::net::SocketAddr;
    use std::path::PathBuf;
    use tower::ServiceExt;

    fn lookup_from(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        move |k: &str| map.get(k).cloned()
    }

    #[test]
    fn from_vars_applies_defaults_when_unset() {
        let cfg = Config::from_vars(lookup_from(&[]));
        assert_eq!(cfg.data_dir, PathBuf::from(DEFAULT_DATA));
        assert_eq!(cfg.listen, DEFAULT_LISTEN);
        assert_eq!(cfg.api_host, None);
    }

    #[test]
    fn from_vars_reads_overrides() {
        let cfg = Config::from_vars(lookup_from(&[
            (ENV_DATA, "/tmp/data-xyz"),
            (ENV_LISTEN, "127.0.0.1:0"),
            (ENV_API, "https://registry.example"),
        ]));
        assert_eq!(cfg.data_dir, PathBuf::from("/tmp/data-xyz"));
        assert_eq!(cfg.listen, "127.0.0.1:0");
        assert_eq!(cfg.api_host.as_deref(), Some("https://registry.example"));
    }

    #[test]
    fn from_vars_treats_empty_api_as_unset() {
        let cfg = Config::from_vars(lookup_from(&[(ENV_API, "")]));
        assert_eq!(cfg.api_host, None);
    }

    #[test]
    fn from_env_smoke() {
        // Exercises the real environment-backed constructor; values are
        // whatever the harness happens to set, so only assert it builds.
        let cfg = Config::from_env();
        assert!(!cfg.listen.is_empty());
    }

    #[test]
    fn socket_addr_parses_valid_listen() {
        let cfg = Config {
            data_dir: PathBuf::from("."),
            listen: "127.0.0.1:8081".to_owned(),
            api_host: None,
        };
        assert_eq!(
            cfg.socket_addr().expect("addr"),
            "127.0.0.1:8081".parse::<SocketAddr>().unwrap()
        );
    }

    #[test]
    fn socket_addr_rejects_garbage() {
        let cfg = Config {
            data_dir: PathBuf::from("."),
            listen: "not-an-addr".to_owned(),
            api_host: None,
        };
        let err = cfg.socket_addr().expect_err("should fail");
        assert!(err.contains(ENV_LISTEN), "error names the env var: {err}");
    }

    #[test]
    fn resolve_api_host_prefers_explicit_then_derives() {
        let addr: SocketAddr = "127.0.0.1:9000".parse().unwrap();
        let explicit = Config {
            data_dir: PathBuf::from("."),
            listen: "127.0.0.1:9000".to_owned(),
            api_host: Some("https://explicit.example".to_owned()),
        };
        assert_eq!(explicit.resolve_api_host(addr), "https://explicit.example");

        let derived = Config {
            data_dir: PathBuf::from("."),
            listen: "127.0.0.1:9000".to_owned(),
            api_host: None,
        };
        assert_eq!(derived.resolve_api_host(addr), "http://127.0.0.1:9000");
    }

    /// R2-9: a wildcard listen host (`0.0.0.0`) with no explicit API host
    /// must NOT advertise a wildcard origin — the checked resolver errors
    /// out so `config.json` never carries an unfetchable `0.0.0.0` origin.
    #[test]
    fn resolve_api_host_checked_rejects_wildcard_without_explicit_api() {
        let cfg = Config {
            data_dir: PathBuf::from("."),
            listen: "0.0.0.0:8081".to_owned(),
            api_host: None,
        };
        let addr: SocketAddr = "0.0.0.0:8081".parse().unwrap();
        let err = cfg
            .resolve_api_host_checked(addr)
            .expect_err("wildcard host must be rejected");
        assert!(err.contains(ENV_API), "error names the env var: {err}");
        assert!(!err.contains("0.0.0.0/"), "must not advertise wildcard");
    }

    /// R2-9: the default config (listen `0.0.0.0:8081`, no API host) is
    /// itself wildcard, so booting it without `FERRO_CARGO_REGISTRY_API`
    /// must fail rather than advertise `http://0.0.0.0:8081`.
    #[test]
    fn default_config_without_api_is_rejected() {
        let cfg = Config::from_vars(lookup_from(&[]));
        let addr = cfg.socket_addr().expect("default listen parses");
        assert!(
            cfg.resolve_api_host_checked(addr).is_err(),
            "default wildcard listen must require an explicit API host"
        );
    }

    /// R2-9: an ephemeral port 0 is equally unfetchable and is rejected.
    #[test]
    fn resolve_api_host_checked_rejects_port_zero() {
        let cfg = Config {
            data_dir: PathBuf::from("."),
            listen: "127.0.0.1:0".to_owned(),
            api_host: None,
        };
        let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
        assert!(
            cfg.resolve_api_host_checked(addr).is_err(),
            "port 0 must be rejected when no explicit API host is set"
        );
    }

    /// R2-9: an explicit `FERRO_CARGO_REGISTRY_API` is always honoured,
    /// even with a wildcard listen — the operator opted in to that origin.
    #[test]
    fn resolve_api_host_checked_honours_explicit_with_wildcard_listen() {
        let cfg = Config {
            data_dir: PathBuf::from("."),
            listen: "0.0.0.0:8081".to_owned(),
            api_host: Some("https://registry.example".to_owned()),
        };
        let addr: SocketAddr = "0.0.0.0:8081".parse().unwrap();
        assert_eq!(
            cfg.resolve_api_host_checked(addr).expect("explicit honoured"),
            "https://registry.example"
        );
    }

    /// R2-9: a concrete non-wildcard listen with no explicit API host
    /// still derives a usable `http://<addr>` origin.
    #[test]
    fn resolve_api_host_checked_derives_for_concrete_addr() {
        let cfg = Config {
            data_dir: PathBuf::from("."),
            listen: "127.0.0.1:8081".to_owned(),
            api_host: None,
        };
        let addr: SocketAddr = "127.0.0.1:8081".parse().unwrap();
        assert_eq!(
            cfg.resolve_api_host_checked(addr).expect("derived"),
            "http://127.0.0.1:8081"
        );
    }

    #[test]
    fn build_state_creates_missing_data_dir() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let data = tmp.path().join("nested/created");
        assert!(!data.exists());
        let state = build_state(&data, "http://localhost").expect("state");
        assert!(data.is_dir(), "data dir created");
        // api_host flowed into the config.
        assert_eq!(state.config.api, "http://localhost");
    }

    #[tokio::test]
    async fn build_app_serves_probes_and_config() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let state = build_state(tmp.path(), "http://localhost").expect("state");
        let app = build_app(state);

        for (uri, expected) in [
            ("/live", StatusCode::OK),
            ("/ready", StatusCode::OK),
            ("/healthz", StatusCode::OK),
            ("/config.json", StatusCode::OK),
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
    async fn probe_routes_healthz_reports_ok_json() {
        let app = probe_routes();
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/healthz")
                    .body(Body::empty())
                    .expect("req"),
            )
            .await
            .expect("resp");
        assert_eq!(resp.status(), StatusCode::OK);
        let body = to_bytes(resp.into_body(), 4096).await.expect("body");
        assert_eq!(&body[..], br#"{"status":"ok"}"#);
    }

    #[test]
    fn init_tracing_is_idempotent() {
        // Best-effort install; calling twice must not panic.
        init_tracing();
        init_tracing();
    }

    #[tokio::test]
    async fn serve_rejects_invalid_listen_before_binding() {
        let cfg = Config {
            data_dir: PathBuf::from("."),
            listen: "definitely-not-an-addr".to_owned(),
            api_host: None,
        };
        let err = super::serve(&cfg).await.expect_err("invalid addr fails");
        assert!(err.to_string().contains(ENV_LISTEN));
    }

}
