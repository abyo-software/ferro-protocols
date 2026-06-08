// SPDX-License-Identifier: Apache-2.0
//! Integration tests for the Prometheus `/metrics` endpoint.
//!
//! Drives a real request through the instrumented router, then scrapes
//! `/metrics` and asserts the Prometheus text-exposition format plus that
//! the request counter incremented for the route just hit.

use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use ferro_blob_store::{Digest, InMemoryBlobStore, SharedBlobStore};
use ferro_oci_server::{AppState, InMemoryRegistryMeta, Metrics, instrument, router};
use http_body_util::BodyExt;
use tower::ServiceExt;

fn instrumented_app() -> Router {
    let blob_store: SharedBlobStore = Arc::new(InMemoryBlobStore::new());
    let registry = Arc::new(InMemoryRegistryMeta::new());
    let state = AppState::new(blob_store, registry);
    let blob_count = state.blob_count_handle();
    instrument(router(state), Metrics::new(), blob_count)
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

/// Read the `ferrooci_storage_blobs` gauge value from a scrape body.
fn storage_blobs_value(body: &str) -> i64 {
    body.lines()
        .find_map(|l| l.strip_prefix("ferrooci_storage_blobs "))
        .and_then(|v| v.trim().parse::<i64>().ok())
        .expect("storage_blobs gauge present")
}

/// Monolithically push a blob and assert `201 Created`.
async fn put_blob(app: &Router, payload: &[u8]) -> String {
    let digest = Digest::sha256_of(payload).to_string();
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/v2/repo/blobs/uploads/?digest={digest}"))
                .body(Body::from(payload.to_vec()))
                .unwrap(),
        )
        .await
        .expect("put blob");
    assert_eq!(resp.status(), StatusCode::CREATED, "blob push");
    digest
}

/// F8 regression: the `ferrooci_storage_blobs` gauge must be maintained
/// by an incremental counter (no `BlobStore::list()` scan per scrape),
/// incrementing on a blob put and decrementing on a blob delete.
#[tokio::test]
async fn storage_blob_gauge_increments_on_put_and_decrements_on_delete() {
    let app = instrumented_app();

    // Start at 0.
    let (_, body) = body_string(&app, "/metrics").await;
    assert_eq!(storage_blobs_value(&body), 0, "empty store");

    // Put two distinct blobs → gauge is 2.
    let d1 = put_blob(&app, b"blob-one").await;
    let _d2 = put_blob(&app, b"blob-two").await;
    let (_, body) = body_string(&app, "/metrics").await;
    assert_eq!(storage_blobs_value(&body), 2, "after two puts");

    // Delete one → gauge is 1.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::DELETE)
                .uri(format!("/v2/repo/blobs/{d1}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("delete blob");
    assert_eq!(resp.status(), StatusCode::ACCEPTED, "blob delete");
    let (_, body) = body_string(&app, "/metrics").await;
    assert_eq!(storage_blobs_value(&body), 1, "after one delete");
}

/// R2-4 regression: the `ferrooci_storage_blobs` gauge tracks *distinct*
/// blobs currently held. PUTting the same digest twice must increment it
/// exactly once; a single later delete must then return it to 0 (not -ve
/// and not stuck at 1). Previously the gauge incremented on every PUT,
/// over-counting duplicates and contradicting the "distinct blobs" help.
#[tokio::test]
async fn storage_blob_gauge_does_not_double_count_duplicate_put() {
    let app = instrumented_app();

    // Start at 0.
    let (_, body) = body_string(&app, "/metrics").await;
    assert_eq!(storage_blobs_value(&body), 0, "empty store");

    // Push the SAME blob twice → still one distinct blob → gauge is 1.
    let d = put_blob(&app, b"duplicate-blob").await;
    let d_again = put_blob(&app, b"duplicate-blob").await;
    assert_eq!(d, d_again, "same payload yields same digest");
    let (_, body) = body_string(&app, "/metrics").await;
    assert_eq!(
        storage_blobs_value(&body),
        1,
        "duplicate put must not double-count distinct blobs"
    );

    // Delete the (single) blob once → gauge back to 0.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::DELETE)
                .uri(format!("/v2/repo/blobs/{d}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("delete blob");
    assert_eq!(resp.status(), StatusCode::ACCEPTED, "blob delete");
    let (_, body) = body_string(&app, "/metrics").await;
    assert_eq!(
        storage_blobs_value(&body),
        0,
        "after deleting the one distinct blob the gauge must be 0, not 1"
    );
}

/// F7 regression: `/metrics` requests are themselves counted under the
/// `metrics` handler label — the middleware is layered *over* the merged
/// `/metrics` route, not under it.
#[tokio::test]
async fn metrics_endpoint_is_self_counted() {
    let app = instrumented_app();

    // First scrape records nothing about `/metrics` yet (it is observed
    // only after it completes), so issue two scrapes: the second sees the
    // first counted.
    let _ = body_string(&app, "/metrics").await;
    let (_, body) = body_string(&app, "/metrics").await;

    let counted = body.lines().any(|line| {
        line.starts_with("ferrooci_http_requests_total")
            && line.contains(r#"handler="metrics""#)
            && line.contains(r#"method="GET""#)
            && line.contains(r#"status="200""#)
    });
    assert!(
        counted,
        "expected /metrics scrape to be self-counted under handler=\"metrics\", got:\n{body}"
    );
}
