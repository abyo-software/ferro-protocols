// SPDX-License-Identifier: Apache-2.0
//! Blob-upload endpoints.
//!
//! Spec: OCI Distribution Spec v1.1 §4.3 "Pushing a blob in chunks",
//! §4.4 "Pushing a blob monolithically", §4.5 "Pushing a blob from a
//! URL", §4.6 "Mounting a blob from another repository", §4.7
//! "Completing an upload", §4.8 "Cancelling an upload".
//!
//! Endpoints implemented here:
//!
//! - `POST /v2/{name}/blobs/uploads/` — start an upload session (or
//!   perform a monolithic push if `?digest=` and a body are present);
//! - `PATCH /v2/{name}/blobs/uploads/{uuid}` — append a chunk;
//! - `PUT /v2/{name}/blobs/uploads/{uuid}?digest=<digest>` — finalize;
//! - `GET /v2/{name}/blobs/uploads/{uuid}` — current upload state;
//! - `DELETE /v2/{name}/blobs/uploads/{uuid}` — cancel.

use std::collections::BTreeMap;

use axum::body::Bytes;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use ferro_blob_store::Digest;

use crate::error::{OciError, OciErrorCode};
use crate::reference::validate_name;
use crate::router::AppState;
use crate::upload::ContentRange;

fn parse_digest(s: &str) -> Result<Digest, OciError> {
    s.parse::<Digest>().map_err(|e| {
        OciError::new(
            OciErrorCode::DigestInvalid,
            format!("invalid digest `{s}`: {e}"),
        )
    })
}

fn upload_location_headers(name: &str, uuid: &str, new_offset: u64) -> HeaderMap {
    let mut headers = HeaderMap::new();
    let location = format!("/v2/{name}/blobs/uploads/{uuid}");
    if let Ok(v) = HeaderValue::from_str(&location) {
        headers.insert(header::LOCATION, v);
    }
    // Spec §4.3: Range header reports the inclusive byte range of
    // bytes currently buffered; for an empty upload this is `0-0`,
    // which also indicates the next byte to be written is `0`.
    let range = if new_offset == 0 {
        "0-0".to_owned()
    } else {
        format!("0-{}", new_offset - 1)
    };
    if let Ok(v) = HeaderValue::from_str(&range) {
        headers.insert(header::RANGE, v);
    }
    if let Ok(v) = HeaderValue::from_str(uuid) {
        headers.insert("Docker-Upload-UUID", v);
    }
    headers.insert("OCI-Chunk-Min-Length", HeaderValue::from_static("0"));
    headers
}

/// Handle `POST /v2/{name}/blobs/uploads/`.
///
/// Spec: OCI Distribution Spec v1.1 §4.3 and §4.4.
///
/// When `digest` is present as a query parameter and the request has a
/// body, perform a monolithic upload and return `201 Created`.
/// Otherwise, allocate a new upload UUID and return `202 Accepted`
/// with the upload `Location`.
pub async fn init_upload(
    state: &AppState,
    name: &str,
    _headers: &HeaderMap,
    params: &BTreeMap<String, String>,
    body: Bytes,
) -> Response {
    if let Err(e) = validate_name(name) {
        return e.into_response();
    }

    // Mount-from-another-repo (§4.6) is a Phase-2 feature; the
    // spec says "If the server does not support cross-repository
    // mounting, it SHOULD discard the mount parameters and return
    // 202 Accepted with a standard upload location". We do exactly
    // that — fall through to the start-session branch.

    if let Some(digest_str) = params.get("digest") {
        // Monolithic upload branch.
        let digest = match parse_digest(digest_str) {
            Ok(d) => d,
            Err(e) => return e.into_response(),
        };
        // Integrity check.
        let actual = Digest::sha256_of(&body);
        if actual.algo() == digest.algo() && actual.hex() != digest.hex() {
            return OciError::new(
                OciErrorCode::DigestInvalid,
                format!("digest mismatch: declared {digest}, computed {actual}"),
            )
            .into_response();
        }
        if let Err(e) = state.blob_store.put(&digest, body).await {
            return OciError::from(e).into_response();
        }
        return blob_created_response(name, &digest);
    }

    // Start-session branch.
    let uuid = match state.registry.start_upload(name).await {
        Ok(u) => u,
        Err(e) => return OciError::from(e).into_response(),
    };
    let headers = upload_location_headers(name, &uuid, 0);
    (StatusCode::ACCEPTED, headers).into_response()
}

/// Handle `PATCH /v2/{name}/blobs/uploads/{uuid}`.
///
/// Spec: OCI Distribution Spec v1.1 §4.3.
///
/// Expects a `Content-Range: <start>-<end>` header matching the
/// current offset of the upload (contiguous chunks only).
pub async fn patch_upload(
    state: &AppState,
    name: &str,
    uuid: &str,
    headers: &HeaderMap,
    body: Bytes,
) -> Response {
    if let Err(e) = validate_name(name) {
        return e.into_response();
    }

    // Validate upload exists.
    let existing = match state.registry.get_upload_state(name, uuid).await {
        Ok(v) => v,
        Err(e) => return OciError::from(e).into_response(),
    };
    let Some(state_snapshot) = existing else {
        return OciError::new(
            OciErrorCode::BlobUploadUnknown,
            format!("unknown upload uuid {uuid}"),
        )
        .into_response();
    };

    let expected_offset = state_snapshot.offset();
    let chunk_start = match headers.get(header::CONTENT_RANGE) {
        Some(v) => {
            let Ok(s) = v.to_str() else {
                return OciError::new(OciErrorCode::BlobUploadInvalid, "non-ASCII Content-Range")
                    .into_response();
            };
            match ContentRange::parse(s) {
                Ok(r) => r.start,
                Err(e) => {
                    return OciError::new(
                        OciErrorCode::BlobUploadInvalid,
                        format!("malformed Content-Range `{s}`: {e}"),
                    )
                    .into_response();
                }
            }
        }
        None => expected_offset,
    };

    if chunk_start != expected_offset {
        return OciError::new(
            OciErrorCode::BlobUploadInvalid,
            format!("out-of-order chunk: expected offset {expected_offset}, got {chunk_start}"),
        )
        .with_status(StatusCode::RANGE_NOT_SATISFIABLE)
        .into_response();
    }

    let new_offset = match state
        .registry
        .append_upload(name, uuid, chunk_start, body)
        .await
    {
        Ok(o) => o,
        Err(e) => return OciError::from(e).into_response(),
    };

    let headers = upload_location_headers(name, uuid, new_offset);
    (StatusCode::ACCEPTED, headers).into_response()
}

