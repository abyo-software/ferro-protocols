// SPDX-License-Identifier: Apache-2.0
//! `GET /v2/_catalog` — repository catalog.
//!
//! Spec: OCI Distribution Spec v1.1 §3.5 "Listing repositories".
//!
//! Supports pagination via `n` (max results) and `last` (cursor).

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::Json;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};

use crate::router::AppState;

/// Response body for `GET /v2/_catalog`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogResponse {
    /// Repository names.
    pub repositories: Vec<String>,
}

/// Handle `GET /v2/_catalog?n=<n>&last=<last>`.
///
/// Spec: OCI Distribution Spec v1.1 §3.5.
pub async fn list_catalog(
    State(state): State<Arc<AppState>>,
    Query(params): Query<BTreeMap<String, String>>,
) -> Response {
    let n = params.get("n").and_then(|s| s.parse::<usize>().ok());
    let last = params.get("last").map(String::as_str);
    let repos = match state.registry.list_repositories(last, n).await {
        Ok(v) => v,
        Err(e) => {
            return crate::error::OciError::from(e).into_response();
        }
    };
    let mut headers = HeaderMap::new();
    if let (Some(limit), Some(last_entry)) = (n, repos.last())
        && repos.len() == limit
    {
        let link = format!("</v2/_catalog?last={last_entry}&n={limit}>; rel=\"next\"");
        if let Ok(v) = HeaderValue::from_str(&link) {
            headers.insert(axum::http::header::LINK, v);
        }
    }
    (
        StatusCode::OK,
        headers,
        Json(CatalogResponse {
            repositories: repos,
        }),
    )
        .into_response()
}
