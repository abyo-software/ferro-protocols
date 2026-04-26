// SPDX-License-Identifier: Apache-2.0
//! Blob endpoints.
//!
//! Spec: OCI Distribution Spec v1.1 §3.2 "Pulling blobs" and §4.9
//! "Deleting blobs".
//!
//! - `GET /v2/{name}/blobs/{digest}` — fetch a blob;
//! - `HEAD /v2/{name}/blobs/{digest}` — existence check, same headers as
//!   GET but no body;
//! - `DELETE /v2/{name}/blobs/{digest}` — delete a blob.
//!
//! Every response carries `Content-Length`, `Docker-Content-Digest`,
//! and `ETag` so clients can cache aggressively.

use axum::body::Body;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use ferro_blob_store::Digest;

use crate::error::{OciError, OciErrorCode};
use crate::reference::validate_name;
use crate::router::AppState;

fn parse_digest(s: &str) -> Result<Digest, OciError> {
    s.parse::<Digest>().map_err(|e| {
        OciError::new(
            OciErrorCode::DigestInvalid,
            format!("invalid digest `{s}`: {e}"),
        )
    })
}

fn common_blob_headers(digest: &Digest, size: usize) -> HeaderMap {
    let mut headers = HeaderMap::new();
    let digest_str = digest.to_string();
    if let Ok(v) = HeaderValue::from_str(&digest_str) {
        headers.insert("Docker-Content-Digest", v.clone());
        // ETag: quoted digest, matches Docker Registry convention.
        if let Ok(etag) = HeaderValue::from_str(&format!("\"{digest_str}\"")) {
            headers.insert(header::ETAG, etag);
        }
    }
    headers.insert(header::CONTENT_LENGTH, HeaderValue::from(size as u64));
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    headers
}

/// SHA-256 digest of the OCI empty descriptor payload (`{}`).
///
/// OCI Image Spec v1.1 §3 designates this digest as a well-known,
/// always-supported payload. Mirrors the constant in
/// [`super::manifest`] so blob GET / HEAD requests for the empty
/// descriptor are served synthetically without a prior upload.
const OCI_EMPTY_DESCRIPTOR_DIGEST: &str =
    "sha256:44136fa355b3678a1146ad16f7e8649e94fb4fc21fe77e8310c060f61caaff8a";

/// The two bytes the empty descriptor digest covers.
const OCI_EMPTY_DESCRIPTOR_BYTES: &[u8] = b"{}";

/// Handle `GET /v2/{name}/blobs/{digest}`.
///
/// Spec: OCI Distribution Spec v1.1 §3.2 "Pulling blobs".
pub async fn get_blob(state: &AppState, name: &str, digest_str: &str) -> Response {
    if let Err(e) = validate_name(name) {
        return e.into_response();
    }
    let digest = match parse_digest(digest_str) {
        Ok(d) => d,
        Err(e) => return e.into_response(),
    };
    if digest_str == OCI_EMPTY_DESCRIPTOR_DIGEST {
        let bytes = bytes::Bytes::from_static(OCI_EMPTY_DESCRIPTOR_BYTES);
        let headers = common_blob_headers(&digest, bytes.len());
        return (StatusCode::OK, headers, Body::from(bytes)).into_response();
    }
    let bytes = match state.blob_store.get(&digest).await {
        Ok(b) => b,
        Err(ferro_blob_store::BlobStoreError::NotFound(_)) => {
            return OciError::new(
                OciErrorCode::BlobUnknown,
                format!("blob {digest} not found"),
            )
            .into_response();
        }
        Err(e) => return OciError::from(e).into_response(),
    };
    let len = bytes.len();
    let headers = common_blob_headers(&digest, len);
    (StatusCode::OK, headers, Body::from(bytes)).into_response()
}

/// Handle `HEAD /v2/{name}/blobs/{digest}`.
///
/// Spec: OCI Distribution Spec v1.1 §3.2.
pub async fn head_blob(state: &AppState, name: &str, digest_str: &str) -> Response {
    if let Err(e) = validate_name(name) {
        return e.into_response();
    }
    let digest = match parse_digest(digest_str) {
        Ok(d) => d,
        Err(e) => return e.into_response(),
    };
    if digest_str == OCI_EMPTY_DESCRIPTOR_DIGEST {
        let headers = common_blob_headers(&digest, OCI_EMPTY_DESCRIPTOR_BYTES.len());
        return (StatusCode::OK, headers).into_response();
    }
    let bytes = match state.blob_store.get(&digest).await {
        Ok(b) => b,
        Err(ferro_blob_store::BlobStoreError::NotFound(_)) => {
            return OciError::new(
                OciErrorCode::BlobUnknown,
                format!("blob {digest} not found"),
            )
            .into_response();
        }
        Err(e) => return OciError::from(e).into_response(),
    };
    let headers = common_blob_headers(&digest, bytes.len());
    (StatusCode::OK, headers).into_response()
}

/// Handle `DELETE /v2/{name}/blobs/{digest}`.
///
/// Spec: OCI Distribution Spec v1.1 §4.9 "Deleting blobs".
pub async fn delete_blob(state: &AppState, name: &str, digest_str: &str) -> Response {
    if let Err(e) = validate_name(name) {
        return e.into_response();
    }
    let digest = match parse_digest(digest_str) {
        Ok(d) => d,
        Err(e) => return e.into_response(),
    };
    let exists = match state.blob_store.contains(&digest).await {
        Ok(b) => b,
        Err(e) => return OciError::from(e).into_response(),
    };
    if !exists {
        return OciError::new(
            OciErrorCode::BlobUnknown,
            format!("blob {digest} not found"),
        )
        .into_response();
    }
    if let Err(e) = state.blob_store.delete(&digest).await {
        return OciError::from(e).into_response();
    }
    (StatusCode::ACCEPTED, HeaderMap::new()).into_response()
}
