// SPDX-License-Identifier: Apache-2.0
//! End-to-end Axum tests for the Cargo registry protocol.

use std::sync::Arc;

use axum::body::{Body, to_bytes};
use axum::http::{Method, Request, StatusCode, header};
use ferro_blob_store::FsBlobStore;
use ferro_cargo_registry_server::{CargoState, encode_publish_body, router};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use tempfile::TempDir;
use tower::ServiceExt;

fn setup() -> (axum::Router, TempDir) {
    let tmp = TempDir::new().expect("tempdir");
    let store = Arc::new(FsBlobStore::new(tmp.path()).expect("blob store"));
    let state = CargoState::new(store, "http://localhost");
    (router(state), tmp)
}

async fn send(app: &axum::Router, req: Request<Body>) -> axum::http::Response<Body> {
    app.clone().oneshot(req).await.expect("response")
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    hex::encode(h.finalize())
}

fn publish_body(name: &str, version: &str, tarball: &[u8]) -> Vec<u8> {
    let cksum = sha256_hex(tarball);
    let manifest = json!({
        "name": name,
        "vers": version,
        "deps": [],
        "features": {},
        "authors": ["abyo"],
        "description": "phase 1 test crate",
        "cksum": cksum,
    });
    encode_publish_body(&manifest, tarball)
}

