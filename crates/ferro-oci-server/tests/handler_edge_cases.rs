// SPDX-License-Identifier: Apache-2.0
//! In-process edge-case coverage for the OCI handlers.
//!
//! These tests drive the full [`router`] (plus probe routes) through
//! `tower::oneshot`, exercising the error / pagination / method-not-
//! allowed / empty-descriptor / cancellation paths that the happy-path
//! conformance smoke test does not reach.

#![allow(clippy::too_many_lines)]

use std::sync::Arc;

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Method, Request, StatusCode, header};
use ferro_blob_store::{Digest, InMemoryBlobStore, SharedBlobStore};
use ferro_oci_server::{AppState, InMemoryRegistryMeta, probe_routes, router};
use serde_json::{Value, json};
use tower::ServiceExt;

/// Well-known OCI empty descriptor digest (`{}`).
const EMPTY_DIGEST: &str = "sha256:44136fa355b3678a1146ad16f7e8649e94fb4fc21fe77e8310c060f61caaff8a";

fn app() -> Router {
    let blob_store: SharedBlobStore = Arc::new(InMemoryBlobStore::new());
    let registry = Arc::new(InMemoryRegistryMeta::new());
    let state = AppState::new(blob_store, registry);
    router(state).merge(probe_routes())
}

async fn send(app: &Router, req: Request<Body>) -> (StatusCode, axum::http::HeaderMap, Vec<u8>) {
    let resp = app.clone().oneshot(req).await.expect("response");
    let status = resp.status();
    let headers = resp.headers().clone();
    let body = to_bytes(resp.into_body(), 1 << 20).await.expect("body");
    (status, headers, body.to_vec())
}

fn get(uri: &str) -> Request<Body> {
    Request::builder()
        .method(Method::GET)
        .uri(uri)
        .body(Body::empty())
        .expect("req")
}

fn method(m: Method, uri: &str, body: Body) -> Request<Body> {
    Request::builder()
        .method(m)
        .uri(uri)
        .body(body)
        .expect("req")
}

fn assert_error_code(body: &[u8], expected: &str) {
    let v: Value = serde_json::from_slice(body).expect("error json");
    assert_eq!(v["errors"][0]["code"], expected, "body={v}");
}

// ---- blob.rs ------------------------------------------------------------

#[tokio::test]
async fn blob_get_missing_returns_404_blob_unknown() {
    let app = app();
    let digest = Digest::sha256_of(b"absent").to_string();
    let (status, _h, body) = send(&app, get(&format!("/v2/repo/blobs/{digest}"))).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_error_code(&body, "BLOB_UNKNOWN");
}

