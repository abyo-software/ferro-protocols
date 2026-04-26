// SPDX-License-Identifier: Apache-2.0
//! Axum HTTP round-trip tests for the Maven handler.
//!
//! Uses `tower::ServiceExt` to drive the router in-process.

use std::sync::Arc;

use axum::body::{Body, to_bytes};
use axum::http::{Method, Request, StatusCode};
use ferro_blob_store::FsBlobStore;
use ferro_maven_layout::{MavenState, router};
use tempfile::TempDir;
use tower::ServiceExt;

fn setup() -> (axum::Router, TempDir) {
    let tmp = TempDir::new().expect("tempdir");
    let store = Arc::new(FsBlobStore::new(tmp.path()).unwrap());
    let state = MavenState::new(store);
    (router(state), tmp)
}

const POM_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<project xmlns="http://maven.apache.org/POM/4.0.0">
    <modelVersion>4.0.0</modelVersion>
    <groupId>com.example</groupId>
    <artifactId>foo</artifactId>
    <version>1.0</version>
    <packaging>jar</packaging>
</project>"#;

async fn put(app: &axum::Router, path: &str, body: &'static [u8]) -> axum::http::Response<Body> {
    app.clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri(path)
                .body(Body::from(body))
                .expect("build"),
        )
        .await
        .expect("response")
}

async fn get(app: &axum::Router, path: &str) -> axum::http::Response<Body> {
    app.clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(path)
                .body(Body::empty())
                .expect("build"),
        )
        .await
        .expect("response")
}

#[tokio::test]
async fn put_jar_then_get_jar_round_trip() {
    let (app, _tmp) = setup();

    let jar_bytes: &'static [u8] = b"fake-jar-contents";
    let put_resp = put(
        &app,
        "/repository/maven-releases/com/example/foo/1.0/foo-1.0.jar",
        jar_bytes,
    )
    .await;
    assert_eq!(put_resp.status(), StatusCode::CREATED);

    let get_resp = get(
        &app,
        "/repository/maven-releases/com/example/foo/1.0/foo-1.0.jar",
    )
    .await;
    assert_eq!(get_resp.status(), StatusCode::OK);
    let got = to_bytes(get_resp.into_body(), 1024).await.expect("body");
    assert_eq!(got.as_ref(), jar_bytes);
}

