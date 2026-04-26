// SPDX-License-Identifier: Apache-2.0
//! `GET /v2/` version-check endpoint.
//!
//! Spec: OCI Distribution Spec v1.1 §3.2 "Determining support".
//!
//! Clients call this endpoint first to confirm the registry implements
//! the v2 API. Unauthenticated installs return `200 OK` with an empty
//! JSON object; authenticated installs may return `401` with a
//! `WWW-Authenticate` challenge, but that layer is owned by
//! a separate auth crate and is not implemented here in Phase 1.

use axum::Json;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde_json::json;

/// Handle `GET /v2/`.
///
/// Spec: OCI Distribution Spec v1.1 §3.2.
pub async fn version_check() -> Response {
    let mut headers = HeaderMap::new();
    headers.insert(
        "Docker-Distribution-API-Version",
        HeaderValue::from_static("registry/2.0"),
    );
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    (StatusCode::OK, headers, Json(json!({}))).into_response()
}
