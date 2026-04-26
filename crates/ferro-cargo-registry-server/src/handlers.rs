// SPDX-License-Identifier: Apache-2.0
//! Axum handlers for the Cargo registry.
//!
//! Spec: `doc.rust-lang.org/cargo/reference/registry-web-api.html`.

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path as AxumPath, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use ferro_blob_store::Digest;
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use tracing::debug;

use crate::error::CargoError;
use crate::index::{entry_from_manifest, render_lines};
use crate::name::{index_path, validate_name};
use crate::owners::{Owner, OwnersMutationResponse, OwnersRequest, OwnersResponse};
use crate::publish;
use crate::router::{CargoState, CrateRecord};
use crate::version::is_valid_semver;
use crate::yank::YankResponse;

/// `GET /config.json` — sparse-index configuration.
pub async fn handle_config_json(State(state): State<CargoState>) -> Response {
    (StatusCode::OK, Json(&*state.config)).into_response()
}

/// `GET /index/{*path}` — sparse-index line files.
///
/// DD R2 F-R2-021: honours `If-None-Match` by computing a strong ETag
/// equal to the quoted SHA-256 of the rendered index body. When the
/// request carries a matching `If-None-Match` header the handler returns
/// `304 Not Modified` with the ETag but no body, which is what Cargo's
/// sparse-index client expects for bandwidth-efficient polling.
pub async fn handle_sparse_index(
    State(state): State<CargoState>,
    AxumPath(path): AxumPath<String>,
    headers: HeaderMap,
) -> Result<Response, CargoError> {
    let name =
        extract_name_from_index_path(&path).ok_or_else(|| CargoError::NotFound(path.clone()))?;
    let crates = state.crates.read().await;
    let record = crates
        .get(name)
        .ok_or_else(|| CargoError::NotFound(name.to_owned()))?;
    let body = render_lines(&record.entries);
    let etag = sparse_index_etag(&body);
    let if_none_match = headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let mut h = HeaderMap::new();
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    if let Ok(v) = HeaderValue::from_str(&etag) {
        h.insert(header::ETAG, v);
    }
    if etag_matches(if_none_match, &etag) {
        return Ok((StatusCode::NOT_MODIFIED, h).into_response());
    }
    Ok((StatusCode::OK, h, body).into_response())
}

/// Compute the sparse-index ETag for `body`.
///
/// Returned as a quoted hex string (strong validator per RFC 9110
/// §8.8.3).
#[must_use]
pub fn sparse_index_etag(body: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(body.as_bytes());
    let digest = hasher.finalize();
    format!("\"{}\"", hex::encode(digest))
}

/// Compare an `If-None-Match` header value against the computed ETag.
///
/// Accepts:
/// - Exact match of the quoted value.
/// - `*` wildcard (RFC 9110 §13.1.2).
/// - Comma-separated list of values.
fn etag_matches(if_none_match: &str, etag: &str) -> bool {
    let raw = if_none_match.trim();
    if raw.is_empty() {
        return false;
    }
    if raw == "*" {
        return true;
    }
    for candidate in raw.split(',') {
        let candidate = candidate.trim();
        // Strip a leading `W/` weak-validator prefix — we issue strong
        // validators, and RFC 9110 §13.1.2 says either MAY be compared
        // with a weak comparison for If-None-Match.
        let candidate = candidate.strip_prefix("W/").unwrap_or(candidate);
        if candidate == etag {
            return true;
        }
    }
    false
}

fn extract_name_from_index_path(path: &str) -> Option<&str> {
    // Layout:
    // - `1/{name}` → name
    // - `2/{name}` → name
    // - `3/{first}/{name}` → name
    // - `{ab}/{cd}/{name}` → name
    let mut it = path.rsplitn(2, '/');
    let name = it.next()?;
    if name.is_empty() {
        return None;
    }
    Some(name)
}

/// `GET /index.git/{*path}` — Phase 2 stub.
pub async fn handle_git_index_stub(
    AxumPath(_path): AxumPath<String>,
) -> Result<Response, CargoError> {
    Err(CargoError::NotImplemented(
        "git index is Phase 2; set `protocol = \"sparse\"` on the client".into(),
    ))
}

/// `PUT /api/v1/crates/new` — publish.
pub async fn handle_publish(
    State(state): State<CargoState>,
    body: Bytes,
) -> Result<Response, CargoError> {
    let req = publish::parse(&body)?;
    let manifest = req.manifest;
    let tarball = req.tarball;

    let name = manifest
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| CargoError::InvalidPublish("manifest missing `name`".into()))?;
    validate_name(name)?;

    let vers = manifest
        .get("vers")
        .or_else(|| manifest.get("version"))
        .and_then(Value::as_str)
        .ok_or_else(|| CargoError::InvalidPublish("manifest missing `vers`".into()))?;
    if !is_valid_semver(vers) {
        return Err(CargoError::InvalidVersion(vers.to_owned()));
    }

    // Checksum: SHA-256 of the tarball body.
    let mut hasher = Sha256::new();
    hasher.update(&tarball);
    let computed = hex::encode(hasher.finalize());
    if let Some(declared) = manifest.get("cksum").and_then(Value::as_str)
        && !declared.is_empty()
        && declared != computed
    {
        return Err(CargoError::ChecksumMismatch {
            declared: declared.to_owned(),
            computed,
        });
    }

    // Ingest.
    let digest = Digest::sha256_of(&tarball);
    state.blobs.put(&digest, tarball.clone()).await?;

    let entry = entry_from_manifest(&manifest, computed)
        .map_err(|e| CargoError::InvalidPublish(format!("manifest coerce: {e}")))?;

    let mut crates = state.crates.write().await;
    let record = crates
        .entry(name.to_owned())
        .or_insert_with(CrateRecord::default);
    record.tarballs.insert(entry.vers.clone(), digest.clone());
    // Append or replace by (name, vers).
    if let Some(existing) = record.entries.iter_mut().find(|e| e.vers == entry.vers) {
        *existing = entry;
    } else {
        record.entries.push(entry);
    }

    debug!(crate_name = %name, version = %vers, "publish complete");
    Ok((
        StatusCode::OK,
        Json(json!({
            "warnings": {
                "invalid_categories": [],
                "invalid_badges": [],
                "other": []
            }
        })),
    )
        .into_response())
}

