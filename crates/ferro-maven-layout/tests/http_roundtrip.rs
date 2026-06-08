// SPDX-License-Identifier: Apache-2.0
//! Axum HTTP round-trip tests for the Maven handler.
//!
//! Uses `tower::ServiceExt` to drive the router in-process. Gated on
//! the `http` feature so `--no-default-features` builds (which omit
//! axum / tokio / async-trait) compile cleanly.

#![cfg(feature = "http")]

use std::sync::Arc;

use axum::body::{Body, to_bytes};
use axum::http::{Method, Request, StatusCode, header};
use ferro_blob_store::{BlobStore, Digest, FsBlobStore};
use ferro_maven_layout::{MAX_ARTIFACT_BODY_BYTES, MavenState, router};
use tempfile::TempDir;
use tower::ServiceExt;

fn setup() -> (axum::Router, TempDir) {
    let (app, tmp, _store) = setup_with_store();
    (app, tmp)
}

/// Like [`setup`] but also returns the backing blob store so a test can
/// assert directly on blob presence (independent of the layout index).
fn setup_with_store() -> (axum::Router, TempDir, Arc<FsBlobStore>) {
    let tmp = TempDir::new().expect("tempdir");
    let store = Arc::new(FsBlobStore::new(tmp.path()).unwrap());
    let state = MavenState::new(store.clone());
    (router(state), tmp, store)
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

async fn head(app: &axum::Router, path: &str) -> axum::http::Response<Body> {
    app.clone()
        .oneshot(
            Request::builder()
                .method(Method::HEAD)
                .uri(path)
                .body(Body::empty())
                .expect("build"),
        )
        .await
        .expect("response")
}

async fn del(app: &axum::Router, path: &str) -> axum::http::Response<Body> {
    app.clone()
        .oneshot(
            Request::builder()
                .method(Method::DELETE)
                .uri(path)
                .body(Body::empty())
                .expect("build"),
        )
        .await
        .expect("response")
}

/// Read the `Content-Type` header of a response as a `&str`.
fn content_type(resp: &axum::http::Response<Body>) -> &str {
    resp.headers()
        .get(header::CONTENT_TYPE)
        .expect("content-type present")
        .to_str()
        .expect("content-type is ascii")
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

/// Each artifact extension maps to a distinct `Content-Type`. This pins
/// every match arm in `build_artifact_response` so deleting any of them
/// is caught by mutation testing.
#[tokio::test]
async fn artifact_content_types_per_extension() {
    let (app, _tmp) = setup();
    let payload: &'static [u8] = b"payload";

    // (path, expected content-type) for one representative of each arm.
    let cases: &[(&str, &str)] = &[
        (
            "/repository/r/com/example/a/1.0/a-1.0.pom",
            "application/xml",
        ),
        (
            "/repository/r/com/example/a/1.0/a-1.0-site.xml",
            "application/xml",
        ),
        (
            "/repository/r/com/example/a/1.0/a-1.0.jar",
            "application/java-archive",
        ),
        (
            "/repository/r/com/example/a/1.0/a-1.0.war",
            "application/java-archive",
        ),
        (
            "/repository/r/com/example/a/1.0/a-1.0.ear",
            "application/java-archive",
        ),
        (
            "/repository/r/com/example/a/1.0/a-1.0-dist.tar.gz",
            "application/gzip",
        ),
        (
            "/repository/r/com/example/a/1.0/a-1.0.tgz",
            "application/gzip",
        ),
        (
            "/repository/r/com/example/a/1.0/a-1.0.zip",
            "application/zip",
        ),
        // Anything else falls through to the catch-all arm.
        (
            "/repository/r/com/example/a/1.0/a-1.0.bin",
            "application/octet-stream",
        ),
    ];

    for (path, expected_ct) in cases {
        // `.pom` and `.xml` go through the POM-validation path only when
        // the extension is literally `pom`; the `.pom` case here uses a
        // matching coordinate-free filename, so store raw bytes for all
        // by putting a non-pom body — except the pom must be valid XML
        // matching the URL coordinate.
        let is_pom = std::path::Path::new(path)
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("pom"));
        let body: &'static [u8] = if is_pom {
            br#"<?xml version="1.0"?><project><modelVersion>4.0.0</modelVersion><groupId>com.example</groupId><artifactId>a</artifactId><version>1.0</version></project>"#
        } else {
            payload
        };
        let put_resp = put(&app, path, body).await;
        assert_eq!(
            put_resp.status(),
            StatusCode::CREATED,
            "PUT failed for {path}"
        );

        let get_resp = get(&app, path).await;
        assert_eq!(get_resp.status(), StatusCode::OK, "GET failed for {path}");
        assert_eq!(
            content_type(&get_resp),
            *expected_ct,
            "content-type mismatch for {path}"
        );
    }
}

