// SPDX-License-Identifier: Apache-2.0
//! Integration tests for the Prometheus `/metrics` endpoint.
//!
//! Drives a real request through the instrumented router, then scrapes
//! `/metrics` and asserts the Prometheus text-exposition format plus that
//! the request counter incremented for the route just hit.

use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use ferro_blob_store::{InMemoryBlobStore, SharedBlobStore};
use ferro_oci_server::{AppState, InMemoryRegistryMeta, Metrics, instrument, router};
use http_body_util::BodyExt;
use tower::ServiceExt;

fn instrumented_app() -> Router {
    let blob_store: SharedBlobStore = Arc::new(InMemoryBlobStore::new());
    let registry = Arc::new(InMemoryRegistryMeta::new());
    let state = AppState::new(blob_store.clone(), registry);
    instrument(router(state), Metrics::new(), blob_store)
}

async fn body_string(app: &Router, uri: &str) -> (StatusCode, String) {
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
        .expect("oneshot");
    let status = resp.status();
    let bytes = resp.into_body().collect().await.expect("collect").to_bytes();
    (status, String::from_utf8(bytes.to_vec()).expect("utf8"))
}

#[tokio::test]
async fn metrics_endpoint_emits_prometheus_text_format() {
    let app = instrumented_app();

    // Drive one real request so the histogram has an observed series
    // (Prometheus omits HELP/TYPE for never-observed label sets).
    let _ = body_string(&app, "/v2/").await;

    let (status, body) = body_string(&app, "/metrics").await;
    assert_eq!(status, StatusCode::OK);

    // Prometheus text exposition format markers.
    assert!(
        body.contains("# HELP ferrooci_http_requests_total"),
        "missing HELP line:\n{body}"
    );
    assert!(
        body.contains("# TYPE ferrooci_http_requests_total counter"),
        "missing TYPE line:\n{body}"
    );
    assert!(
        body.contains("# TYPE ferrooci_http_request_duration_seconds histogram"),
        "missing histogram TYPE:\n{body}"
    );
    // build_info + storage gauges exist.
    assert!(body.contains("ferrooci_build_info"), "missing build_info");
    assert!(
        body.contains("ferrooci_storage_blobs"),
        "missing storage_blobs gauge"
    );
}

#[tokio::test]
async fn request_counter_increments_for_hit_route() {
    let app = instrumented_app();

    // Hit a real protocol endpoint (the `/v2/` version check).
    let (status, _) = body_string(&app, "/v2/").await;
    assert_eq!(status, StatusCode::OK);

    // Scrape and confirm the counter recorded the GET on the `version`
    // handler with a 200 status.
    let (_, body) = body_string(&app, "/metrics").await;
    let counted = body.lines().any(|line| {
        line.starts_with("ferrooci_http_requests_total")
            && line.contains(r#"handler="version""#)
            && line.contains(r#"method="GET""#)
            && line.contains(r#"status="200""#)
            && line.trim_end().ends_with(" 1")
    });
    assert!(
        counted,
        "expected a +1 sample for GET version 200, got:\n{body}"
    );
}

#[tokio::test]
async fn storage_blob_gauge_reflects_stored_blobs() {
    let app = instrumented_app();
    // No blobs yet → gauge is 0.
    let (_, body) = body_string(&app, "/metrics").await;
    assert!(
        body.lines().any(|l| l == "ferrooci_storage_blobs 0"),
        "expected storage_blobs 0 on empty store:\n{body}"
    );
}