/// Handle `PUT /v2/{name}/blobs/uploads/{uuid}?digest=<digest>`.
///
/// Spec: OCI Distribution Spec v1.1 §4.7 "Completing an upload".
///
/// May include a trailing body (the final chunk) which is appended
/// before the digest is verified.
pub async fn finish_upload(
    state: &AppState,
    name: &str,
    uuid: &str,
    params: &BTreeMap<String, String>,
    body: Bytes,
) -> Response {
    if let Err(e) = validate_name(name) {
        return e.into_response();
    }

    let Some(digest_str) = params.get("digest") else {
        return OciError::new(
            OciErrorCode::DigestInvalid,
            "missing `digest` query parameter",
        )
        .into_response();
    };
    let declared = match parse_digest(digest_str) {
        Ok(d) => d,
        Err(e) => return e.into_response(),
    };

    // Verify session exists.
    let existing = match state.registry.get_upload_state(name, uuid).await {
        Ok(v) => v,
        Err(e) => return OciError::from(e).into_response(),
    };
    let Some(state_snapshot) = existing else {
        return OciError::new(
            OciErrorCode::BlobUploadUnknown,
            format!("unknown upload uuid {uuid}"),
        )
        .into_response();
    };

    // Append the final chunk if the PUT carried one.
    if !body.is_empty()
        && let Err(e) = state
            .registry
            .append_upload(name, uuid, state_snapshot.offset(), body)
            .await
    {
        return OciError::from(e).into_response();
    }

    // Take the accumulated bytes and hand them to the blob store.
    let bytes = match state.registry.take_upload_bytes(name, uuid).await {
        Ok(Some(b)) => b,
        Ok(None) => {
            return OciError::new(
                OciErrorCode::BlobUploadUnknown,
                format!("upload {uuid} has no buffered bytes"),
            )
            .into_response();
        }
        Err(e) => return OciError::from(e).into_response(),
    };

    // Verify the recomputed digest matches the declared one.
    let actual = Digest::sha256_of(&bytes);
    if declared.algo() == actual.algo() && actual.hex() != declared.hex() {
        return OciError::new(
            OciErrorCode::DigestInvalid,
            format!("digest mismatch: declared {declared}, computed {actual}"),
        )
        .into_response();
    }

    if let Err(e) = state.blob_store.put(&declared, bytes).await {
        return OciError::from(e).into_response();
    }
    if let Err(e) = state.registry.complete_upload(name, uuid, &declared).await {
        return OciError::from(e).into_response();
    }

    blob_created_response(name, &declared)
}

/// Handle `GET /v2/{name}/blobs/uploads/{uuid}`.
///
/// Spec: OCI Distribution Spec v1.1 §4.3 "Upload state".
pub async fn get_upload_status(state: &AppState, name: &str, uuid: &str) -> Response {
    if let Err(e) = validate_name(name) {
        return e.into_response();
    }
    let existing = match state.registry.get_upload_state(name, uuid).await {
        Ok(v) => v,
        Err(e) => return OciError::from(e).into_response(),
    };
    let Some(s) = existing else {
        return OciError::new(
            OciErrorCode::BlobUploadUnknown,
            format!("unknown upload uuid {uuid}"),
        )
        .into_response();
    };
    let headers = upload_location_headers(name, uuid, s.offset());
    (StatusCode::NO_CONTENT, headers).into_response()
}

/// Handle `DELETE /v2/{name}/blobs/uploads/{uuid}`.
///
/// Spec: OCI Distribution Spec v1.1 §4.8 "Cancelling an upload".
pub async fn cancel_upload(state: &AppState, name: &str, uuid: &str) -> Response {
    if let Err(e) = validate_name(name) {
        return e.into_response();
    }
    let removed = match state.registry.cancel_upload(name, uuid).await {
        Ok(b) => b,
        Err(e) => return OciError::from(e).into_response(),
    };
    if !removed {
        return OciError::new(
            OciErrorCode::BlobUploadUnknown,
            format!("unknown upload uuid {uuid}"),
        )
        .into_response();
    }
    (StatusCode::NO_CONTENT, HeaderMap::new()).into_response()
}

fn blob_created_response(name: &str, digest: &Digest) -> Response {
    let mut headers = HeaderMap::new();
    let location = format!("/v2/{name}/blobs/{digest}");
    if let Ok(v) = HeaderValue::from_str(&location) {
        headers.insert(header::LOCATION, v);
    }
    if let Ok(v) = HeaderValue::from_str(&digest.to_string()) {
        headers.insert("Docker-Content-Digest", v);
    }
    headers.insert(header::CONTENT_LENGTH, HeaderValue::from(0u64));
    (StatusCode::CREATED, headers).into_response()
}