#[tokio::test]
async fn get_missing_is_404() {
    let (app, _tmp) = setup();
    let resp = get(
        &app,
        "/repository/maven-releases/com/example/nope/1.0/nope-1.0.jar",
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn put_pom_mismatched_coordinate_rejected() {
    let (app, _tmp) = setup();
    // POM coords say foo:1.0, URL says bar:1.0 → must 400.
    let resp = put(
        &app,
        "/repository/maven-releases/com/example/bar/1.0/bar-1.0.pom",
        POM_XML.as_bytes(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn put_pom_matching_coordinate_accepted() {
    let (app, _tmp) = setup();
    let resp = put(
        &app,
        "/repository/maven-releases/com/example/foo/1.0/foo-1.0.pom",
        POM_XML.as_bytes(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CREATED);
}

#[tokio::test]
async fn get_sha1_sidecar_computed_on_the_fly() {
    let (app, _tmp) = setup();
    let jar_bytes: &'static [u8] = b"abc";
    put(
        &app,
        "/repository/maven-releases/com/example/foo/1.0/foo-1.0.jar",
        jar_bytes,
    )
    .await;

    let resp = get(
        &app,
        "/repository/maven-releases/com/example/foo/1.0/foo-1.0.jar.sha1",
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let got = to_bytes(resp.into_body(), 1024).await.expect("body");
    assert_eq!(got.as_ref(), b"a9993e364706816aba3e25717850c26c9cd0d89d");
}

#[tokio::test]
async fn put_checksum_sidecar_matching_accepted() {
    let (app, _tmp) = setup();
    let jar_bytes: &'static [u8] = b"abc";
    put(
        &app,
        "/repository/maven-releases/com/example/foo/1.0/foo-1.0.jar",
        jar_bytes,
    )
    .await;

    // SHA-1 of "abc" = a9993e364706816aba3e25717850c26c9cd0d89d
    let resp = put(
        &app,
        "/repository/maven-releases/com/example/foo/1.0/foo-1.0.jar.sha1",
        b"a9993e364706816aba3e25717850c26c9cd0d89d\n",
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CREATED);
}

#[tokio::test]
async fn put_checksum_sidecar_mismatch_rejected() {
    let (app, _tmp) = setup();
    let jar_bytes: &'static [u8] = b"abc";
    put(
        &app,
        "/repository/maven-releases/com/example/foo/1.0/foo-1.0.jar",
        jar_bytes,
    )
    .await;

    let resp = put(
        &app,
        "/repository/maven-releases/com/example/foo/1.0/foo-1.0.jar.sha1",
        b"0000000000000000000000000000000000000000\n",
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn get_generated_maven_metadata_xml() {
    let (app, _tmp) = setup();
    let jar: &'static [u8] = b"jarbytes";
    put(
        &app,
        "/repository/maven-releases/com/example/foo/1.0/foo-1.0.jar",
        jar,
    )
    .await;
    put(
        &app,
        "/repository/maven-releases/com/example/foo/1.1/foo-1.1.jar",
        jar,
    )
    .await;

    let resp = get(
        &app,
        "/repository/maven-releases/com/example/foo/maven-metadata.xml",
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = to_bytes(resp.into_body(), 1024 * 1024).await.expect("body");
    let xml = std::str::from_utf8(&body).expect("utf8");
    assert!(xml.contains("<artifactId>foo</artifactId>"));
    assert!(xml.contains("<version>1.0</version>"));
    assert!(xml.contains("<version>1.1</version>"));
    assert!(xml.contains("<release>1.1</release>"));
}

#[tokio::test]
async fn snapshot_put_generates_version_metadata() {
    let (app, _tmp) = setup();
    let jar: &'static [u8] = b"snap-jar";
    put(
        &app,
        "/repository/maven-snapshots/com/example/foo/1.0-SNAPSHOT/foo-1.0-SNAPSHOT.jar",
        jar,
    )
    .await;

    let resp = get(
        &app,
        "/repository/maven-snapshots/com/example/foo/1.0-SNAPSHOT/maven-metadata.xml",
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = to_bytes(resp.into_body(), 1024 * 1024).await.expect("body");
    let xml = std::str::from_utf8(&body).expect("utf8");
    assert!(xml.contains("<snapshot>"));
    assert!(xml.contains("<buildNumber>1</buildNumber>"));
}

#[tokio::test]
async fn delete_removes_artifact() {
    let (app, _tmp) = setup();
    let jar: &'static [u8] = b"deletable";
    put(
        &app,
        "/repository/maven-releases/com/example/foo/1.0/foo-1.0.jar",
        jar,
    )
    .await;

    let del = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::DELETE)
                .uri("/repository/maven-releases/com/example/foo/1.0/foo-1.0.jar")
                .body(Body::empty())
                .expect("build"),
        )
        .await
        .expect("resp");
    assert_eq!(del.status(), StatusCode::NO_CONTENT);

    let resp = get(
        &app,
        "/repository/maven-releases/com/example/foo/1.0/foo-1.0.jar",
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn head_returns_no_body() {
    let (app, _tmp) = setup();
    let jar: &'static [u8] = b"probe";
    put(
        &app,
        "/repository/maven-releases/com/example/foo/1.0/foo-1.0.jar",
        jar,
    )
    .await;

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::HEAD)
                .uri("/repository/maven-releases/com/example/foo/1.0/foo-1.0.jar")
                .body(Body::empty())
                .expect("build"),
        )
        .await
        .expect("resp");
    assert_eq!(resp.status(), StatusCode::OK);
    let body = to_bytes(resp.into_body(), 1024).await.expect("body");
    assert!(body.is_empty());
}
