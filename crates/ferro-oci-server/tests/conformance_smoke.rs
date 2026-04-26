// SPDX-License-Identifier: Apache-2.0
//! End-to-end smoke tests for the OCI Distribution Spec v1.1 handlers.
//!
//! These tests exercise the full router shape as the conformance suite
//! would: start-upload, chunked PATCH, finalize PUT, blob GET/HEAD,
//! manifest PUT/GET by tag and digest, tag listing, catalog listing,
//! manifest DELETE, referrers API, and the standard error cases.

// Tests are full request-response walks — splitting them into helper
// functions would hide the spec narrative, so we accept longer bodies.
#![allow(clippy::too_many_lines)]

use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode, header};
use bytes::Bytes;
use ferro_blob_store::Digest;
use ferro_blob_store::{FsBlobStore, SharedBlobStore};
use ferro_oci_server::{AppState, InMemoryRegistryMeta, router};
use http_body_util::BodyExt;
use tempfile::TempDir;
use tower::ServiceExt;

fn make_app() -> (Router, TempDir) {
    let tmp = TempDir::new().expect("tempdir");
    let blob_store: SharedBlobStore = Arc::new(FsBlobStore::new(tmp.path()).expect("blob store"));
    let registry = Arc::new(InMemoryRegistryMeta::new());
    let state = Arc::new(AppState {
        blob_store,
        registry,
    });
    (router(state), tmp)
}

async fn send(app: &Router, req: Request<Body>) -> (StatusCode, HeaderMap, Bytes) {
    let resp = app.clone().oneshot(req).await.expect("oneshot response");
    let status = resp.status();
    let headers = resp.headers().clone();
    let body = resp
        .into_body()
        .collect()
        .await
        .expect("collect body")
        .to_bytes();
    (status, headers, body)
}

#[tokio::test]
async fn base_endpoint_returns_v2_version_header() {
    let (app, _tmp) = make_app();
    let (status, headers, body) = send(
        &app,
        Request::builder()
            .method("GET")
            .uri("/v2/")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        headers
            .get("Docker-Distribution-API-Version")
            .expect("version header")
            .to_str()
            .unwrap(),
        "registry/2.0"
    );
    // Body is the empty JSON object.
    assert_eq!(&body[..], b"{}");
}

#[tokio::test]
async fn monolithic_blob_upload_and_get_roundtrip() {
    let (app, _tmp) = make_app();
    let payload = Bytes::from_static(b"hello monolithic");
    let digest = Digest::sha256_of(&payload);
    let name = "lib/alpine";

    // POST monolithic upload.
    let req = Request::builder()
        .method("POST")
        .uri(format!("/v2/{name}/blobs/uploads/?digest={digest}"))
        .body(Body::from(payload.clone()))
        .unwrap();
    let (status, headers, _body) = send(&app, req).await;
    assert_eq!(status, StatusCode::CREATED, "monolithic upload");
    assert_eq!(
        headers
            .get("Docker-Content-Digest")
            .unwrap()
            .to_str()
            .unwrap(),
        digest.to_string()
    );
    let location = headers.get(header::LOCATION).unwrap().to_str().unwrap();
    assert_eq!(location, format!("/v2/{name}/blobs/{digest}"));

    // GET the blob back.
    let req = Request::builder()
        .method("GET")
        .uri(format!("/v2/{name}/blobs/{digest}"))
        .body(Body::empty())
        .unwrap();
    let (status, headers, body) = send(&app, req).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(&body[..], &payload[..]);
    assert_eq!(
        headers
            .get("Docker-Content-Digest")
            .unwrap()
            .to_str()
            .unwrap(),
        digest.to_string()
    );
    assert_eq!(
        headers
            .get(header::CONTENT_LENGTH)
            .unwrap()
            .to_str()
            .unwrap(),
        payload.len().to_string()
    );

    // HEAD returns the same headers.
    let req = Request::builder()
        .method("HEAD")
        .uri(format!("/v2/{name}/blobs/{digest}"))
        .body(Body::empty())
        .unwrap();
    let (status, headers, body) = send(&app, req).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.is_empty(), "HEAD must have empty body");
    assert_eq!(
        headers
            .get("Docker-Content-Digest")
            .unwrap()
            .to_str()
            .unwrap(),
        digest.to_string()
    );
}