#[tokio::test]
async fn config_json_round_trip() {
    let (app, _tmp) = setup();
    let resp = send(
        &app,
        Request::builder()
            .method(Method::GET)
            .uri("/config.json")
            .body(Body::empty())
            .expect("build"),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = to_bytes(resp.into_body(), 4096).await.expect("body");
    let v: Value = serde_json::from_slice(&body).expect("parse");
    assert!(v["dl"].as_str().unwrap_or_default().contains("{crate}"));
    assert_eq!(v["auth-required"], false);
}

#[tokio::test]
async fn publish_then_download_tarball() {
    let (app, _tmp) = setup();
    let tarball: &[u8] = b"crate-tarball-contents";
    let body = publish_body("foo", "1.0.0", tarball);
    let resp = send(
        &app,
        Request::builder()
            .method(Method::PUT)
            .uri("/api/v1/crates/new")
            .body(Body::from(body))
            .expect("build"),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = send(
        &app,
        Request::builder()
            .method(Method::GET)
            .uri("/api/v1/crates/foo/1.0.0/download")
            .body(Body::empty())
            .expect("build"),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let got = to_bytes(resp.into_body(), 4096).await.expect("body");
    assert_eq!(got.as_ref(), tarball);
}

#[tokio::test]
async fn publish_then_sparse_index_lists_entry() {
    let (app, _tmp) = setup();
    let tarball: &[u8] = b"tarball-1";
    send(
        &app,
        Request::builder()
            .method(Method::PUT)
            .uri("/api/v1/crates/new")
            .body(Body::from(publish_body("serde", "1.0.0", tarball)))
            .expect("build"),
    )
    .await;

    let resp = send(
        &app,
        Request::builder()
            .method(Method::GET)
            .uri("/index/se/rd/serde")
            .body(Body::empty())
            .expect("build"),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = to_bytes(resp.into_body(), 4096).await.expect("body");
    let text = std::str::from_utf8(&body).expect("utf8");
    let first_line = text.lines().next().expect("line");
    let v: Value = serde_json::from_str(first_line).expect("parse");
    assert_eq!(v["name"], "serde");
    assert_eq!(v["vers"], "1.0.0");
    assert_eq!(v["yanked"], false);
}

#[tokio::test]
async fn yank_then_unyank_flips_index_flag() {
    let (app, _tmp) = setup();
    send(
        &app,
        Request::builder()
            .method(Method::PUT)
            .uri("/api/v1/crates/new")
            .body(Body::from(publish_body("serde", "1.0.0", b"x")))
            .expect("build"),
    )
    .await;
    // Yank.
    let resp = send(
        &app,
        Request::builder()
            .method(Method::DELETE)
            .uri("/api/v1/crates/serde/1.0.0/yank")
            .body(Body::empty())
            .expect("build"),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);

    // Sparse index shows yanked: true.
    let resp = send(
        &app,
        Request::builder()
            .method(Method::GET)
            .uri("/index/se/rd/serde")
            .body(Body::empty())
            .expect("build"),
    )
    .await;
    let body = to_bytes(resp.into_body(), 4096).await.expect("body");
    let first = std::str::from_utf8(&body)
        .unwrap()
        .lines()
        .next()
        .unwrap()
        .to_owned();
    let v: Value = serde_json::from_str(&first).unwrap();
    assert_eq!(v["yanked"], true);

    // Unyank.
    let resp = send(
        &app,
        Request::builder()
            .method(Method::PUT)
            .uri("/api/v1/crates/serde/1.0.0/unyank")
            .body(Body::empty())
            .expect("build"),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = send(
        &app,
        Request::builder()
            .method(Method::GET)
            .uri("/index/se/rd/serde")
            .body(Body::empty())
            .expect("build"),
    )
    .await;
    let body = to_bytes(resp.into_body(), 4096).await.expect("body");
    let first = std::str::from_utf8(&body)
        .unwrap()
        .lines()
        .next()
        .unwrap()
        .to_owned();
    let v: Value = serde_json::from_str(&first).unwrap();
    assert_eq!(v["yanked"], false);
}

#[tokio::test]
async fn owners_add_list_remove() {
    let (app, _tmp) = setup();
    send(
        &app,
        Request::builder()
            .method(Method::PUT)
            .uri("/api/v1/crates/new")
            .body(Body::from(publish_body("foo", "1.0.0", b"x")))
            .expect("build"),
    )
    .await;

    // Add two owners.
    let resp = send(
        &app,
        Request::builder()
            .method(Method::PUT)
            .uri("/api/v1/crates/foo/owners")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                serde_json::to_vec(&json!({"users": ["alice", "bob"]})).unwrap(),
            ))
            .expect("build"),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);

    // List.
    let resp = send(
        &app,
        Request::builder()
            .method(Method::GET)
            .uri("/api/v1/crates/foo/owners")
            .body(Body::empty())
            .expect("build"),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = to_bytes(resp.into_body(), 4096).await.expect("body");
    let v: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["users"].as_array().unwrap().len(), 2);

    // Remove alice.
    let resp = send(
        &app,
        Request::builder()
            .method(Method::DELETE)
            .uri("/api/v1/crates/foo/owners")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                serde_json::to_vec(&json!({"users": ["alice"]})).unwrap(),
            ))
            .expect("build"),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);

    // Remaining owner is bob.
    let resp = send(
        &app,
        Request::builder()
            .method(Method::GET)
            .uri("/api/v1/crates/foo/owners")
            .body(Body::empty())
            .expect("build"),
    )
    .await;
    let body = to_bytes(resp.into_body(), 4096).await.expect("body");
    let v: Value = serde_json::from_slice(&body).unwrap();
    let users = v["users"].as_array().unwrap();
    assert_eq!(users.len(), 1);
    assert_eq!(users[0]["login"], "bob");
}

#[tokio::test]
async fn checksum_mismatch_rejected() {
    let (app, _tmp) = setup();
    let tarball: &[u8] = b"real";
    let manifest = json!({
        "name": "foo",
        "vers": "1.0.0",
        "deps": [],
        "features": {},
        "cksum": "deadbeef",
    });
    let body = encode_publish_body(&manifest, tarball);
    let resp = send(
        &app,
        Request::builder()
            .method(Method::PUT)
            .uri("/api/v1/crates/new")
            .body(Body::from(body))
            .expect("build"),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn invalid_semver_rejected() {
    let (app, _tmp) = setup();
    let manifest = json!({
        "name": "foo",
        "vers": "not-a-version",
        "deps": [],
        "features": {},
        "cksum": "",
    });
    let body = encode_publish_body(&manifest, b"x");
    let resp = send(
        &app,
        Request::builder()
            .method(Method::PUT)
            .uri("/api/v1/crates/new")
            .body(Body::from(body))
            .expect("build"),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn git_index_stub_returns_501() {
    let (app, _tmp) = setup();
    let resp = send(
        &app,
        Request::builder()
            .method(Method::GET)
            .uri("/index.git/info/refs")
            .body(Body::empty())
            .expect("build"),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NOT_IMPLEMENTED);
}

#[tokio::test]
async fn unknown_crate_download_404() {
    let (app, _tmp) = setup();
    let resp = send(
        &app,
        Request::builder()
            .method(Method::GET)
            .uri("/api/v1/crates/foo/1.0.0/download")
            .body(Body::empty())
            .expect("build"),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

/// DD R2 F-R2-021: the sparse-index GET must honour `If-None-Match`.
/// The first GET returns `200 OK` with a strong ETag; the second GET
/// carrying the same ETag returns `304 Not Modified` with no body; a
/// mismatched ETag falls back to `200 OK`.
#[tokio::test]
async fn sparse_index_honours_if_none_match() {
    let (app, _tmp) = setup();
    send(
        &app,
        Request::builder()
            .method(Method::PUT)
            .uri("/api/v1/crates/new")
            .body(Body::from(publish_body("serde", "1.0.0", b"tarball")))
            .expect("build"),
    )
    .await;

    // First fetch picks up the ETag.
    let resp = send(
        &app,
        Request::builder()
            .method(Method::GET)
            .uri("/index/se/rd/serde")
            .body(Body::empty())
            .expect("build"),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let etag = resp
        .headers()
        .get(header::ETAG)
        .and_then(|v| v.to_str().ok())
        .expect("etag header present")
        .to_owned();
    assert!(etag.starts_with('\"') && etag.ends_with('\"'));

    // Second fetch with matching ETag → 304.
    let resp = send(
        &app,
        Request::builder()
            .method(Method::GET)
            .uri("/index/se/rd/serde")
            .header(header::IF_NONE_MATCH, etag.clone())
            .body(Body::empty())
            .expect("build"),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NOT_MODIFIED);
    let body = to_bytes(resp.into_body(), 4096).await.expect("body");
    assert!(body.is_empty(), "304 must have empty body");

    // Third fetch with a mismatched ETag → 200.
    let resp = send(
        &app,
        Request::builder()
            .method(Method::GET)
            .uri("/index/se/rd/serde")
            .header(header::IF_NONE_MATCH, "\"deadbeef\"")
            .body(Body::empty())
            .expect("build"),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = to_bytes(resp.into_body(), 4096).await.expect("body");
    assert!(!body.is_empty(), "200 must carry a body");
}
