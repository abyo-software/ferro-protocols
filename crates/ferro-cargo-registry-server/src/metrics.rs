// SPDX-License-Identifier: Apache-2.0
//! Prometheus instrumentation for `ferro-cargo-registry-server`.
//!
//! Owns a [`prometheus::Registry`] and the metric handles the server
//! exports, the axum middleware that records every HTTP request, and the
//! `GET /metrics` handler that renders the registry in the Prometheus
//! text exposition format.
//!
//! # Cardinality
//!
//! The request counter and latency histogram are labelled by `method`
//! and a *stable handler name* derived from the matched route — never the
//! raw path, whose crate names and versions would explode the series
//! count. See [`Metrics::handler_for`].
//!
//! # Exported metrics
//!
//! | Name | Type | Labels | Meaning |
//! |---|---|---|---|
//! | `ferrocargo_http_requests_total` | counter | `method`, `handler`, `status` | HTTP requests, by 3-digit status |
//! | `ferrocargo_http_request_duration_seconds` | histogram | `method`, `handler` | Request latency, seconds |
//! | `ferrocargo_in_flight` | gauge | — | HTTP requests currently being served |
//! | `ferrocargo_crates_total` | gauge | — | Distinct crate names in the index |
//! | `ferrocargo_crate_versions` | gauge | — | Distinct crate *versions* in the index |
//! | `ferrocargo_storage_bytes` | gauge | — | Best-effort store size in bytes (see note) |
//! | `ferrocargo_build_info` | gauge | `version` | Always `1`; carries the build version label |
//!
//! `ferrocargo_storage_bytes` is registered for the Grafana "Registry
//! store size" panel but stays at `0`: the in-memory index + the
//! [`ferro_blob_store::BlobStore`] trait do not expose byte sizes
//! cheaply. The honest signals today are `ferrocargo_crates_total` (one
//! per crate name) and `ferrocargo_crate_versions` (one per published
//! version), both computed exactly from the index on each scrape.

use std::time::Instant;

use axum::Router;
use axum::extract::{MatchedPath, Request, State};
use axum::http::{Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use prometheus::{
    Encoder, HistogramVec, IntCounterVec, IntGauge, Registry, TextEncoder, histogram_opts,
    opts, register_gauge_with_registry, register_histogram_vec_with_registry,
    register_int_counter_vec_with_registry, register_int_gauge_with_registry,
};

use crate::router::CargoState;

/// Histogram buckets (seconds): sub-millisecond index reads up to a
/// multi-second crate publish.
const DURATION_BUCKETS: &[f64] = &[
    0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0,
];

/// Owns the Prometheus registry and the metric handles for the server.
///
/// Cheap to clone — the `prometheus` handles are reference-counted, so
/// clones share the same underlying series.
#[derive(Clone)]
pub struct Metrics {
    registry: Registry,
    requests_total: IntCounterVec,
    request_duration: HistogramVec,
    in_flight: IntGauge,
    crates_total: IntGauge,
    crate_versions: IntGauge,
}

impl std::fmt::Debug for Metrics {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Metrics").finish_non_exhaustive()
    }
}