#[tokio::test]
async fn blob_get_invalid_digest_returns_400() {
    let app = app();
    let (status, _h, body) = send(&app, get("/v2/repo/blobs/not-a-digest")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_error_code(&body, "DIGEST_INVALID");
}

#[tokio::test]
async fn blob_get_empty_descriptor_served_synthetically() {
    let app = app();
    let (status, headers, body) = send(&app, get(&format!("/v2/repo/blobs/{EMPTY_DIGEST}"))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, b"{}");
    assert_eq!(headers[header::CONTENT_LENGTH], "2");
    assert!(headers.contains_key("docker-content-digest"));
}

#[tokio::test]
async fn blob_head_empty_descriptor_ok_no_body() {
    let app = app();
    let (status, headers, body) = send(
        &app,
        method(Method::HEAD, &format!("/v2/repo/blobs/{EMPTY_DIGEST}"), Body::empty()),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.is_empty());
    assert_eq!(headers[header::CONTENT_LENGTH], "2");
}

#[tokio::test]
async fn blob_head_missing_returns_404() {
    let app = app();
    let digest = Digest::sha256_of(b"absent-head").to_string();
    let (status, _h, _b) = send(
        &app,
        method(Method::HEAD, &format!("/v2/repo/blobs/{digest}"), Body::empty()),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn blob_delete_existing_then_missing() {
    let app = app();
    // Upload a blob monolithically so it exists.
    let payload = b"deletable-bytes";
    let digest = Digest::sha256_of(payload).to_string();
    let (status, _h, _b) = send(
        &app,
        method(
            Method::POST,
            &format!("/v2/repo/blobs/uploads/?digest={digest}"),
            Body::from(&payload[..]),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    // Delete it → 202.
    let (status, _h, _b) = send(
        &app,
        method(Method::DELETE, &format!("/v2/repo/blobs/{digest}"), Body::empty()),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);

    // Deleting again → 404 BLOB_UNKNOWN.
    let (status, _h, body) = send(
        &app,
        method(Method::DELETE, &format!("/v2/repo/blobs/{digest}"), Body::empty()),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_error_code(&body, "BLOB_UNKNOWN");
}

#[tokio::test]
async fn blob_get_invalid_name_returns_400() {
    let app = app();
    // Upper-case repository name is invalid per OCI name grammar.
    let (status, _h, body) = send(&app, get(&format!("/v2/Bad_NAME/blobs/{EMPTY_DIGEST}"))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_error_code(&body, "NAME_INVALID");
}

// ---- blob_upload.rs -----------------------------------------------------

#[tokio::test]
async fn monolithic_upload_digest_mismatch_returns_400() {
    let app = app();
    let wrong = Digest::sha256_of(b"something-else").to_string();
    let (status, _h, body) = send(
        &app,
        method(
            Method::POST,
            &format!("/v2/repo/blobs/uploads/?digest={wrong}"),
            Body::from(&b"actual-bytes"[..]),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_error_code(&body, "DIGEST_INVALID");
}

#[tokio::test]
async fn monolithic_upload_bad_digest_string_returns_400() {
    let app = app();
    let (status, _h, body) = send(
        &app,
        method(
            Method::POST,
            "/v2/repo/blobs/uploads/?digest=not-a-digest",
            Body::from(&b"x"[..]),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_error_code(&body, "DIGEST_INVALID");
}

#[tokio::test]
async fn start_upload_then_get_status_then_cancel() {
    let app = app();
    // Start a session.
    let (status, headers, _b) =
        send(&app, method(Method::POST, "/v2/repo/blobs/uploads/", Body::empty())).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let location = headers[header::LOCATION].to_str().unwrap().to_owned();
    assert!(location.contains("/blobs/uploads/"));

    // GET upload status → 204 with Range header.
    let (status, headers, _b) = send(&app, get(&location)).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(headers.contains_key(header::RANGE));

    // Cancel → 204.
    let (status, _h, _b) =
        send(&app, method(Method::DELETE, &location, Body::empty())).await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // Cancel again → unknown upload (404).
    let (status, _h, body) =
        send(&app, method(Method::DELETE, &location, Body::empty())).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_error_code(&body, "BLOB_UPLOAD_UNKNOWN");
}

#[tokio::test]
async fn get_status_for_unknown_upload_returns_404() {
    let app = app();
    let (status, _h, body) = send(&app, get("/v2/repo/blobs/uploads/does-not-exist")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_error_code(&body, "BLOB_UPLOAD_UNKNOWN");
}

#[tokio::test]
async fn patch_unknown_upload_returns_404() {
    let app = app();
    let (status, _h, body) = send(
        &app,
        method(
            Method::PATCH,
            "/v2/repo/blobs/uploads/ghost",
            Body::from(&b"chunk"[..]),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_error_code(&body, "BLOB_UPLOAD_UNKNOWN");
}

#[tokio::test]
async fn patch_out_of_order_content_range_returns_416() {
    let app = app();
    let (_s, headers, _b) =
        send(&app, method(Method::POST, "/v2/repo/blobs/uploads/", Body::empty())).await;
    let location = headers[header::LOCATION].to_str().unwrap().to_owned();
    // Declare a chunk that starts at 50 even though offset is 0.
    let req = Request::builder()
        .method(Method::PATCH)
        .uri(&location)
        .header(header::CONTENT_RANGE, "50-99")
        .body(Body::from(&b"data"[..]))
        .expect("req");
    let (status, _h, body) = send(&app, req).await;
    assert_eq!(status, StatusCode::RANGE_NOT_SATISFIABLE);
    assert_error_code(&body, "BLOB_UPLOAD_INVALID");
}

#[tokio::test]
async fn patch_malformed_content_range_returns_400() {
    let app = app();
    let (_s, headers, _b) =
        send(&app, method(Method::POST, "/v2/repo/blobs/uploads/", Body::empty())).await;
    let location = headers[header::LOCATION].to_str().unwrap().to_owned();
    let req = Request::builder()
        .method(Method::PATCH)
        .uri(&location)
        .header(header::CONTENT_RANGE, "garbage")
        .body(Body::from(&b"data"[..]))
        .expect("req");
    let (status, _h, body) = send(&app, req).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_error_code(&body, "BLOB_UPLOAD_INVALID");
}

#[tokio::test]
async fn finish_upload_missing_digest_param_returns_400() {
    let app = app();
    let (_s, headers, _b) =
        send(&app, method(Method::POST, "/v2/repo/blobs/uploads/", Body::empty())).await;
    let location = headers[header::LOCATION].to_str().unwrap().to_owned();
    // PUT with no ?digest=.
    let (status, _h, body) =
        send(&app, method(Method::PUT, &location, Body::from(&b"final"[..]))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_error_code(&body, "DIGEST_INVALID");
}

#[tokio::test]
async fn finish_unknown_upload_returns_404() {
    let app = app();
    let digest = Digest::sha256_of(b"whatever").to_string();
    let (status, _h, body) = send(
        &app,
        method(
            Method::PUT,
            &format!("/v2/repo/blobs/uploads/ghost?digest={digest}"),
            Body::empty(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_error_code(&body, "BLOB_UPLOAD_UNKNOWN");
}

#[tokio::test]
async fn upload_invalid_name_returns_400() {
    let app = app();
    let (status, _h, body) =
        send(&app, method(Method::POST, "/v2/BAD/blobs/uploads/", Body::empty())).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_error_code(&body, "NAME_INVALID");
}

// ---- catalog.rs + tags.rs ----------------------------------------------

#[tokio::test]
async fn catalog_empty_is_ok_with_no_repositories() {
    let app = app();
    let (status, _h, body) = send(&app, get("/v2/_catalog")).await;
    assert_eq!(status, StatusCode::OK);
    let v: Value = serde_json::from_slice(&body).expect("json");
    assert_eq!(v["repositories"].as_array().expect("arr").len(), 0);
}

#[tokio::test]
async fn catalog_pagination_emits_link_header() {
    let app = app();
    // Push three tiny manifests in distinct repos so the catalog has
    // entries; paginate with n=1 to force the Link header.
    for repo in ["a-repo", "b-repo", "c-repo"] {
        push_min_manifest(&app, repo, "latest").await;
    }
    let (status, headers, body) = send(&app, get("/v2/_catalog?n=1")).await;
    assert_eq!(status, StatusCode::OK);
    let v: Value = serde_json::from_slice(&body).expect("json");
    assert_eq!(v["repositories"].as_array().unwrap().len(), 1);
    assert!(
        headers.contains_key(header::LINK),
        "n=1 over 3 repos must emit a next Link"
    );
}

#[tokio::test]
async fn tags_list_pagination_emits_link_header() {
    let app = app();
    for tag in ["v1", "v2", "v3"] {
        push_min_manifest(&app, "tagrepo", tag).await;
    }
    let (status, headers, body) = send(&app, get("/v2/tagrepo/tags/list?n=1")).await;
    assert_eq!(status, StatusCode::OK);
    let v: Value = serde_json::from_slice(&body).expect("json");
    assert_eq!(v["name"], "tagrepo");
    assert_eq!(v["tags"].as_array().unwrap().len(), 1);
    assert!(headers.contains_key(header::LINK));
}

#[tokio::test]
async fn tags_list_invalid_name_returns_400() {
    let app = app();
    let (status, _h, body) = send(&app, get("/v2/BAD/tags/list")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_error_code(&body, "NAME_INVALID");
}

// ---- manifest.rs --------------------------------------------------------

/// Push a minimal image manifest referencing only the empty descriptor.
async fn push_min_manifest(app: &Router, name: &str, reference: &str) -> Digest {
    let manifest = json!({
        "schemaVersion": 2,
        "mediaType": "application/vnd.oci.image.manifest.v1+json",
        "config": {
            "mediaType": "application/vnd.oci.image.config.v1+json",
            "digest": EMPTY_DIGEST,
            "size": 2
        },
        "layers": []
    });
    let body = serde_json::to_vec(&manifest).expect("ser");
    let digest = Digest::sha256_of(&body);
    let req = Request::builder()
        .method(Method::PUT)
        .uri(format!("/v2/{name}/manifests/{reference}"))
        .header(
            header::CONTENT_TYPE,
            "application/vnd.oci.image.manifest.v1+json",
        )
        .body(Body::from(body))
        .expect("req");
    let (status, _h, _b) = send(app, req).await;
    assert_eq!(status, StatusCode::CREATED, "manifest push for {name}:{reference}");
    digest
}

#[tokio::test]
async fn manifest_put_missing_content_type_returns_400() {
    let app = app();
    let req = Request::builder()
        .method(Method::PUT)
        .uri("/v2/repo/manifests/latest")
        .body(Body::from(&b"{}"[..]))
        .expect("req");
    let (status, _h, body) = send(&app, req).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_error_code(&body, "MANIFEST_INVALID");
}

#[tokio::test]
async fn manifest_put_unsupported_media_type_returns_400() {
    let app = app();
    let req = Request::builder()
        .method(Method::PUT)
        .uri("/v2/repo/manifests/latest")
        .header(header::CONTENT_TYPE, "text/plain")
        .body(Body::from(&b"{}"[..]))
        .expect("req");
    let (status, _h, body) = send(&app, req).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_error_code(&body, "MANIFEST_INVALID");
}

#[tokio::test]
async fn manifest_put_invalid_json_returns_400() {
    let app = app();
    let req = Request::builder()
        .method(Method::PUT)
        .uri("/v2/repo/manifests/latest")
        .header(
            header::CONTENT_TYPE,
            "application/vnd.oci.image.manifest.v1+json",
        )
        .body(Body::from(&b"{not json"[..]))
        .expect("req");
    let (status, _h, body) = send(&app, req).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_error_code(&body, "MANIFEST_INVALID");
}

#[tokio::test]
async fn manifest_put_missing_referenced_layer_blob_returns_404() {
    let app = app();
    // Manifest references a layer blob that was never uploaded.
    let missing = Digest::sha256_of(b"never-uploaded-layer").to_string();
    let manifest = json!({
        "schemaVersion": 2,
        "mediaType": "application/vnd.oci.image.manifest.v1+json",
        "config": { "mediaType": "application/vnd.oci.image.config.v1+json", "digest": EMPTY_DIGEST, "size": 2 },
        "layers": [ { "mediaType": "application/vnd.oci.image.layer.v1.tar", "digest": missing, "size": 10 } ]
    });
    let req = Request::builder()
        .method(Method::PUT)
        .uri("/v2/repo/manifests/latest")
        .header(
            header::CONTENT_TYPE,
            "application/vnd.oci.image.manifest.v1+json",
        )
        .body(Body::from(serde_json::to_vec(&manifest).unwrap()))
        .expect("req");
    let (status, _h, body) = send(&app, req).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_error_code(&body, "MANIFEST_BLOB_UNKNOWN");
}

#[tokio::test]
async fn image_index_missing_child_returns_404() {
    let app = app();
    let missing = Digest::sha256_of(b"absent-child-manifest").to_string();
    let index = json!({
        "schemaVersion": 2,
        "mediaType": "application/vnd.oci.image.index.v1+json",
        "manifests": [ { "mediaType": "application/vnd.oci.image.manifest.v1+json", "digest": missing, "size": 5 } ]
    });
    let req = Request::builder()
        .method(Method::PUT)
        .uri("/v2/repo/manifests/idx")
        .header(
            header::CONTENT_TYPE,
            "application/vnd.oci.image.index.v1+json",
        )
        .body(Body::from(serde_json::to_vec(&index).unwrap()))
        .expect("req");
    let (status, _h, body) = send(&app, req).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_error_code(&body, "MANIFEST_BLOB_UNKNOWN");
}

#[tokio::test]
async fn manifest_delete_by_tag_returns_405() {
    let app = app();
    push_min_manifest(&app, "repo", "latest").await;
    let (status, _h, body) = send(
        &app,
        method(Method::DELETE, "/v2/repo/manifests/latest", Body::empty()),
    )
    .await;
    assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);
    assert_error_code(&body, "UNSUPPORTED");
}

#[tokio::test]
async fn manifest_delete_by_digest_then_missing() {
    let app = app();
    let digest = push_min_manifest(&app, "repo", "latest").await;
    let digest_str = digest.to_string();
    // Delete by digest → 202.
    let (status, _h, _b) = send(
        &app,
        method(Method::DELETE, &format!("/v2/repo/manifests/{digest_str}"), Body::empty()),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    // Delete again by digest → 404.
    let (status, _h, body) = send(
        &app,
        method(Method::DELETE, &format!("/v2/repo/manifests/{digest_str}"), Body::empty()),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_error_code(&body, "MANIFEST_UNKNOWN");
}

#[tokio::test]
async fn manifest_delete_invalid_reference_returns_400() {
    let app = app();
    // `sha256:` prefix with non-hex content parses as a digest reference
    // but fails digest validation → 400 DIGEST_INVALID (a URI-legal but
    // semantically invalid reference, unlike a raw space).
    let (status, _h, _b) = send(
        &app,
        method(
            Method::DELETE,
            "/v2/repo/manifests/sha256:ZZZZ",
            Body::empty(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn manifest_get_unparseable_range_serves_full_body_200() {
    let app = app();
    push_min_manifest(&app, "repo", "latest").await;
    let req = Request::builder()
        .method(Method::GET)
        .uri("/v2/repo/manifests/latest")
        .header(header::RANGE, "items=0-5")
        .body(Body::empty())
        .expect("req");
    let (status, headers, body) = send(&app, req).await;
    assert_eq!(status, StatusCode::OK);
    assert!(!body.is_empty());
    assert_eq!(headers[header::ACCEPT_RANGES], "bytes");
}

#[tokio::test]
async fn manifest_get_inverted_range_returns_416() {
    let app = app();
    push_min_manifest(&app, "repo", "latest").await;
    let req = Request::builder()
        .method(Method::GET)
        .uri("/v2/repo/manifests/latest")
        .header(header::RANGE, "bytes=80-10")
        .body(Body::empty())
        .expect("req");
    let (status, _h, _b) = send(&app, req).await;
    assert_eq!(status, StatusCode::RANGE_NOT_SATISFIABLE);
}

// ---- router.rs dispatch (method-not-allowed + unroutable) ---------------

#[tokio::test]
async fn tags_list_with_post_is_405() {
    let app = app();
    let (status, _h, body) = send(
        &app,
        method(Method::POST, "/v2/repo/tags/list", Body::empty()),
    )
    .await;
    assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);
    assert_error_code(&body, "UNSUPPORTED");
}

#[tokio::test]
async fn unroutable_v2_suffix_returns_name_unknown() {
    let app = app();
    // A path that has no recognised keyword suffix.
    let (status, _h, body) = send(&app, get("/v2/repo/bogus/segment")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_error_code(&body, "NAME_UNKNOWN");
}

#[tokio::test]
async fn referrers_with_post_is_405() {
    let app = app();
    let (status, _h, _b) = send(
        &app,
        method(
            Method::POST,
            &format!("/v2/repo/referrers/{EMPTY_DIGEST}"),
            Body::empty(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);
}

#[tokio::test]
async fn probe_routes_live_ready_healthz() {
    let app = app();
    for uri in ["/live", "/ready", "/healthz"] {
        let (status, _h, _b) = send(&app, get(uri)).await;
        assert_eq!(status, StatusCode::OK, "GET {uri}");
    }
}
