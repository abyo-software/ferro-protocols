// SPDX-License-Identifier: Apache-2.0
//! Axum router factory that wires the `/v2/**` OCI endpoints.
//!
//! Spec: OCI Distribution Spec v1.1 §3 "API".
//!
//! The router is stateful — it takes an [`AppState`] carrying the blob
//! store and the registry-metadata plane. Callers build the state once
//! at boot and then call [`router`] to obtain an `axum::Router` they
//! can mount under `/`.

use std::sync::Arc;

use axum::Router;
use axum::routing::{delete, get, post};

use crate::handlers::{base, catalog};
use crate::registry::RegistryMeta;
use ferro_blob_store::SharedBlobStore;

/// Shared HTTP handler state.
pub struct AppState {
    /// Blob-bytes plane.
    pub blob_store: SharedBlobStore,
    /// Metadata plane (manifests, tags, upload sessions, referrers).
    pub registry: Arc<dyn RegistryMeta>,
}

/// Build the Axum router for every `/v2/**` OCI endpoint.
pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        // Version / auth challenge.
        .route("/v2/", get(base::version_check))
        .route("/v2", get(base::version_check))
        // Catalog and tag listing.
        .route("/v2/_catalog", get(catalog::list_catalog))
        .route("/v2/{*rest}", get(dispatch::dispatch_get))
        .route("/v2/{*rest}", axum::routing::head(dispatch::dispatch_head))
        .route("/v2/{*rest}", delete(dispatch::dispatch_delete))
        // Blob uploads -- POST / PATCH / PUT.
        .route(
            "/v2/{*rest}",
            post(dispatch::dispatch_post)
                .patch(dispatch::dispatch_patch_inner)
                .put(dispatch::dispatch_put_inner),
        )
        .with_state(state)
}

/// Small axum-aware dispatch layer.
///
/// Distribution routes use the `{name}` path parameter which can contain
/// slashes. Axum's `{*rest}` wildcard allows us to greedily capture the
/// path tail and then inspect the suffix ourselves. We then dispatch to
/// the real handler based on the suffix shape — `blobs/{digest}`,
/// `blobs/uploads/{uuid?}`, `manifests/{reference}`, `tags/list`, or
/// `referrers/{digest}`.
pub mod dispatch {
    use std::sync::Arc;

    use axum::body::Bytes;
    use axum::extract::{Path, Query, State};
    use axum::http::{HeaderMap, Method, StatusCode};
    use axum::response::{IntoResponse, Response};

    use super::AppState;
    use crate::error::{OciError, OciErrorCode};
    use crate::handlers::{blob, blob_upload, manifest as manifest_h, referrers, tags};

    /// Split `rest` into `(name, suffix)` where `suffix` is one of
    /// `blobs/...`, `manifests/...`, `tags/list`, `referrers/...`.
    fn split_rest(rest: &str) -> Option<(&str, &str)> {
        // Walk the path and find the last segment boundary where the
        // suffix starts with a known keyword. This accommodates
        // multi-level names like `my-org/library/alpine`.
        let keywords = ["blobs/", "manifests/", "tags/list", "referrers/"];
        for kw in keywords {
            if let Some(idx) = rest.rfind(kw) {
                // Ensure the match is preceded by a `/` or is at idx 0
                // after the name prefix.
                if idx == 0 {
                    return None;
                }
                if &rest[idx - 1..idx] != "/" {
                    continue;
                }
                let name = &rest[..idx - 1];
                let suffix = &rest[idx..];
                return Some((name, suffix));
            }
        }
        None
    }

    /// Decode the rest into (name, suffix) or return NAME_INVALID.
    fn decode(rest: &str) -> Result<(String, String), OciError> {
        let (name, suffix) = split_rest(rest).ok_or_else(|| {
            OciError::new(OciErrorCode::NameUnknown, format!("cannot route `{rest}`"))
        })?;
        Ok((name.to_owned(), suffix.to_owned()))
    }