/// HEAD of an existing artifact must carry the same headers a GET would
/// (notably `Content-Type` and `Content-Length`) while omitting the
/// body. A `Default::default()` response would have neither header and a
/// 200 status, so asserting the headers catches the `handle_head`
/// replacement mutant.
#[tokio::test]
async fn head_returns_get_headers_without_body() {
    let (app, _tmp) = setup();
    let jar: &'static [u8] = b"probe-bytes";
    put(
        &app,
        "/repository/r/com/example/foo/1.0/foo-1.0.jar",
        jar,
    )
    .await;

    let resp = head(&app, "/repository/r/com/example/foo/1.0/foo-1.0.jar").await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(content_type(&resp), "application/java-archive");
    let len = resp
        .headers()
        .get(header::CONTENT_LENGTH)
        .expect("content-length present")
        .to_str()
        .expect("ascii");
    assert_eq!(len, jar.len().to_string());
    let body = to_bytes(resp.into_body(), 1024).await.expect("body");
    assert!(body.is_empty());
}

/// HEAD of a missing resource must propagate the 404, not a blanket 200.
/// `Default::default()` for the handler would yield a 200, so this kills
/// the `handle_head` replacement mutant from the other direction.
#[tokio::test]
async fn head_missing_is_404() {
    let (app, _tmp) = setup();
    let resp = head(&app, "/repository/r/com/example/gone/1.0/gone-1.0.jar").await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

/// Deleting one of two paths that share the same blob must NOT delete the
/// underlying blob: the surviving path must still GET its content. This
/// exercises the `still_referenced` reference-count guard in
/// `handle_delete`, killing both the `== → !=` and the deleted-`!`
/// mutants (either would wrongly delete the shared blob).
#[tokio::test]
async fn delete_keeps_blob_shared_by_another_path() {
    let (app, _tmp) = setup();
    let shared: &'static [u8] = b"shared-blob-contents";

    let path_a = "/repository/r/com/example/foo/1.0/foo-1.0.jar";
    let path_b = "/repository/r/com/example/bar/1.0/bar-1.0.jar";
    put(&app, path_a, shared).await;
    put(&app, path_b, shared).await;

    // Delete A.
    let del_resp = del(&app, path_a).await;
    assert_eq!(del_resp.status(), StatusCode::NO_CONTENT);

    // A is gone from the layout index.
    assert_eq!(get(&app, path_a).await.status(), StatusCode::NOT_FOUND);

    // B must still resolve to the (still-present) shared blob.
    let get_b = get(&app, path_b).await;
    assert_eq!(get_b.status(), StatusCode::OK, "shared blob was wrongly deleted");
    let got = to_bytes(get_b.into_body(), 1024).await.expect("body");
    assert_eq!(got.as_ref(), shared);
}

/// R5-1 invariant (data-loss race guard): when two Maven paths reference
/// the same content-addressed digest, deleting one path must leave the
/// blob on disk because the surviving path still references it. We assert
/// on the blob store directly (not just via GET) so the invariant is
/// pinned at the storage layer. Then, deleting the LAST reference removes
/// the blob (no orphan leak).
///
/// `handle_delete` removes the layout entry, scans for remaining
/// references, and deletes the blob under a single write guard so a
/// concurrent identical PUT cannot insert a reference into the window
/// between the scan and the delete. This test verifies the *invariant*
/// the critical section protects; the bypass variant below proves the
/// reference check is load-bearing.
#[tokio::test]
async fn delete_preserves_blob_while_other_path_references_digest() {
    let (app, _tmp, store) = setup_with_store();
    let shared: &'static [u8] = b"content-addressed-shared-payload";
    let digest = Digest::sha256_of(shared);

    let path_a = "/repository/r/com/example/foo/1.0/foo-1.0.jar";
    let path_b = "/repository/r/com/example/bar/1.0/bar-1.0.jar";
    put(&app, path_a, shared).await;
    put(&app, path_b, shared).await;

    // Both paths are content-addressed to the same digest, so exactly one
    // blob exists on disk.
    assert!(store.contains(&digest).await.expect("contains"));

    // Delete A: B still references digest D → the blob MUST survive.
    assert_eq!(del(&app, path_a).await.status(), StatusCode::NO_CONTENT);
    assert!(
        store.contains(&digest).await.expect("contains"),
        "blob deleted while still referenced by another path (data loss)"
    );

    // B must still resolve to the surviving blob.
    let get_b = get(&app, path_b).await;
    assert_eq!(get_b.status(), StatusCode::OK, "surviving path no longer resolves");
    let got = to_bytes(get_b.into_body(), 1024).await.expect("body");
    assert_eq!(got.as_ref(), shared);

    // Delete B: this is now the LAST reference → the blob MUST be removed
    // (no orphan leak).
    assert_eq!(del(&app, path_b).await.status(), StatusCode::NO_CONTENT);
    assert!(
        !store.contains(&digest).await.expect("contains"),
        "orphan blob leaked after its last reference was deleted"
    );
}

/// Proves the still-referenced reference check in `handle_delete` is
/// load-bearing: a deliberately reference-blind delete (bypassing the
/// scan) deletes the shared blob even though another path still points at
/// it, which is exactly the data loss the real handler must prevent. This
/// mirrors the invariant of
/// `delete_preserves_blob_while_other_path_references_digest`; if that
/// test passed for a *trivial* reason (e.g. two distinct blobs), this one
/// would also pass, so seeing this fail-mode confirms the assertion is
/// meaningful.
#[tokio::test]
async fn bypassing_reference_check_loses_shared_blob() {
    let (app, _tmp, store) = setup_with_store();
    let shared: &'static [u8] = b"content-addressed-shared-payload";
    let digest = Digest::sha256_of(shared);

    let path_a = "/repository/r/com/example/foo/1.0/foo-1.0.jar";
    let path_b = "/repository/r/com/example/bar/1.0/bar-1.0.jar";
    put(&app, path_a, shared).await;
    put(&app, path_b, shared).await;
    assert!(store.contains(&digest).await.expect("contains"));

    // Simulate a delete that DROPS the reference check (the bug class):
    // unconditionally delete the blob after removing one path. This is the
    // bypass the real handler must NOT do.
    store.delete(&digest).await.expect("delete");

    // The shared blob is now gone even though path B still references it:
    // a GET of the surviving path can no longer be served. This failure is
    // what the reference check prevents, proving the guard is meaningful.
    assert!(
        !store.contains(&digest).await.expect("contains"),
        "bypass test setup invalid: blob should be gone after reference-blind delete"
    );
    let get_b = get(&app, path_b).await;
    assert_ne!(
        get_b.status(),
        StatusCode::OK,
        "surviving path unexpectedly served — bypass did not lose the blob"
    );
}

/// R5-2: a Maven artifact PUT larger than Axum's 2 MiB default body limit
/// must succeed (the router installs `MAX_ARTIFACT_BODY_BYTES`). Without
/// the explicit limit this body would be rejected with 413.
#[tokio::test]
async fn put_artifact_over_default_body_limit_succeeds() {
    let (app, _tmp) = setup();

    // 3 MiB: comfortably above the 2 MiB Axum default, well under the cap.
    let big: Vec<u8> = vec![0x4du8; 3 * 1024 * 1024];
    assert!(big.len() > 2 * 1024 * 1024, "body must exceed the 2 MiB default");
    assert!(big.len() < MAX_ARTIFACT_BODY_BYTES, "body must stay under the cap");

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri("/repository/maven-releases/com/example/big/1.0/big-1.0.jar")
                .body(Body::from(big.clone()))
                .expect("build"),
        )
        .await
        .expect("response");
    assert_eq!(
        resp.status(),
        StatusCode::CREATED,
        "large artifact PUT rejected (likely 413 from the 2 MiB default limit)"
    );

    // And it round-trips back out intact.
    let get_resp = get(&app, "/repository/maven-releases/com/example/big/1.0/big-1.0.jar").await;
    assert_eq!(get_resp.status(), StatusCode::OK);
    let got = to_bytes(get_resp.into_body(), MAX_ARTIFACT_BODY_BYTES)
        .await
        .expect("body");
    assert_eq!(got.len(), big.len());
}