#[tokio::test]
async fn chunked_blob_upload_three_chunks_then_finalize() {
    let (app, _tmp) = make_app();
    let name = "lib/chunky";

    let chunk1 = Bytes::from_static(b"first-");
    let chunk2 = Bytes::from_static(b"second-");
    let chunk3 = Bytes::from_static(b"third");
    let full: Vec<u8> = [&chunk1[..], &chunk2[..], &chunk3[..]].concat();
    let digest = Digest::sha256_of(&full);

    // Start upload.
    let (status, headers, _body) = send(
        &app,
        Request::builder()
            .method("POST")
            .uri(format!("/v2/{name}/blobs/uploads/"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let location = headers
        .get(header::LOCATION)
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    assert!(location.starts_with(&format!("/v2/{name}/blobs/uploads/")));
    let uuid = location.trim_start_matches(&format!("/v2/{name}/blobs/uploads/"));
    assert!(!uuid.is_empty(), "uuid allocated");
    assert_eq!(headers.get(header::RANGE).unwrap().to_str().unwrap(), "0-0");
    assert_eq!(
        headers
            .get("OCI-Chunk-Min-Length")
            .unwrap()
            .to_str()
            .unwrap(),
        "0"
    );
    assert_eq!(
        headers.get("Docker-Upload-UUID").unwrap().to_str().unwrap(),
        uuid
    );

    // PATCH chunk 1.
    let (status, headers, _body) = send(
        &app,
        Request::builder()
            .method("PATCH")
            .uri(location.clone())
            .header(header::CONTENT_RANGE, "0-5")
            .body(Body::from(chunk1.clone()))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(headers.get(header::RANGE).unwrap().to_str().unwrap(), "0-5");

    // PATCH chunk 2.
    let (status, _headers, _body) = send(
        &app,
        Request::builder()
            .method("PATCH")
            .uri(location.clone())
            .header(
                header::CONTENT_RANGE,
                format!("{}-{}", chunk1.len(), chunk1.len() + chunk2.len() - 1),
            )
            .body(Body::from(chunk2.clone()))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);

    // PATCH chunk 3.
    let (status, _headers, _body) = send(
        &app,
        Request::builder()
            .method("PATCH")
            .uri(location.clone())
            .header(
                header::CONTENT_RANGE,
                format!(
                    "{}-{}",
                    chunk1.len() + chunk2.len(),
                    chunk1.len() + chunk2.len() + chunk3.len() - 1
                ),
            )
            .body(Body::from(chunk3.clone()))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);

    // Finalize.
    let (status, headers, _body) = send(
        &app,
        Request::builder()
            .method("PUT")
            .uri(format!("{location}?digest={digest}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "finalize");
    assert_eq!(
        headers.get(header::LOCATION).unwrap().to_str().unwrap(),
        format!("/v2/{name}/blobs/{digest}")
    );
    assert_eq!(
        headers
            .get("Docker-Content-Digest")
            .unwrap()
            .to_str()
            .unwrap(),
        digest.to_string()
    );

    // GET reassembled blob.
    let (status, _headers, body) = send(
        &app,
        Request::builder()
            .method("GET")
            .uri(format!("/v2/{name}/blobs/{digest}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(&body[..], &full[..]);
}

#[tokio::test]
async fn manifest_put_and_get_by_tag_and_digest_and_tag_listing() {
    let (app, _tmp) = make_app();
    let name = "lib/app";

    // Upload a config blob and a layer blob monolithically.
    let config = Bytes::from_static(b"{\"architecture\":\"amd64\",\"os\":\"linux\"}");
    let config_digest = Digest::sha256_of(&config);
    let (status, _h, _b) = send(
        &app,
        Request::builder()
            .method("POST")
            .uri(format!("/v2/{name}/blobs/uploads/?digest={config_digest}"))
            .body(Body::from(config.clone()))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    let layer = Bytes::from_static(b"layer-bytes-go-here");
    let layer_digest = Digest::sha256_of(&layer);
    let (status, _h, _b) = send(
        &app,
        Request::builder()
            .method("POST")
            .uri(format!("/v2/{name}/blobs/uploads/?digest={layer_digest}"))
            .body(Body::from(layer.clone()))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    // Build a manifest referring to both.
    let manifest_body = serde_json::json!({
        "schemaVersion": 2,
        "mediaType": "application/vnd.oci.image.manifest.v1+json",
        "config": {
            "mediaType": "application/vnd.oci.image.config.v1+json",
            "digest": config_digest.to_string(),
            "size": config.len()
        },
        "layers": [
            {
                "mediaType": "application/vnd.oci.image.layer.v1.tar+gzip",
                "digest": layer_digest.to_string(),
                "size": layer.len()
            }
        ]
    });
    let manifest_bytes = Bytes::from(serde_json::to_vec(&manifest_body).unwrap());
    let manifest_digest = Digest::sha256_of(&manifest_bytes);

    // PUT manifest by tag.
    let (status, headers, _body) = send(
        &app,
        Request::builder()
            .method("PUT")
            .uri(format!("/v2/{name}/manifests/v1"))
            .header(
                header::CONTENT_TYPE,
                "application/vnd.oci.image.manifest.v1+json",
            )
            .body(Body::from(manifest_bytes.clone()))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "manifest PUT");
    assert_eq!(
        headers
            .get("Docker-Content-Digest")
            .unwrap()
            .to_str()
            .unwrap(),
        manifest_digest.to_string()
    );

    // GET by tag.
    let (status, headers, body) = send(
        &app,
        Request::builder()
            .method("GET")
            .uri(format!("/v2/{name}/manifests/v1"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "manifest GET by tag");
    assert_eq!(
        headers.get(header::CONTENT_TYPE).unwrap().to_str().unwrap(),
        "application/vnd.oci.image.manifest.v1+json"
    );
    assert_eq!(&body[..], &manifest_bytes[..]);

    // GET by digest.
    let (status, _headers, body) = send(
        &app,
        Request::builder()
            .method("GET")
            .uri(format!("/v2/{name}/manifests/{manifest_digest}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "manifest GET by digest");
    assert_eq!(&body[..], &manifest_bytes[..]);

    // HEAD by tag.
    let (status, headers, _body) = send(
        &app,
        Request::builder()
            .method("HEAD")
            .uri(format!("/v2/{name}/manifests/v1"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "manifest HEAD by tag");
    assert_eq!(
        headers
            .get("Docker-Content-Digest")
            .unwrap()
            .to_str()
            .unwrap(),
        manifest_digest.to_string()
    );

    // Tag list.
    let (status, _headers, body) = send(
        &app,
        Request::builder()
            .method("GET")
            .uri(format!("/v2/{name}/tags/list"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["name"], name);
    assert_eq!(json["tags"], serde_json::json!(["v1"]));

    // Catalog.
    let (status, _headers, body) = send(
        &app,
        Request::builder()
            .method("GET")
            .uri("/v2/_catalog")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["repositories"], serde_json::json!([name]));

    // DELETE manifest by tag must be rejected with 405.
    let (status, _headers, _body) = send(
        &app,
        Request::builder()
            .method("DELETE")
            .uri(format!("/v2/{name}/manifests/v1"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::METHOD_NOT_ALLOWED,
        "DELETE manifest by tag is not allowed"
    );

    // DELETE manifest by digest succeeds.
    let (status, _headers, _body) = send(
        &app,
        Request::builder()
            .method("DELETE")
            .uri(format!("/v2/{name}/manifests/{manifest_digest}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::ACCEPTED,
        "DELETE manifest by digest must be accepted"
    );
}

#[tokio::test]
async fn referrers_api_empty_and_populated() {
    let (app, _tmp) = make_app();
    let name = "lib/referrers";

    // Upload subject blob + subject manifest.
    let config = Bytes::from_static(b"{}");
    let config_digest = Digest::sha256_of(&config);
    let (status, _h, _b) = send(
        &app,
        Request::builder()
            .method("POST")
            .uri(format!("/v2/{name}/blobs/uploads/?digest={config_digest}"))
            .body(Body::from(config.clone()))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    let subject_manifest = serde_json::json!({
        "schemaVersion": 2,
        "mediaType": "application/vnd.oci.image.manifest.v1+json",
        "config": {
            "mediaType": "application/vnd.oci.image.config.v1+json",
            "digest": config_digest.to_string(),
            "size": config.len()
        },
        "layers": []
    });
    let subject_bytes = Bytes::from(serde_json::to_vec(&subject_manifest).unwrap());
    let subject_digest = Digest::sha256_of(&subject_bytes);

    // Referrers query before any referrer exists -> empty index.
    let (status, headers, body) = send(
        &app,
        Request::builder()
            .method("GET")
            .uri(format!("/v2/{name}/referrers/{subject_digest}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        headers.get(header::CONTENT_TYPE).unwrap().to_str().unwrap(),
        "application/vnd.oci.image.index.v1+json"
    );
    let idx: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(idx["schemaVersion"], 2);
    assert_eq!(idx["manifests"], serde_json::json!([]));

    // PUT subject manifest.
    let (status, _h, _b) = send(
        &app,
        Request::builder()
            .method("PUT")
            .uri(format!("/v2/{name}/manifests/v1"))
            .header(
                header::CONTENT_TYPE,
                "application/vnd.oci.image.manifest.v1+json",
            )
            .body(Body::from(subject_bytes.clone()))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    // Upload an SBOM-style referrer manifest pointing at the subject.
    let sbom_config = Bytes::from_static(b"{\"sbom\":true}");
    let sbom_config_digest = Digest::sha256_of(&sbom_config);
    let (status, _h, _b) = send(
        &app,
        Request::builder()
            .method("POST")
            .uri(format!(
                "/v2/{name}/blobs/uploads/?digest={sbom_config_digest}"
            ))
            .body(Body::from(sbom_config.clone()))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    let referrer = serde_json::json!({
        "schemaVersion": 2,
        "mediaType": "application/vnd.oci.image.manifest.v1+json",
        "artifactType": "application/spdx+json",
        "config": {
            "mediaType": "application/vnd.oci.image.config.v1+json",
            "digest": sbom_config_digest.to_string(),
            "size": sbom_config.len()
        },
        "layers": [],
        "subject": {
            "mediaType": "application/vnd.oci.image.manifest.v1+json",
            "digest": subject_digest.to_string(),
            "size": subject_bytes.len()
        }
    });
    let referrer_bytes = Bytes::from(serde_json::to_vec(&referrer).unwrap());
    let referrer_digest = Digest::sha256_of(&referrer_bytes);
    let (status, headers, _b) = send(
        &app,
        Request::builder()
            .method("PUT")
            .uri(format!("/v2/{name}/manifests/{referrer_digest}"))
            .header(
                header::CONTENT_TYPE,
                "application/vnd.oci.image.manifest.v1+json",
            )
            .body(Body::from(referrer_bytes.clone()))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    // Subject header must be echoed back per spec §3.3.
    assert_eq!(
        headers.get("OCI-Subject").unwrap().to_str().unwrap(),
        subject_digest.to_string()
    );

    // Referrers query -> one entry.
    let (status, _headers, body) = send(
        &app,
        Request::builder()
            .method("GET")
            .uri(format!("/v2/{name}/referrers/{subject_digest}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let idx: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let manifests = idx["manifests"].as_array().unwrap();
    assert_eq!(manifests.len(), 1);
    assert_eq!(manifests[0]["digest"], referrer_digest.to_string());
    assert_eq!(manifests[0]["artifactType"], "application/spdx+json");

    // Referrers query with filter -> OCI-Filters-Applied header.
    let (status, headers, body) = send(
        &app,
        Request::builder()
            .method("GET")
            .uri(format!(
                "/v2/{name}/referrers/{subject_digest}?artifactType=application/spdx%2Bjson"
            ))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        headers
            .get("OCI-Filters-Applied")
            .expect("filter header")
            .to_str()
            .unwrap(),
        "artifactType"
    );
    let idx: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let manifests = idx["manifests"].as_array().unwrap();
    assert_eq!(manifests.len(), 1);
}

// ---- OCI Distribution Spec v1.1 §end-12 conformance: References setup
// ---- Mirrors the failing testcases in
// ----   `tests/conformance/oci/reports/baseline/junit.xml`
// ---- under "Content Discovery / References ...".
//
// The conformance suite Setup pushes 5 referrer manifests pointing at
// the same subject and then asserts:
// - GET /referrers/{subject} returns 5 manifests
// - GET /referrers/{subject}?artifactType=<one type> returns 2
//   (two of the five share an artifactType; the others differ)
// - GET /referrers/{never-pushed-subject} returns 1 (the suite also
//   pushes a referrer pointing at a manifest that is NEVER itself
//   pushed — spec §end-12 explicitly allows referring to a
//   not-yet-existent subject).

#[tokio::test]
async fn referrers_setup_5_referrers_2_with_shared_artifact_type() {
    let (app, _tmp) = make_app();
    let name = "lib/conformance-ref";

    // Push the OCI empty blob (`{}` → sha256:44136fa355...) — every
    // artifact-style referrer in conformance points its `config` at
    // this descriptor.
    let empty = Bytes::from_static(b"{}");
    let empty_digest = Digest::sha256_of(&empty);
    let (status, _h, _b) = send(
        &app,
        Request::builder()
            .method("POST")
            .uri(format!("/v2/{name}/blobs/uploads/?digest={empty_digest}"))
            .body(Body::from(empty.clone()))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "empty blob push must succeed");

    // Push a subject layer + subject manifest.
    let layer = Bytes::from_static(b"subject-layer");
    let layer_digest = Digest::sha256_of(&layer);
    let (status, _h, _b) = send(
        &app,
        Request::builder()
            .method("POST")
            .uri(format!("/v2/{name}/blobs/uploads/?digest={layer_digest}"))
            .body(Body::from(layer.clone()))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    let subject_manifest = serde_json::json!({
        "schemaVersion": 2,
        "mediaType": "application/vnd.oci.image.manifest.v1+json",
        "config": {
            "mediaType": "application/vnd.oci.empty.v1+json",
            "digest": empty_digest.to_string(),
            "size": empty.len(),
        },
        "layers": [{
            "mediaType": "application/vnd.oci.image.layer.v1.tar",
            "digest": layer_digest.to_string(),
            "size": layer.len(),
        }],
    });
    let subject_bytes = Bytes::from(serde_json::to_vec(&subject_manifest).unwrap());
    let subject_digest = Digest::sha256_of(&subject_bytes);
    let (status, _h, _b) = send(
        &app,
        Request::builder()
            .method("PUT")
            .uri(format!("/v2/{name}/manifests/{subject_digest}"))
            .header(
                header::CONTENT_TYPE,
                "application/vnd.oci.image.manifest.v1+json",
            )
            .body(Body::from(subject_bytes.clone()))
            .unwrap(),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "subject manifest push must succeed"
    );

    // 5 referrer manifests — 2 share the artifactType
    // `application/vnd.test.A`; the other 3 each have a unique type.
    let artifact_types = [
        "application/vnd.test.A",
        "application/vnd.test.A", // <- duplicate to test filter count
        "application/vnd.test.B",
        "application/vnd.test.C",
        "application/vnd.test.D",
    ];
    for (i, at) in artifact_types.iter().enumerate() {
        let referrer = serde_json::json!({
            "schemaVersion": 2,
            "mediaType": "application/vnd.oci.image.manifest.v1+json",
            "artifactType": at,
            "config": {
                "mediaType": "application/vnd.oci.empty.v1+json",
                "digest": empty_digest.to_string(),
                "size": empty.len(),
            },
            "layers": [{
                "mediaType": "application/vnd.oci.empty.v1+json",
                "digest": empty_digest.to_string(),
                "size": empty.len(),
            }],
            "subject": {
                "mediaType": "application/vnd.oci.image.manifest.v1+json",
                "digest": subject_digest.to_string(),
                "size": subject_bytes.len(),
            },
            "annotations": {
                "test.id": format!("ref-{i}"),
            }
        });
        let referrer_bytes = Bytes::from(serde_json::to_vec(&referrer).unwrap());
        let referrer_digest = Digest::sha256_of(&referrer_bytes);
        let (status, _headers, _body) = send(
            &app,
            Request::builder()
                .method("PUT")
                .uri(format!("/v2/{name}/manifests/{referrer_digest}"))
                .header(
                    header::CONTENT_TYPE,
                    "application/vnd.oci.image.manifest.v1+json",
                )
                .body(Body::from(referrer_bytes))
                .unwrap(),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::CREATED,
            "referrer #{i} (artifactType={at}) PUT must succeed"
        );
    }

    // Listing all referrers must return all 5.
    let (status, _headers, body) = send(
        &app,
        Request::builder()
            .method("GET")
            .uri(format!("/v2/{name}/referrers/{subject_digest}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let idx: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let manifests = idx["manifests"].as_array().unwrap();
    assert_eq!(
        manifests.len(),
        5,
        "all 5 referrers must surface; got {manifests:#?}"
    );

    // Filter to artifactType A — must return exactly the 2 sharing it.
    let (status, headers, body) = send(
        &app,
        Request::builder()
            .method("GET")
            .uri(format!(
                "/v2/{name}/referrers/{subject_digest}?artifactType=application/vnd.test.A"
            ))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        headers
            .get("OCI-Filters-Applied")
            .expect("filter header")
            .to_str()
            .unwrap(),
        "artifactType"
    );
    let idx: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let manifests = idx["manifests"].as_array().unwrap();
    assert_eq!(
        manifests.len(),
        2,
        "filter must return exactly the 2 referrers sharing the type"
    );
    for m in manifests {
        assert_eq!(m["artifactType"], "application/vnd.test.A");
    }
}

#[tokio::test]
async fn referrer_with_unpushed_oci_empty_config_should_succeed() {
    // OCI Image Spec v1.1 §3 designates the empty descriptor
    // (sha256:44136fa355...) as a well-known always-supported
    // payload — the conformance suite uses it as the `config` for
    // every referrer manifest *without* explicitly pushing the
    // empty blob first. Until this hardening lands the registry
    // returned 404 MANIFEST_BLOB_UNKNOWN on those PUTs, which is
    // the root cause of the 4 Content-Discovery conformance
    // failures the dd-r3-remediation OCI row tracks.
    let (app, _tmp) = make_app();
    let name = "lib/empty-config";

    let phantom_subject = Digest::sha256_of(b"phantom-subject");

    // Note: empty blob NOT pushed.
    let referrer = serde_json::json!({
        "schemaVersion": 2,
        "mediaType": "application/vnd.oci.image.manifest.v1+json",
        "artifactType": "application/vnd.test.empty-config",
        "config": {
            "mediaType": "application/vnd.oci.empty.v1+json",
            "digest": "sha256:44136fa355b3678a1146ad16f7e8649e94fb4fc21fe77e8310c060f61caaff8a",
            "size": 2,
        },
        "layers": [{
            "mediaType": "application/vnd.oci.empty.v1+json",
            "digest": "sha256:44136fa355b3678a1146ad16f7e8649e94fb4fc21fe77e8310c060f61caaff8a",
            "size": 2,
        }],
        "subject": {
            "mediaType": "application/vnd.oci.image.manifest.v1+json",
            "digest": phantom_subject.to_string(),
            "size": 16,
        },
    });
    let referrer_bytes = Bytes::from(serde_json::to_vec(&referrer).unwrap());
    let referrer_digest = Digest::sha256_of(&referrer_bytes);
    let (status, _h, body) = send(
        &app,
        Request::builder()
            .method("PUT")
            .uri(format!("/v2/{name}/manifests/{referrer_digest}"))
            .header(
                header::CONTENT_TYPE,
                "application/vnd.oci.image.manifest.v1+json",
            )
            .body(Body::from(referrer_bytes))
            .unwrap(),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "PUT of a referrer using the OCI empty descriptor without an \
         explicit blob upload must succeed (Image Spec §3) — got {status:?} body={body:?}"
    );
}

#[tokio::test]
async fn referrers_for_missing_subject_manifest_returns_registered_referrers() {
    // Spec §end-12: subject MAY refer to a non-existent manifest.
    // Conformance pushes a referrer whose subject digest is one
    // that never gets pushed itself, then GETs /referrers/<that>
    // and expects the referrer to be listed (200 OK + 1 entry).
    let (app, _tmp) = make_app();
    let name = "lib/missing-subject";

    let empty = Bytes::from_static(b"{}");
    let empty_digest = Digest::sha256_of(&empty);
    let (status, _h, _b) = send(
        &app,
        Request::builder()
            .method("POST")
            .uri(format!("/v2/{name}/blobs/uploads/?digest={empty_digest}"))
            .body(Body::from(empty.clone()))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    // Subject digest is computed locally; the manifest is NEVER
    // pushed.
    let phantom_bytes = b"{\"phantom\":true}";
    let phantom_digest = Digest::sha256_of(phantom_bytes);

    // Push a referrer pointing at the phantom subject.
    let referrer = serde_json::json!({
        "schemaVersion": 2,
        "mediaType": "application/vnd.oci.image.manifest.v1+json",
        "artifactType": "application/vnd.test.phantom",
        "config": {
            "mediaType": "application/vnd.oci.empty.v1+json",
            "digest": empty_digest.to_string(),
            "size": empty.len(),
        },
        "layers": [],
        "subject": {
            "mediaType": "application/vnd.oci.image.manifest.v1+json",
            "digest": phantom_digest.to_string(),
            "size": phantom_bytes.len(),
        },
    });
    let referrer_bytes = Bytes::from(serde_json::to_vec(&referrer).unwrap());
    let referrer_digest = Digest::sha256_of(&referrer_bytes);
    let (status, _h, _b) = send(
        &app,
        Request::builder()
            .method("PUT")
            .uri(format!("/v2/{name}/manifests/{referrer_digest}"))
            .header(
                header::CONTENT_TYPE,
                "application/vnd.oci.image.manifest.v1+json",
            )
            .body(Body::from(referrer_bytes))
            .unwrap(),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "PUT of a referrer with non-existent subject must succeed (spec §end-12)"
    );

    let (status, _h, body) = send(
        &app,
        Request::builder()
            .method("GET")
            .uri(format!("/v2/{name}/referrers/{phantom_digest}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let idx: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let manifests = idx["manifests"].as_array().unwrap();
    assert_eq!(
        manifests.len(),
        1,
        "referrer pointing at a never-pushed subject must still surface in /referrers/<that>"
    );
}

#[tokio::test]
async fn error_blob_unknown_returns_404_with_spec_shape() {
    let (app, _tmp) = make_app();
    let bogus = format!("sha256:{}", "0".repeat(64));
    let (status, headers, body) = send(
        &app,
        Request::builder()
            .method("GET")
            .uri(format!("/v2/lib/app/blobs/{bogus}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(
        headers.get(header::CONTENT_TYPE).unwrap().to_str().unwrap(),
        "application/json"
    );
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["errors"][0]["code"], "BLOB_UNKNOWN");
}

#[tokio::test]
async fn error_digest_invalid_returns_400() {
    let (app, _tmp) = make_app();
    let (status, _headers, body) = send(
        &app,
        Request::builder()
            .method("GET")
            .uri("/v2/lib/app/blobs/sha256:deadbeef")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["errors"][0]["code"], "DIGEST_INVALID");
}

#[tokio::test]
async fn error_name_invalid_returns_400() {
    let (app, _tmp) = make_app();
    let digest = format!("sha256:{}", "a".repeat(64));
    let (status, _headers, body) = send(
        &app,
        Request::builder()
            .method("GET")
            .uri(format!("/v2/INVALID_UPPER/blobs/{digest}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["errors"][0]["code"], "NAME_INVALID");
}

#[tokio::test]
async fn error_manifest_blob_unknown_when_layer_missing() {
    let (app, _tmp) = make_app();
    let name = "lib/broken";

    // Don't upload any blob; reference a phantom layer.
    let phantom = format!("sha256:{}", "b".repeat(64));
    let manifest = serde_json::json!({
        "schemaVersion": 2,
        "mediaType": "application/vnd.oci.image.manifest.v1+json",
        "config": {
            "mediaType": "application/vnd.oci.image.config.v1+json",
            "digest": phantom,
            "size": 1
        },
        "layers": []
    });
    let body_bytes = Bytes::from(serde_json::to_vec(&manifest).unwrap());
    let (status, _headers, body) = send(
        &app,
        Request::builder()
            .method("PUT")
            .uri(format!("/v2/{name}/manifests/v1"))
            .header(
                header::CONTENT_TYPE,
                "application/vnd.oci.image.manifest.v1+json",
            )
            .body(Body::from(body_bytes))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["errors"][0]["code"], "MANIFEST_BLOB_UNKNOWN");
}

#[tokio::test]
async fn error_manifest_unknown_returns_404() {
    let (app, _tmp) = make_app();
    let (status, _headers, body) = send(
        &app,
        Request::builder()
            .method("GET")
            .uri("/v2/lib/app/manifests/missing")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["errors"][0]["code"], "MANIFEST_UNKNOWN");
}

// OCI Distribution Spec v1.1 §3.2 + conformance suite tests
// `Pull manifests HEAD/GET nonexistent manifest should return 404`
// require a 404 MANIFEST_UNKNOWN response even when the reference
// itself is syntactically invalid (bad digest hex, tag with control
// chars, unknown algorithm prefix, etc). Earlier releases returned
// 400 DIGEST_INVALID / MANIFEST_INVALID here, which the
// conformance suite flagged. The Phase 7 fix routes invalid
// references through the same 404 path as a valid-but-missing tag.

#[tokio::test]
async fn manifest_get_with_invalid_digest_hex_returns_404_not_400() {
    let (app, _tmp) = make_app();
    let (status, _headers, body) = send(
        &app,
        Request::builder()
            .method("GET")
            // Valid algo prefix but hex too short — earlier returned
            // 400 DIGEST_INVALID; spec §3.2 wants 404.
            .uri("/v2/lib/app/manifests/sha256:deadbeef")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["errors"][0]["code"], "MANIFEST_UNKNOWN");
}

#[tokio::test]
async fn manifest_head_with_unknown_algo_prefix_returns_404_not_400() {
    let (app, _tmp) = make_app();
    let (status, _headers, _body) = send(
        &app,
        Request::builder()
            .method("HEAD")
            // `md5:` is not a registered manifest digest algorithm —
            // earlier returned 400 MANIFEST_INVALID; spec wants 404.
            .uri("/v2/lib/app/manifests/md5:0123456789abcdef0123456789abcdef")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn manifest_get_with_invalid_tag_chars_returns_404_not_400() {
    let (app, _tmp) = make_app();
    let (status, _headers, body) = send(
        &app,
        Request::builder()
            .method("GET")
            // Leading non-alphanumeric — `is_valid_tag` rejected this
            // and we used to surface MANIFEST_INVALID 400; spec wants
            // 404 because the conformance suite treats every
            // unparseable reference on the GET path as "not found".
            .uri("/v2/lib/app/manifests/.bad-tag")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["errors"][0]["code"], "MANIFEST_UNKNOWN");
}

#[tokio::test]
async fn upload_digest_mismatch_on_finalize_returns_digest_invalid() {
    let (app, _tmp) = make_app();
    let name = "lib/app";
    // Start upload.
    let (status, headers, _body) = send(
        &app,
        Request::builder()
            .method("POST")
            .uri(format!("/v2/{name}/blobs/uploads/"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let location = headers
        .get(header::LOCATION)
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();

    // Append 4 bytes of "abcd".
    let (status, _h, _b) = send(
        &app,
        Request::builder()
            .method("PATCH")
            .uri(location.clone())
            .header(header::CONTENT_RANGE, "0-3")
            .body(Body::from(Bytes::from_static(b"abcd")))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);

    // Finalize with the WRONG digest.
    let wrong = Digest::sha256_of(b"not-abcd");
    let (status, _headers, body) = send(
        &app,
        Request::builder()
            .method("PUT")
            .uri(format!("{location}?digest={wrong}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["errors"][0]["code"], "DIGEST_INVALID");
}

#[tokio::test]
async fn cancel_upload_after_start() {
    let (app, _tmp) = make_app();
    let name = "lib/cancel";

    let (status, headers, _body) = send(
        &app,
        Request::builder()
            .method("POST")
            .uri(format!("/v2/{name}/blobs/uploads/"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let location = headers
        .get(header::LOCATION)
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();

    let (status, _h, _b) = send(
        &app,
        Request::builder()
            .method("DELETE")
            .uri(location.clone())
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // Cancelling again returns BLOB_UPLOAD_UNKNOWN.
    let (status, _h, body) = send(
        &app,
        Request::builder()
            .method("DELETE")
            .uri(location)
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["errors"][0]["code"], "BLOB_UPLOAD_UNKNOWN");
}

// ---------------------------------------------------------------------------
// F-R2-023 — manifest GET honours `Range` (RFC 7233).
//
// The four tests below cover:
//   * `Range: bytes=0-99` returns 206 with exactly 100 bytes plus
//     `Content-Range` and `Accept-Ranges` headers;
//   * `Range: bytes=99999-` past the end returns 416;
//   * a request with no `Range` header still returns the full 200 body
//     (existing behaviour preserved);
//   * `Range: bytes=N-` open-ended is satisfied to end-of-body.
// ---------------------------------------------------------------------------

/// Helper: PUT a small manifest and return its tag and digest.
async fn put_small_manifest(app: &Router, name: &str) -> (String, Digest, Bytes) {
    // Push a config blob so the manifest validates.
    let config = Bytes::from_static(b"{\"architecture\":\"amd64\",\"os\":\"linux\"}");
    let config_digest = Digest::sha256_of(&config);
    let (status, _h, _b) = send(
        app,
        Request::builder()
            .method("POST")
            .uri(format!("/v2/{name}/blobs/uploads/?digest={config_digest}"))
            .body(Body::from(config.clone()))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    let manifest_body = serde_json::json!({
        "schemaVersion": 2,
        "mediaType": "application/vnd.oci.image.manifest.v1+json",
        "config": {
            "mediaType": "application/vnd.oci.image.config.v1+json",
            "digest": config_digest.to_string(),
            "size": config.len()
        },
        "layers": []
    });
    let manifest_bytes = Bytes::from(serde_json::to_vec(&manifest_body).unwrap());
    let manifest_digest = Digest::sha256_of(&manifest_bytes);

    let (status, _h, _b) = send(
        app,
        Request::builder()
            .method("PUT")
            .uri(format!("/v2/{name}/manifests/range-tag"))
            .header(
                header::CONTENT_TYPE,
                "application/vnd.oci.image.manifest.v1+json",
            )
            .body(Body::from(manifest_bytes.clone()))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    ("range-tag".to_owned(), manifest_digest, manifest_bytes)
}

#[tokio::test]
async fn manifest_get_range_first_100_bytes_returns_206() {
    let (app, _tmp) = make_app();
    let (tag, _digest, full_body) = put_small_manifest(&app, "lib/range-app").await;

    // `bytes=0-99` selects the first 100 bytes of the manifest.
    // The fixture manifest is at least 100 bytes (config descriptor +
    // layers array + JSON framing), but we sanity-check anyway.
    assert!(
        full_body.len() >= 100,
        "fixture manifest must be ≥100 bytes for this test; was {}",
        full_body.len()
    );

    let (status, headers, body) = send(
        &app,
        Request::builder()
            .method("GET")
            .uri(format!("/v2/lib/range-app/manifests/{tag}"))
            .header(header::RANGE, "bytes=0-99")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::PARTIAL_CONTENT,
        "Range request must produce 206"
    );
    assert_eq!(body.len(), 100, "body must contain exactly 100 bytes");
    assert_eq!(&body[..], &full_body[0..100]);

    let cr = headers
        .get(header::CONTENT_RANGE)
        .expect("Content-Range header")
        .to_str()
        .unwrap();
    assert_eq!(cr, format!("bytes 0-99/{}", full_body.len()));

    let ar = headers
        .get(header::ACCEPT_RANGES)
        .expect("Accept-Ranges header")
        .to_str()
        .unwrap();
    assert_eq!(ar, "bytes");

    // Content-Length must equal the slice length, not the total size.
    let cl = headers
        .get(header::CONTENT_LENGTH)
        .expect("Content-Length header")
        .to_str()
        .unwrap()
        .parse::<usize>()
        .unwrap();
    assert_eq!(cl, 100);
}

#[tokio::test]
async fn manifest_get_range_out_of_bounds_returns_416() {
    let (app, _tmp) = make_app();
    let (tag, _digest, full_body) = put_small_manifest(&app, "lib/range-oob").await;

    let (status, headers, _body) = send(
        &app,
        Request::builder()
            .method("GET")
            .uri(format!("/v2/lib/range-oob/manifests/{tag}"))
            .header(header::RANGE, "bytes=99999-")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::RANGE_NOT_SATISFIABLE,
        "out-of-bounds start must produce 416"
    );

    // Per RFC 7233 §4.4, a 416 SHOULD carry `Content-Range: bytes */N`.
    let cr = headers
        .get(header::CONTENT_RANGE)
        .expect("Content-Range header on 416")
        .to_str()
        .unwrap();
    assert_eq!(cr, format!("bytes */{}", full_body.len()));
}

#[tokio::test]
async fn manifest_get_without_range_returns_full_body_200() {
    let (app, _tmp) = make_app();
    let (tag, _digest, full_body) = put_small_manifest(&app, "lib/range-full").await;

    let (status, headers, body) = send(
        &app,
        Request::builder()
            .method("GET")
            .uri(format!("/v2/lib/range-full/manifests/{tag}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "no Range header → 200 OK");
    assert_eq!(&body[..], &full_body[..], "no Range header → full body");

    // Even on the 200 path we advertise range support.
    let ar = headers
        .get(header::ACCEPT_RANGES)
        .expect("Accept-Ranges header")
        .to_str()
        .unwrap();
    assert_eq!(ar, "bytes");
}

#[tokio::test]
async fn manifest_get_open_ended_range_satisfied_to_end() {
    let (app, _tmp) = make_app();
    let (tag, _digest, full_body) = put_small_manifest(&app, "lib/range-open").await;
    let total = full_body.len();
    assert!(total > 10, "manifest must be > 10 bytes for this test");

    // `bytes=10-` is valid and means "from byte 10 to end-of-body".
    let (status, headers, body) = send(
        &app,
        Request::builder()
            .method("GET")
            .uri(format!("/v2/lib/range-open/manifests/{tag}"))
            .header(header::RANGE, "bytes=10-")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::PARTIAL_CONTENT);
    assert_eq!(&body[..], &full_body[10..]);

    let cr = headers
        .get(header::CONTENT_RANGE)
        .expect("Content-Range header")
        .to_str()
        .unwrap();
    let last = total - 1;
    assert_eq!(cr, format!("bytes 10-{last}/{total}"));
}
