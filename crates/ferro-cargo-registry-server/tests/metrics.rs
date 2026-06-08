// SPDX-License-Identifier: Apache-2.0
//! Integration tests for the Prometheus `/metrics` endpoint.
//!
//! Drives a real request through the instrumented router, then scrapes
//! `/metrics` and asserts the Prometheus text-exposition format, that the
//! request counter incremented, and that the crate-count gauge tracks a
//! published crate.

use std::sync::Arc;

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Method, Request, StatusCode};
use ferro_blob_store::FsBlobStore;
use ferro_cargo_registry_server::{CargoState, Metrics, encode_publish_body, instrument, router};
use serde_json::json;
use sha2::{Digest as _, Sha256};
use tempfile::TempDir;
use tower::ServiceExt;

fn setup() -> (Router, TempDir) {
    let tmp = TempDir::new().expect("tempdir");
    let store = Arc::new(FsBlobStore::new(tmp.path()).expect("blob store"));
    let state = CargoState::new(store, "http://localhost");
    let app = instrument(router(state.clone()), Metrics::new(), state);
    (app, tmp)
}

async fn get_string(app: &Router, uri: &str) -> (StatusCode, String) {
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(uri)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("response");
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.expect("body");
    (status, String::from_utf8(bytes.to_vec()).expect("utf8"))
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    hex::encode(h.finalize())
}

fn publish_body(name: &str, version: &str, tarball: &[u8]) -> Vec<u8> {
    let manifest = json!({
        "name": name,
        "vers": version,
        "deps": [],
        "features": {},
        "authors": ["abyo"],
        "description": "metrics test crate",
        "cksum": sha256_hex(tarball),
    });
    encode_publish_body(&manifest, tarball)
}

#[tokio::test]
async fn metrics_endpoint_emits_prometheus_text_format() {
    let (app, _tmp) = setup();

    // Drive a real request so the histogram has an observed series.
    let _ = get_string(&app, "/config.json").await;

    let (status, body) = get_string(&app, "/metrics").await;
    assert_eq!(status, StatusCode::OK);

    assert!(
        body.contains("# HELP ferrocargo_http_requests_total"),
        "missing HELP:\n{body}"
    );
    assert!(
        body.contains("# TYPE ferrocargo_http_requests_total counter"),
        "missing TYPE:\n{body}"
    );
    assert!(
        body.contains("# TYPE ferrocargo_http_request_duration_seconds histogram"),
        "missing histogram TYPE:\n{body}"
    );
    assert!(body.contains("ferrocargo_build_info"), "missing build_info");
    assert!(
        body.contains("ferrocargo_crate_versions"),
        "missing crate_versions gauge"
    );
}

#[tokio::test]
async fn request_counter_increments_for_hit_route() {
    let (app, _tmp) = setup();

    let (status, _) = get_string(&app, "/config.json").await;
    assert_eq!(status, StatusCode::OK);

    let (_, body) = get_string(&app, "/metrics").await;
    let counted = body.lines().any(|line| {
        line.starts_with("ferrocargo_http_requests_total")
            && line.contains(r#"handler="config""#)
            && line.contains(r#"method="GET""#)
            && line.contains(r#"status="200""#)
            && line.trim_end().ends_with(" 1")
    });
    assert!(
        counted,
        "expected +1 sample for GET config 200, got:\n{body}"
    );
}

/// R2-3 regression: a `/metrics` scrape is itself instrumented. The
/// module docs claim every HTTP request is recorded, so two `GET
/// /metrics` calls must leave a `handler="metrics"` series in the
/// counter (the first scrape is counted and visible to the second).
#[tokio::test]
async fn metrics_scrape_is_self_instrumented() {
    let (app, _tmp) = setup();

    // First scrape — counted by the middleware that now wraps /metrics.
    let (status, _) = get_string(&app, "/metrics").await;
    assert_eq!(status, StatusCode::OK);

    // Second scrape observes the first scrape's counter sample.
    let (_, body) = get_string(&app, "/metrics").await;
    let counted = body.lines().any(|line| {
        line.starts_with("ferrocargo_http_requests_total")
            && line.contains(r#"handler="metrics""#)
            && line.contains(r#"method="GET""#)
            && line.contains(r#"status="200""#)
    });
    assert!(
        counted,
        "expected a handler=\"metrics\" counter sample, got:\n{body}"
    );
}

#[tokio::test]
async fn crate_version_gauge_tracks_published_crate() {
    let (app, _tmp) = setup();

    // Empty index → gauges read 0.
    let (_, body) = get_string(&app, "/metrics").await;
    assert!(
        body.lines().any(|l| l == "ferrocargo_crate_versions 0"),
        "expected crate_versions 0 on empty index:\n{body}"
    );

    // Publish a crate.
    let tarball = b"\x1f\x8b\x08\x00fake-tarball-bytes";
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri("/api/v1/crates/new")
                .body(Body::from(publish_body("metricscrate", "1.0.0", tarball)))
                .unwrap(),
        )
        .await
        .expect("publish response");
    assert_eq!(resp.status(), StatusCode::OK, "publish should succeed");

    // Re-scrape: the version gauge now reads 1.
    let (_, body) = get_string(&app, "/metrics").await;
    assert!(
        body.lines().any(|l| l == "ferrocargo_crate_versions 1"),
        "expected crate_versions 1 after publish:\n{body}"
    );
    assert!(
        body.lines().any(|l| l == "ferrocargo_crates_total 1"),
        "expected crates_total 1 after publish:\n{body}"
    );
}
