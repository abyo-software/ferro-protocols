// SPDX-License-Identifier: Apache-2.0
//! `GET /v2/{name}/tags/list` — tag listing.
//!
//! Spec: OCI Distribution Spec v1.1 §3.6 "Listing tags".
//!
//! Supports pagination via `n` and `last`.

use std::collections::BTreeMap;

use axum::Json;
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};

use crate::error::OciError;
use crate::reference::validate_name;
use crate::router::AppState;

/// Response body for `GET /v2/{name}/tags/list`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TagListResponse {
    /// Repository name the tags belong to.
    pub name: String,
    /// Tag names.
    pub tags: Vec<String>,
}

/// Handle `GET /v2/{name}/tags/list?n=<n>&last=<last>`.
///
/// Spec: OCI Distribution Spec v1.1 §3.6.
pub async fn list_tags(
    state: &AppState,
    name: &str,
    params: &BTreeMap<String, String>,
) -> Response {
    if let Err(e) = validate_name(name) {
        return e.into_response();
    }
    let n = params.get("n").and_then(|s| s.parse::<usize>().ok());
    let last = params.get("last").map(String::as_str);
    let tags = match state.registry.list_tags(name, last, n).await {
        Ok(v) => v,
        Err(e) => return OciError::from(e).into_response(),
    };

    let mut headers = HeaderMap::new();
    if let (Some(limit), Some(last_tag)) = (n, tags.last())
        && tags.len() == limit
    {
        let link = format!("</v2/{name}/tags/list?last={last_tag}&n={limit}>; rel=\"next\"");
        if let Ok(v) = HeaderValue::from_str(&link) {
            headers.insert(axum::http::header::LINK, v);
        }
    }

    (
        StatusCode::OK,
        headers,
        Json(TagListResponse {
            name: name.to_owned(),
            tags,
        }),
    )
        .into_response()
}