    /// GET dispatcher.
    pub async fn dispatch_get(
        State(state): State<Arc<AppState>>,
        Path(rest): Path<String>,
        Query(params): Query<std::collections::BTreeMap<String, String>>,
        headers: HeaderMap,
    ) -> Response {
        let (name, suffix) = match decode(&rest) {
            Ok(v) => v,
            Err(e) => return e.into_response(),
        };
        dispatch_inner(
            state,
            name,
            suffix,
            Method::GET,
            headers,
            params,
            Bytes::new(),
        )
        .await
    }

    /// HEAD dispatcher.
    pub async fn dispatch_head(
        State(state): State<Arc<AppState>>,
        Path(rest): Path<String>,
        headers: HeaderMap,
    ) -> Response {
        let (name, suffix) = match decode(&rest) {
            Ok(v) => v,
            Err(e) => return e.into_response(),
        };
        dispatch_inner(
            state,
            name,
            suffix,
            Method::HEAD,
            headers,
            std::collections::BTreeMap::default(),
            Bytes::new(),
        )
        .await
    }

    /// DELETE dispatcher.
    pub async fn dispatch_delete(
        State(state): State<Arc<AppState>>,
        Path(rest): Path<String>,
        headers: HeaderMap,
    ) -> Response {
        let (name, suffix) = match decode(&rest) {
            Ok(v) => v,
            Err(e) => return e.into_response(),
        };
        dispatch_inner(
            state,
            name,
            suffix,
            Method::DELETE,
            headers,
            std::collections::BTreeMap::default(),
            Bytes::new(),
        )
        .await
    }

    /// POST dispatcher (blob upload init).
    pub async fn dispatch_post(
        State(state): State<Arc<AppState>>,
        Path(rest): Path<String>,
        Query(params): Query<std::collections::BTreeMap<String, String>>,
        headers: HeaderMap,
        body: Bytes,
    ) -> Response {
        let (name, suffix) = match decode(&rest) {
            Ok(v) => v,
            Err(e) => return e.into_response(),
        };
        dispatch_inner(state, name, suffix, Method::POST, headers, params, body).await
    }

    /// PATCH dispatcher.
    pub async fn dispatch_patch_inner(
        State(state): State<Arc<AppState>>,
        Path(rest): Path<String>,
        headers: HeaderMap,
        body: Bytes,
    ) -> Response {
        let (name, suffix) = match decode(&rest) {
            Ok(v) => v,
            Err(e) => return e.into_response(),
        };
        dispatch_inner(
            state,
            name,
            suffix,
            Method::PATCH,
            headers,
            std::collections::BTreeMap::default(),
            body,
        )
        .await
    }

    /// PUT dispatcher.
    pub async fn dispatch_put_inner(
        State(state): State<Arc<AppState>>,
        Path(rest): Path<String>,
        Query(params): Query<std::collections::BTreeMap<String, String>>,
        headers: HeaderMap,
        body: Bytes,
    ) -> Response {
        let (name, suffix) = match decode(&rest) {
            Ok(v) => v,
            Err(e) => return e.into_response(),
        };
        dispatch_inner(state, name, suffix, Method::PUT, headers, params, body).await
    }

