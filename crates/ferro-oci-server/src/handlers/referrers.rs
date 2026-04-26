// SPDX-License-Identifier: Apache-2.0
//! Referrers API.
//!
//! Spec: OCI Distribution Spec v1.1 §3.3 "Content Discovery".
//!
//! - `GET /v2/{name}/referrers/{digest}?artifactType=<type>`;
//! - Response is an OCI image index whose `manifests` array contains
//!   one descriptor per referrer;
//! - When `artifactType` is supplied and filtering applied, the
//!   response MUST include the `OCI-Filters-Applied: artifactType`
//!   header so the client knows the server honoured the filter.

use std::collections::BTreeMap;

use axum::Json;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use ferro_blob_store::Digest;
use serde_json::json;

use crate::error::{OciError, OciErrorCode};
use crate::media_types::OCI_IMAGE_INDEX;
use crate::reference::validate_name;
use crate::router::AppState;

/// Handle `GET /v2/{name}/referrers/{digest}`.
///
/// Spec: OCI Distribution Spec v1.1 §3.3.
pub async fn get_referrers(
    state: &AppState,
    name: &str,
    digest_str: &str,
    params: &BTreeMap<String, String>,
) -> Response {
    if let Err(e) = validate_name(name) {
        return e.into_response();
    }
    let digest = match digest_str.parse::<Digest>() {
        Ok(d) => d,
        Err(e) => {
            return OciError::new(
                OciErrorCode::DigestInvalid,
                format!("invalid digest `{digest_str}`: {e}"),
            )
            .into_response();
        }
    };

    let artifact_type = params.get("artifactType").map(String::as_str);
    let referrers = match state
        .registry
        .list_referrers(name, &digest, artifact_type)
        .await
    {
        Ok(v) => v,
        Err(e) => return OciError::from(e).into_response(),
    };

    let body = json!({
        "schemaVersion": 2,
        "mediaType": OCI_IMAGE_INDEX,
        "manifests": referrers,
    });

    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(OCI_IMAGE_INDEX),
    );
    if artifact_type.is_some() {
        headers.insert(
            "OCI-Filters-Applied",
            HeaderValue::from_static("artifactType"),
        );
    }
    (StatusCode::OK, headers, Json(body)).into_response()
}
