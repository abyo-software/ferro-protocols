// SPDX-License-Identifier: Apache-2.0
//! Prometheus instrumentation for `ferro-oci-server`.
//!
//! This module owns a [`prometheus::Registry`] and the metric handles
//! the server exports, plus the axum middleware that records every HTTP
//! request and the `GET /metrics` handler that renders the registry in
//! the Prometheus text exposition format.
//!
//! # Cardinality
//!
//! Labels are deliberately low-cardinality. The request counter and the
//! latency histogram are labelled by `method` and a *stable handler
//! name* derived from the matched route — never the raw path, whose
//! digests, crate names, and upload UUIDs would explode the series
//! count. See [`Metrics::handler_for`].
//!
//! # Exported metrics
//!
//! | Name | Type | Labels | Meaning |
//! |---|---|---|---|
//! | `ferrooci_http_requests_total` | counter | `method`, `handler`, `status` | HTTP requests, by 3-digit status |
//! | `ferrooci_http_request_duration_seconds` | histogram | `method`, `handler` | Wall-clock request latency, seconds |
//! | `ferrooci_uploads_in_flight` | gauge | — | HTTP requests currently being served |
//! | `ferrooci_storage_blobs` | gauge | — | Distinct blobs in the blob store |
//! | `ferrooci_storage_bytes` | gauge | — | Best-effort blob-store size in bytes (see note) |
//! | `ferrooci_build_info` | gauge | `version` | Always `1`; carries the build version label |
//!
//! `ferrooci_storage_bytes` is only populated when the blob store can
//! report per-blob sizes cheaply; with the current
//! [`ferro_blob_store::BlobStore`] trait (which exposes a blob *list*
//! but not sizes) the gauge is left at `0` and `ferrooci_storage_blobs`
//! is the honest, exact signal. The Grafana dashboard's "Blob store
//! size" panel therefore reads `0` until a size-reporting backend is
//! wired; the blob-count gauge is the one to trust today.

use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Instant;