    #[allow(clippy::too_many_arguments)]
    async fn dispatch_inner(
        state: Arc<AppState>,
        name: String,
        suffix: String,
        method: Method,
        headers: HeaderMap,
        params: std::collections::BTreeMap<String, String>,
        body: Bytes,
    ) -> Response {
        // Tag listing.
        if suffix == "tags/list" {
            return if method == Method::GET {
                tags::list_tags(&state, &name, &params)
                    .await
                    .into_response()
            } else {
                OciError::new(OciErrorCode::Unsupported, "unsupported method")
                    .with_status(StatusCode::METHOD_NOT_ALLOWED)
                    .into_response()
            };
        }
        // Referrers.
        if let Some(rest) = suffix.strip_prefix("referrers/") {
            return if method == Method::GET {
                referrers::get_referrers(&state, &name, rest, &params)
                    .await
                    .into_response()
            } else {
                OciError::new(OciErrorCode::Unsupported, "unsupported method")
                    .with_status(StatusCode::METHOD_NOT_ALLOWED)
                    .into_response()
            };
        }
        // Manifests.
        if let Some(rest) = suffix.strip_prefix("manifests/") {
            return match method {
                Method::GET => manifest_h::get_manifest(&state, &name, rest, &headers)
                    .await
                    .into_response(),
                Method::HEAD => manifest_h::head_manifest(&state, &name, rest)
                    .await
                    .into_response(),
                Method::PUT => manifest_h::put_manifest(&state, &name, rest, &headers, body)
                    .await
                    .into_response(),
                Method::DELETE => manifest_h::delete_manifest(&state, &name, rest)
                    .await
                    .into_response(),
                _ => OciError::new(OciErrorCode::Unsupported, "unsupported method")
                    .with_status(StatusCode::METHOD_NOT_ALLOWED)
                    .into_response(),
            };
        }
        // Blob uploads.
        if let Some(rest) = suffix.strip_prefix("blobs/uploads/") {
            let uuid = rest.trim_end_matches('/');
            return match method {
                Method::POST => {
                    // `rest` is "" for the "/blobs/uploads/" endpoint.
                    blob_upload::init_upload(&state, &name, &headers, &params, body)
                        .await
                        .into_response()
                }
                Method::PATCH => blob_upload::patch_upload(&state, &name, uuid, &headers, body)
                    .await
                    .into_response(),
                Method::PUT => blob_upload::finish_upload(&state, &name, uuid, &params, body)
                    .await
                    .into_response(),
                Method::GET => blob_upload::get_upload_status(&state, &name, uuid)
                    .await
                    .into_response(),
                Method::DELETE => blob_upload::cancel_upload(&state, &name, uuid)
                    .await
                    .into_response(),
                _ => OciError::new(OciErrorCode::Unsupported, "unsupported method")
                    .with_status(StatusCode::METHOD_NOT_ALLOWED)
                    .into_response(),
            };
        }
        // Blobs (by digest).
        if let Some(rest) = suffix.strip_prefix("blobs/") {
            return match method {
                Method::GET => blob::get_blob(&state, &name, rest).await.into_response(),
                Method::HEAD => blob::head_blob(&state, &name, rest).await.into_response(),
                Method::DELETE => blob::delete_blob(&state, &name, rest).await.into_response(),
                _ => OciError::new(OciErrorCode::Unsupported, "unsupported method")
                    .with_status(StatusCode::METHOD_NOT_ALLOWED)
                    .into_response(),
            };
        }
        OciError::new(
            OciErrorCode::NameUnknown,
            format!("cannot route `{name}/{suffix}`"),
        )
        .into_response()
    }

    #[cfg(test)]
    mod tests {
        use super::split_rest;

        #[test]
        fn split_simple_manifest_path() {
            let (name, suffix) = split_rest("alpine/manifests/latest").expect("split");
            assert_eq!(name, "alpine");
            assert_eq!(suffix, "manifests/latest");
        }

        #[test]
        fn split_nested_blob_path() {
            let (name, suffix) = split_rest("my-org/lib/alpine/blobs/uploads/abc").expect("split");
            assert_eq!(name, "my-org/lib/alpine");
            assert_eq!(suffix, "blobs/uploads/abc");
        }

        #[test]
        fn split_tags_list() {
            let (name, suffix) = split_rest("lib/alpine/tags/list").expect("split");
            assert_eq!(name, "lib/alpine");
            assert_eq!(suffix, "tags/list");
        }

        #[test]
        fn split_referrers() {
            let (name, suffix) = split_rest("lib/alpine/referrers/sha256:abcd").expect("split");
            assert_eq!(name, "lib/alpine");
            assert_eq!(suffix, "referrers/sha256:abcd");
        }

        #[test]
        fn split_none_for_bare_name() {
            assert!(split_rest("alpine").is_none());
        }
    }
}