impl Metrics {
    /// Build a fresh registry and register every server metric on it.
    ///
    /// # Panics
    ///
    /// Panics only on a duplicate metric name, which is a programming
    /// error here (the names are constant).
    #[must_use]
    pub fn new() -> Self {
        let registry = Registry::new();

        let requests_total = register_int_counter_vec_with_registry!(
            opts!(
                "ferrocargo_http_requests_total",
                "Total HTTP requests, labelled by method, matched handler, and status code."
            ),
            &["method", "handler", "status"],
            registry
        )
        .expect("register ferrocargo_http_requests_total");

        let request_duration = register_histogram_vec_with_registry!(
            histogram_opts!(
                "ferrocargo_http_request_duration_seconds",
                "HTTP request latency in seconds, by method and matched handler.",
                DURATION_BUCKETS.to_vec()
            ),
            &["method", "handler"],
            registry
        )
        .expect("register ferrocargo_http_request_duration_seconds");

        let in_flight = register_int_gauge_with_registry!(
            opts!(
                "ferrocargo_in_flight",
                "HTTP requests currently being served (in-flight)."
            ),
            registry
        )
        .expect("register ferrocargo_in_flight");

        let crates_total = register_int_gauge_with_registry!(
            opts!(
                "ferrocargo_crates_total",
                "Distinct crate names currently held in the index."
            ),
            registry
        )
        .expect("register ferrocargo_crates_total");

        let crate_versions = register_int_gauge_with_registry!(
            opts!(
                "ferrocargo_crate_versions",
                "Distinct crate versions currently held in the index."
            ),
            registry
        )
        .expect("register ferrocargo_crate_versions");

        // Registered so the series exists for the Grafana "Registry
        // store size" panel; held only by the registry. Stays at 0 — the
        // index + BlobStore trait cannot report sizes cheaply. The crate
        // count gauges above are the honest signal today.
        let _storage_bytes = register_gauge_with_registry!(
            opts!(
                "ferrocargo_storage_bytes",
                "Best-effort store size in bytes; 0 when the backend cannot report sizes cheaply."
            ),
            registry
        )
        .expect("register ferrocargo_storage_bytes");

        let build_info = register_gauge_with_registry!(
            opts!(
                "ferrocargo_build_info",
                "Build information; constant 1 carrying the version label."
            )
            .const_label("version", env!("CARGO_PKG_VERSION")),
            registry
        )
        .expect("register ferrocargo_build_info");
        build_info.set(1.0);

        Self {
            registry,
            requests_total,
            request_duration,
            in_flight,
            crates_total,
            crate_versions,
        }
    }

    /// Map a request's matched route + path to a stable, bounded handler
    /// name. Falls back to `"other"` so the label set can never grow with
    /// attacker-controlled input (crate names / versions).
    fn handler_for(matched: Option<&str>, path: &str) -> &'static str {
        match matched {
            Some("/config.json") => "config",
            Some(
                "/index/{*path}" | "/{prefix}/{name}" | "/{p0}/{p1}/{name}",
            ) => "index",
            Some("/index.git/{*path}") => "git_index",
            Some("/api/v1/crates/new") => "publish",
            Some("/api/v1/crates/{name}/{version}/download") => "download",
            Some("/api/v1/crates/{name}/{version}/yank") => "yank",
            Some("/api/v1/crates/{name}/{version}/unyank") => "unyank",
            Some("/api/v1/crates/{name}/owners") => "owners",
            Some("/live") => "live",
            Some("/ready") => "ready",
            Some("/healthz") => "healthz",
            Some("/metrics") => "metrics",
            _ => {
                // Fall back to coarse path-prefix classification for
                // anything without a matched route.
                if path.starts_with("/index") {
                    "index"
                } else if path.starts_with("/api/v1/crates") {
                    "crates_api"
                } else {
                    "other"
                }
            }
        }
    }

    /// Refresh the crate-count gauges from the registry index.
    ///
    /// `crates_total` counts distinct crate names; `crate_versions` sums
    /// every published version across all crates.
    pub async fn refresh_storage(&self, state: &CargoState) {
        let crates = state.crates.read().await;
        let name_count = crates.len();
        let version_count = crates.values().map(|r| r.entries.len()).sum::<usize>();
        drop(crates);

        self.crates_total
            .set(i64::try_from(name_count).unwrap_or(i64::MAX));
        self.crate_versions
            .set(i64::try_from(version_count).unwrap_or(i64::MAX));
    }

    /// Render the registry in the Prometheus text exposition format.
    fn encode(&self) -> String {
        let mut buf = Vec::new();
        let encoder = TextEncoder::new();
        if encoder.encode(&self.registry.gather(), &mut buf).is_ok() {
            String::from_utf8(buf).unwrap_or_default()
        } else {
            String::new()
        }
    }
}

impl Default for Metrics {
    fn default() -> Self {
        Self::new()
    }
}

/// State threaded through the metrics middleware + `/metrics` handler.
#[derive(Clone)]
pub struct MetricsState {
    /// The metric handles.
    pub metrics: Metrics,
    /// Registry state, sampled on each scrape for the crate-count gauges.
    pub cargo: CargoState,
}