use axum::Router;
use axum::extract::{MatchedPath, Request, State};
use axum::http::{Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use prometheus::{
    Encoder, HistogramVec, IntCounterVec, IntGauge, Registry, TextEncoder,
    histogram_opts, opts, register_gauge_with_registry,
    register_histogram_vec_with_registry, register_int_counter_vec_with_registry,
    register_int_gauge_with_registry,
};

/// Histogram buckets (seconds) tuned for a registry: sub-millisecond
/// metadata reads up to multi-second blob transfers.
const DURATION_BUCKETS: &[f64] = &[
    0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0,
];

/// Owns the Prometheus registry and the metric handles for the server.
///
/// Cheap to clone — every field is reference-counted internally by the
/// `prometheus` crate, so clones share the same underlying series.
#[derive(Clone)]
pub struct Metrics {
    registry: Registry,
    requests_total: IntCounterVec,
    request_duration: HistogramVec,
    in_flight: IntGauge,
    storage_blobs: IntGauge,
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
    /// Panics only if two metrics with the same name are registered,
    /// which is a programming error here (the names are constant).
    #[must_use]
    pub fn new() -> Self {
        let registry = Registry::new();

        let requests_total = register_int_counter_vec_with_registry!(
            opts!(
                "ferrooci_http_requests_total",
                "Total HTTP requests, labelled by method, matched handler, and status code."
            ),
            &["method", "handler", "status"],
            registry
        )
        .expect("register ferrooci_http_requests_total");

        let request_duration = register_histogram_vec_with_registry!(
            histogram_opts!(
                "ferrooci_http_request_duration_seconds",
                "HTTP request latency in seconds, by method and matched handler.",
                DURATION_BUCKETS.to_vec()
            ),
            &["method", "handler"],
            registry
        )
        .expect("register ferrooci_http_request_duration_seconds");

        let in_flight = register_int_gauge_with_registry!(
            opts!(
                "ferrooci_uploads_in_flight",
                "HTTP requests currently being served (in-flight)."
            ),
            registry
        )
        .expect("register ferrooci_uploads_in_flight");

        let storage_blobs = register_int_gauge_with_registry!(
            opts!(
                "ferrooci_storage_blobs",
                "Distinct blobs currently held in the blob store."
            ),
            registry
        )
        .expect("register ferrooci_storage_blobs");

        // Registered so the series exists for the Grafana "Blob store
        // size" panel; held only by the registry. It stays at 0 because
        // the BlobStore trait cannot report sizes cheaply — the blob
        // *count* gauge above is the honest signal today.
        let _storage_bytes = register_gauge_with_registry!(
            opts!(
                "ferrooci_storage_bytes",
                "Best-effort blob-store size in bytes; 0 when the backend cannot report sizes cheaply."
            ),
            registry
        )
        .expect("register ferrooci_storage_bytes");

        let build_info = register_gauge_with_registry!(
            opts!(
                "ferrooci_build_info",
                "Build information; constant 1 carrying the version label."
            )
            .const_label("version", env!("CARGO_PKG_VERSION")),
            registry
        )
        .expect("register ferrooci_build_info");
        build_info.set(1.0);

        Self {
            registry,
            requests_total,
            request_duration,
            in_flight,
            storage_blobs,
        }
    }

    /// Map a request's matched route + path tail to a stable, bounded
    /// handler name. Falls back to `"other"` for anything unmatched so
    /// the label set can never grow with attacker-controlled input.
    fn handler_for(matched: Option<&str>, path: &str) -> &'static str {
        // The OCI surface is a single `/v2/{*rest}` wildcard plus a few
        // static routes, so we classify on the path *tail shape* rather
        // than the (constant) matched pattern.
        match matched {
            Some("/v2/" | "/v2") => "version",
            Some("/v2/_catalog") => "catalog",
            Some("/live") => "live",
            Some("/ready") => "ready",
            Some("/healthz") => "healthz",
            Some("/metrics") => "metrics",
            _ => {
                if path.ends_with("/tags/list") {
                    "tags"
                } else if path.contains("/referrers/") {
                    "referrers"
                } else if path.contains("/manifests/") {
                    "manifests"
                } else if path.contains("/blobs/uploads") {
                    "uploads"
                } else if path.contains("/blobs/") {
                    "blobs"
                } else {
                    "other"
                }
            }
        }
    }

    /// Refresh the storage gauges from the incremental blob counter.
    ///
    /// Reads the O(1) atomic blob count maintained by the blob handlers
    /// (see [`crate::router::AppState`]) rather than performing an
    /// O(number-of-blobs) `BlobStore::list()` filesystem scan on every
    /// scrape — an open `/metrics` endpoint must not amplify into heavy
    /// storage work. The gauge reports "blobs written via this server
    /// instance"; see the `AppState::blob_count` field docs for the exact
    /// best-effort semantics.
    pub fn refresh_storage(&self, blob_count: i64) {
        self.storage_blobs.set(blob_count);
    }

    /// Render the registry in the Prometheus text exposition format.
    fn encode(&self) -> String {
        let mut buf = Vec::new();
        let encoder = TextEncoder::new();
        // Encoding to a Vec only fails on a write error, which an
        // in-memory Vec cannot produce.
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
    /// Incremental blob counter, read on each `/metrics` scrape for the
    /// `ferrooci_storage_blobs` gauge. Shared with
    /// [`crate::router::AppState`] so the count the handlers maintain is
    /// the count the scrape reports — no filesystem scan required.
    pub blob_count: Arc<AtomicI64>,
}

/// Axum middleware: record count + latency + in-flight for every request.
///
/// Uses [`MatchedPath`] (the route *pattern*, not the raw path) so the
/// label set stays bounded regardless of request input.
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
fn method_label(method: &Method) -> &'static str {
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

/// `GET /metrics` handler — refreshes the storage gauges, then renders
/// the registry in the Prometheus text exposition format.
async fn metrics_handler(State(state): State<MetricsState>) -> Response {
    state
        .metrics
        .refresh_storage(state.blob_count.load(Ordering::Relaxed));
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
/// The `/metrics` route is merged into `app` *before* the tracking
/// middleware is layered over the combined router, so a `/metrics` scrape
/// is itself counted under the `metrics` handler label (matching the
/// label emitted by [`Metrics::handler_for`]). `blob_count` is the shared
/// incremental counter (from [`crate::router::AppState`]) read on each
/// scrape for the storage gauges.
pub fn instrument(app: Router, metrics: Metrics, blob_count: Arc<AtomicI64>) -> Router {
    let state = MetricsState {
        metrics,
        blob_count,
    };
    app.merge(metrics_routes(state.clone()))
        .layer(axum::middleware::from_fn_with_state(state, track_metrics))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handler_for_classifies_known_routes() {
        assert_eq!(Metrics::handler_for(Some("/v2/"), "/v2/"), "version");
        assert_eq!(
            Metrics::handler_for(Some("/v2/_catalog"), "/v2/_catalog"),
            "catalog"
        );
        assert_eq!(Metrics::handler_for(Some("/healthz"), "/healthz"), "healthz");
    }

    #[test]
    fn handler_for_classifies_wildcard_tails() {
        let m = Some("/v2/{*rest}");
        assert_eq!(Metrics::handler_for(m, "/v2/alpine/tags/list"), "tags");
        assert_eq!(
            Metrics::handler_for(m, "/v2/alpine/manifests/latest"),
            "manifests"
        );
        assert_eq!(
            Metrics::handler_for(m, "/v2/alpine/blobs/uploads/abc"),
            "uploads"
        );
        assert_eq!(
            Metrics::handler_for(m, "/v2/alpine/blobs/sha256:deadbeef"),
            "blobs"
        );
        assert_eq!(
            Metrics::handler_for(m, "/v2/alpine/referrers/sha256:abcd"),
            "referrers"
        );
    }

    #[test]
    fn handler_for_unmatched_is_bounded_to_other() {
        assert_eq!(Metrics::handler_for(None, "/random/garbage/path"), "other");
    }

    #[test]
    fn encode_emits_prometheus_text_format() {
        let m = Metrics::new();
        m.requests_total
            .with_label_values(&["GET", "version", "200"])
            .inc();
        m.request_duration
            .with_label_values(&["GET", "version"])
            .observe(0.01);
        let text = m.encode();
        assert!(text.contains("# HELP ferrooci_http_requests_total"));
        assert!(text.contains("# TYPE ferrooci_http_requests_total counter"));
        assert!(text.contains("ferrooci_build_info"));
        assert!(text.contains("ferrooci_http_request_duration_seconds"));
        assert!(text.contains(r#"handler="version""#));
    }
}