/// `GET /api/v1/crates/{name}/{version}/download`.
pub async fn handle_download(
    State(state): State<CargoState>,
    AxumPath((name, version)): AxumPath<(String, String)>,
) -> Result<Response, CargoError> {
    validate_name(&name)?;
    let crates = state.crates.read().await;
    let record = crates
        .get(&name)
        .ok_or_else(|| CargoError::NotFound(name.clone()))?;
    let digest = record
        .tarballs
        .get(&version)
        .ok_or_else(|| CargoError::NotFound(format!("{name} {version}")))?
        .clone();
    let bytes = state.blobs.get(&digest).await?;
    let mut h = HeaderMap::new();
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/x-tar"),
    );
    let etag = HeaderValue::from_str(&format!("\"{}\"", digest.hex()))
        .unwrap_or_else(|_| HeaderValue::from_static("\"\""));
    h.insert(header::ETAG, etag);
    if let Ok(v) = HeaderValue::from_str(&bytes.len().to_string()) {
        h.insert(header::CONTENT_LENGTH, v);
    }
    Ok((StatusCode::OK, h, bytes).into_response())
}

/// `DELETE /api/v1/crates/{name}/{version}/yank`.
pub async fn handle_yank(
    State(state): State<CargoState>,
    AxumPath((name, version)): AxumPath<(String, String)>,
) -> Result<Response, CargoError> {
    set_yanked(&state, &name, &version, true).await?;
    Ok((StatusCode::OK, Json(YankResponse::ok())).into_response())
}

/// `PUT /api/v1/crates/{name}/{version}/unyank`.
pub async fn handle_unyank(
    State(state): State<CargoState>,
    AxumPath((name, version)): AxumPath<(String, String)>,
) -> Result<Response, CargoError> {
    set_yanked(&state, &name, &version, false).await?;
    Ok((StatusCode::OK, Json(YankResponse::ok())).into_response())
}

async fn set_yanked(
    state: &CargoState,
    name: &str,
    version: &str,
    yanked: bool,
) -> Result<(), CargoError> {
    validate_name(name)?;
    let mut crates = state.crates.write().await;
    let record = crates
        .get_mut(name)
        .ok_or_else(|| CargoError::NotFound(name.to_owned()))?;
    let entry = record
        .entries
        .iter_mut()
        .find(|e| e.vers == version)
        .ok_or_else(|| CargoError::NotFound(format!("{name} {version}")))?;
    entry.yanked = yanked;
    Ok(())
}

/// `GET /api/v1/crates/{name}/owners`.
pub async fn handle_owners_list(
    State(state): State<CargoState>,
    AxumPath(name): AxumPath<String>,
) -> Result<Response, CargoError> {
    validate_name(&name)?;
    let crates = state.crates.read().await;
    let record = crates
        .get(&name)
        .ok_or_else(|| CargoError::NotFound(name.clone()))?;
    Ok((
        StatusCode::OK,
        Json(OwnersResponse {
            users: record.owners.clone(),
        }),
    )
        .into_response())
}

/// `PUT /api/v1/crates/{name}/owners` — add owners.
pub async fn handle_owners_add(
    State(state): State<CargoState>,
    AxumPath(name): AxumPath<String>,
    Json(req): Json<OwnersRequest>,
) -> Result<Response, CargoError> {
    mutate_owners(&state, &name, &req.users, false).await?;
    Ok((
        StatusCode::OK,
        Json(OwnersMutationResponse {
            ok: true,
            msg: Some("owners updated".into()),
        }),
    )
        .into_response())
}

/// `DELETE /api/v1/crates/{name}/owners` — remove owners.
pub async fn handle_owners_delete(
    State(state): State<CargoState>,
    AxumPath(name): AxumPath<String>,
    Json(req): Json<OwnersRequest>,
) -> Result<Response, CargoError> {
    mutate_owners(&state, &name, &req.users, true).await?;
    Ok((
        StatusCode::OK,
        Json(OwnersMutationResponse {
            ok: true,
            msg: None,
        }),
    )
        .into_response())
}

async fn mutate_owners(
    state: &CargoState,
    name: &str,
    logins: &[String],
    remove: bool,
) -> Result<(), CargoError> {
    validate_name(name)?;
    let mut crates = state.crates.write().await;
    let record = crates
        .get_mut(name)
        .ok_or_else(|| CargoError::NotFound(name.to_owned()))?;
    if remove {
        record
            .owners
            .retain(|o| !logins.iter().any(|l| l == &o.login));
    } else {
        // Assign a deterministic id based on existing count.
        let mut next_id = record.owners.iter().map(|o| o.id).max().unwrap_or(0) + 1;
        for login in logins {
            if record.owners.iter().any(|o| &o.login == login) {
                continue;
            }
            record.owners.push(Owner {
                id: next_id,
                login: login.clone(),
                name: None,
            });
            next_id += 1;
        }
    }
    Ok(())
}

/// Exposed for tests that want to assert the sparse-index path builder
/// matches the handler's own dispatch.
#[doc(hidden)]
#[must_use]
pub fn derive_index_path(name: &str) -> String {
    index_path(name)
}