/// Axum middleware: record count + latency + in-flight for every request.
pub async fn track_metrics(
    State(state): State<MetricsState>,
    request: Request,
    next: Next,
) -> Response {
    let method = request.method().clone();
    let matched = request
        .extensions()
        .get::<MatchedPath>()
        .map(|m| m.as_str().to_owned());
    let path = request.uri().path().to_owned();
    let handler = Metrics::handler_for(matched.as_deref(), &path);

    let m = &state.metrics;
    m.in_flight.inc();
    let started = Instant::now();
    let response = next.run(request).await;
    let elapsed = started.elapsed().as_secs_f64();
    m.in_flight.dec();

    let method_str = method_label(&method);
    let status = response.status().as_u16().to_string();
    m.requests_total
        .with_label_values(&[method_str, handler, &status])
        .inc();
    m.request_duration
        .with_label_values(&[method_str, handler])
        .observe(elapsed);

    response
}

/// Map a [`Method`] to a stable static label, defaulting to `"OTHER"`.
const fn method_label(method: &Method) -> &'static str {
    match *method {
        Method::GET => "GET",
        Method::HEAD => "HEAD",
        Method::POST => "POST",
        Method::PUT => "PUT",
        Method::PATCH => "PATCH",
        Method::DELETE => "DELETE",
        Method::OPTIONS => "OPTIONS",
        _ => "OTHER",
    }
}

/// `GET /metrics` handler — refreshes the crate-count gauges, then renders
/// the registry in the Prometheus text exposition format.
async fn metrics_handler(State(state): State<MetricsState>) -> Response {
    state.metrics.refresh_storage(&state.cargo).await;
    let body = state.metrics.encode();
    (
        StatusCode::OK,
        [(
            axum::http::header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )],
        body,
    )
        .into_response()
}

/// Build a router exposing `GET /metrics`, backed by the given state.
///
/// Merge this into the server router so `/metrics` is served on the same
/// port as the protocol surface (matching the Helm `ServiceMonitor`,
/// which scrapes the `http` port at `/metrics`).
pub fn metrics_routes(state: MetricsState) -> Router {
    Router::new()
        .route("/metrics", get(metrics_handler))
        .with_state(state)
}

/// Wrap `app` so every request is instrumented and `/metrics` is served.
///
/// The middleware records count/latency/in-flight on `app`'s routes; the
/// merged `/metrics` route renders them. `cargo` is sampled on each scrape
/// for the crate-count gauges.
pub fn instrument(app: Router, metrics: Metrics, cargo: CargoState) -> Router {
    let state = MetricsState { metrics, cargo };
    app.layer(axum::middleware::from_fn_with_state(
        state.clone(),
        track_metrics,
    ))
    .merge(metrics_routes(state))
}

#[cfg(test)]
mod tests {
    use super::Metrics;

    #[test]
    fn handler_for_classifies_known_routes() {
        assert_eq!(
            Metrics::handler_for(Some("/config.json"), "/config.json"),
            "config"
        );
        assert_eq!(
            Metrics::handler_for(Some("/api/v1/crates/new"), "/api/v1/crates/new"),
            "publish"
        );
        assert_eq!(
            Metrics::handler_for(
                Some("/api/v1/crates/{name}/{version}/download"),
                "/api/v1/crates/serde/1.0.0/download"
            ),
            "download"
        );
        assert_eq!(
            Metrics::handler_for(Some("/index/{*path}"), "/index/se/rd/serde"),
            "index"
        );
        assert_eq!(Metrics::handler_for(Some("/healthz"), "/healthz"), "healthz");
    }

    #[test]
    fn handler_for_unmatched_is_bounded() {
        assert_eq!(Metrics::handler_for(None, "/index/anything"), "index");
        assert_eq!(
            Metrics::handler_for(None, "/api/v1/crates/foo"),
            "crates_api"
        );
        assert_eq!(Metrics::handler_for(None, "/totally/unknown"), "other");
    }

    #[test]
    fn encode_emits_prometheus_text_format() {
        let m = Metrics::new();
        m.requests_total
            .with_label_values(&["GET", "config", "200"])
            .inc();
        m.request_duration
            .with_label_values(&["GET", "config"])
            .observe(0.01);
        let text = m.encode();
        assert!(text.contains("# HELP ferrocargo_http_requests_total"));
        assert!(text.contains("# TYPE ferrocargo_http_requests_total counter"));
        assert!(text.contains("ferrocargo_build_info"));
        assert!(text.contains("ferrocargo_crate_versions"));
        assert!(text.contains("ferrocargo_http_request_duration_seconds"));
        assert!(text.contains(r#"handler="config""#));
    }
}
