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
use crate::name::{canonical_name, index_path, validate_name};
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
/// DD R2 F-R2-021: honours `If-None-Match` by computing a strong `ETag`
/// equal to the quoted SHA-256 of the rendered index body. When the
/// request carries a matching `If-None-Match` header the handler returns
/// `304 Not Modified` with the `ETag` but no body, which is what Cargo's
/// sparse-index client expects for bandwidth-efficient polling.
pub async fn handle_sparse_index(
    State(state): State<CargoState>,
    AxumPath(path): AxumPath<String>,
    headers: HeaderMap,
) -> Result<Response, CargoError> {
    let name =
        extract_name_from_index_path(&path).ok_or_else(|| CargoError::NotFound(path.clone()))?;
    serve_index(&state, name, &headers).await
}

/// Root-level sparse-index handler for the two-segment layout.
///
/// Cargo configured with `index = "sparse+http://host/"` (the index
/// base equal to the server root) fetches line files at the bare
/// canonical layout — `1/{name}` and `2/{name}` for short names —
/// without an `/index/` prefix. This handler serves those
/// root-relative paths so a stock `cargo publish` / `cargo fetch`
/// round-trip works without rewriting the index URL. The trailing
/// path segment is the canonical crate name.
pub async fn handle_sparse_index_root2(
    State(state): State<CargoState>,
    AxumPath((_prefix, name)): AxumPath<(String, String)>,
    headers: HeaderMap,
) -> Result<Response, CargoError> {
    serve_index(&state, &name, &headers).await
}

/// Root-level sparse-index handler for the three-segment layout.
///
/// Serves `3/{c}/{name}` and `{ab}/{cd}/{name}` shapes (names of three
/// or more characters) when the index base is the server root. See
/// [`handle_sparse_index_root2`] for the two-segment counterpart.
pub async fn handle_sparse_index_root3(
    State(state): State<CargoState>,
    AxumPath((_p0, _p1, name)): AxumPath<(String, String, String)>,
    headers: HeaderMap,
) -> Result<Response, CargoError> {
    serve_index(&state, &name, &headers).await
}

/// Shared body for the prefixed and root sparse-index handlers.
async fn serve_index(
    state: &CargoState,
    name: &str,
    headers: &HeaderMap,
) -> Result<Response, CargoError> {
    let crates = state.crates.read().await;
    let record = crates
        .get(&canonical_name(name))
        .ok_or_else(|| CargoError::NotFound(name.to_owned()))?;
    let body = render_lines(&record.entries);
    drop(crates);
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

/// Compute the sparse-index `ETag` for `body`.
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

/// Compare an `If-None-Match` header value against the computed `ETag`.
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

    // Coerce the manifest into a sparse-index entry up front — this is
    // cheap validation and must precede any blob write (R2-8).
    let entry = entry_from_manifest(&manifest, computed)
        .map_err(|e| CargoError::InvalidPublish(format!("manifest coerce: {e}")))?;

    // Records are keyed by the canonical name (lowercased, `-`/`_`
    // folded) so a mixed-case publish (`MyCrate`) is retrievable at the
    // lowercase sparse-index path cargo requests. The display case lives
    // in the IndexEntry `name`.
    let key = canonical_name(name);
    let digest = Digest::sha256_of(&tarball);

    // Hold the write lock across the whole ingest so collision and
    // duplicate-version checks plus the blob write are serialized — this
    // closes the TOCTOU window where two concurrent publishes of the same
    // `(name, vers)` could both pass the check.
    let mut crates = state.crates.write().await;

    // R2-8: validate BEFORE writing the tarball blob, so a rejected
    // publish never leaves an orphan blob on disk.
    if let Some(existing) = crates.get(&key) {
        // Reject a publish that collides with a *different* existing crate
        // under cargo's case-insensitive / `-`-vs-`_` uniqueness rules.
        if let Some(existing_display) = existing.entries.first().map(|e| e.name.as_str())
            && existing_display != name
        {
            let existing_owned = existing_display.to_owned();
            drop(crates);
            return Err(CargoError::NameConflict {
                requested: name.to_owned(),
                existing: existing_owned,
            });
        }
        // R2-5: published versions are immutable. Reject a re-publish of
        // an already-present `(name, vers)` with 409 Conflict; only yank /
        // unyank may mutate an existing index line.
        if existing.entries.iter().any(|e| e.vers == entry.vers) {
            drop(crates);
            return Err(CargoError::DuplicateVersion {
                name: name.to_owned(),
                version: vers.to_owned(),
            });
        }
    }

    // Validation passed — now persist the tarball blob and append the
    // index entry. The blob put happens under the lock so the on-disk
    // state and the in-memory index advance together.
    state.blobs.put(&digest, tarball.clone()).await?;
    let vers_key = entry.vers.clone();
    let record = crates.entry(key.clone()).or_insert_with(CrateRecord::default);
    record.tarballs.insert(vers_key.clone(), digest.clone());
    record.entries.push(entry);

    // R2-6 / R3-2: write the index map through to the durable snapshot
    // while the lock is held so on-disk state matches memory. If the
    // snapshot cannot be written we must NOT acknowledge the publish:
    // roll back the in-memory entry and delete the just-written blob (if
    // no surviving version still references it), then return 500 so the
    // client knows the crate was not durably stored.
    if let Err(err) = state.persist_locked(&crates) {
        rollback_publish(&mut crates, &key, &vers_key, &digest);
        drop(crates);
        // Best-effort orphan-blob cleanup. A content-addressed blob may be
        // shared with another version (identical bytes); only delete it
        // when no remaining mapping references this digest.
        if !digest_still_referenced(&state, &digest).await
            && let Err(del_err) = state.blobs.delete(&digest).await
        {
            tracing::error!(%del_err, "failed to delete orphan blob after persist rollback");
        }
        return Err(CargoError::Persistence(err.to_string()));
    }
    drop(crates);

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

/// Undo the in-memory effect of a publish whose durable snapshot write
/// failed (DD R3-2): remove the version's tarball mapping and the index
/// entry just appended for `vers`, and drop the crate record entirely if
/// that leaves it empty (the publish that created it).
///
/// The caller still holds the write guard, so this leaves the in-memory
/// map exactly as it was before the failed publish.
fn rollback_publish(
    crates: &mut std::collections::BTreeMap<String, CrateRecord>,
    key: &str,
    vers: &str,
    digest: &Digest,
) {
    if let Some(record) = crates.get_mut(key) {
        // Only un-map if the mapping still points at the blob we wrote —
        // never clobber a pre-existing version of the same string (there
        // can be only one per `vers`, but stay defensive).
        if record.tarballs.get(vers) == Some(digest) {
            record.tarballs.remove(vers);
        }
        record.entries.retain(|e| e.vers != vers);
        if record.entries.is_empty() && record.tarballs.is_empty() && record.owners.is_empty() {
            crates.remove(key);
        }
    }
}

/// Whether any crate still maps a version to `digest`.
///
/// Used after a publish rollback to decide if the just-written tarball
/// blob is now an orphan (safe to delete) or is shared with another,
/// surviving version (must be kept). Takes the read lock briefly.
async fn digest_still_referenced(state: &CargoState, digest: &Digest) -> bool {
    let crates = state.crates.read().await;
    crates
        .values()
        .any(|rec| rec.tarballs.values().any(|d| d == digest))
}

/// `GET /api/v1/crates/{name}/{version}/download`.
pub async fn handle_download(
    State(state): State<CargoState>,
    AxumPath((name, version)): AxumPath<(String, String)>,
) -> Result<Response, CargoError> {
    validate_name(&name)?;
    let crates = state.crates.read().await;
    let record = crates
        .get(&canonical_name(&name))
        .ok_or_else(|| CargoError::NotFound(name.clone()))?;
    let digest = record
        .tarballs
        .get(&version)
        .ok_or_else(|| CargoError::NotFound(format!("{name} {version}")))?
        .clone();
    drop(crates);
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
        .get_mut(&canonical_name(name))
        .ok_or_else(|| CargoError::NotFound(name.to_owned()))?;
    let entry = record
        .entries
        .iter_mut()
        .find(|e| e.vers == version)
        .ok_or_else(|| CargoError::NotFound(format!("{name} {version}")))?;
    let previous = entry.yanked;
    entry.yanked = yanked;
    // R2-6 / R3-2: yank / unyank mutate an existing index line; mirror it.
    // On a durable-write failure, restore the prior flag and fail the
    // request rather than acknowledging a change that was not persisted.
    if let Err(err) = state.persist_locked(&crates) {
        if let Some(record) = crates.get_mut(&canonical_name(name))
            && let Some(entry) = record.entries.iter_mut().find(|e| e.vers == version)
        {
            entry.yanked = previous;
        }
        drop(crates);
        return Err(CargoError::Persistence(err.to_string()));
    }
    drop(crates);
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
        .get(&canonical_name(&name))
        .ok_or_else(|| CargoError::NotFound(name.clone()))?;
    let users = record.owners.clone();
    drop(crates);
    Ok((StatusCode::OK, Json(OwnersResponse { users })).into_response())
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
        .get_mut(&canonical_name(name))
        .ok_or_else(|| CargoError::NotFound(name.to_owned()))?;
    // Snapshot the prior owner list so a failed durable write can be
    // rolled back (R3-2).
    let previous_owners = record.owners.clone();
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
    // R2-6 / R3-2: owner changes are durable index state; mirror them. On
    // a durable-write failure, restore the prior owner list and fail the
    // request rather than acknowledging an un-persisted change.
    if let Err(err) = state.persist_locked(&crates) {
        if let Some(record) = crates.get_mut(&canonical_name(name)) {
            record.owners = previous_owners;
        }
        drop(crates);
        return Err(CargoError::Persistence(err.to_string()));
    }
    drop(crates);
    Ok(())
}

/// Exposed for tests that want to assert the sparse-index path builder
/// matches the handler's own dispatch.
#[doc(hidden)]
#[must_use]
pub fn derive_index_path(name: &str) -> String {
    index_path(name)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Arc;

    use ferro_blob_store::{Digest, FsBlobStore};
    use tokio::sync::RwLock;

    use super::{derive_index_path, mutate_owners, set_yanked};
    use crate::config::IndexConfig;
    use crate::index::IndexEntry;
    use crate::name::{canonical_name, index_path};
    use crate::owners::Owner;
    use crate::router::{CargoState, CrateRecord};

    /// `derive_index_path` is the thin re-export the handlers use; it must
    /// return the real layout, not an empty / placeholder string. This
    /// kills both the `String::new()` and the `"xyzzy".into()` return
    /// mutants by pinning the actual layout for several name lengths.
    #[test]
    fn derive_index_path_matches_index_path_layout() {
        for name in ["a", "ab", "abc", "serde", "MyCrate"] {
            assert_eq!(derive_index_path(name), index_path(name));
        }
        // Concrete expected values so a constant-return mutant cannot
        // survive even if `index_path` itself were also mutated.
        assert_eq!(derive_index_path("serde"), "se/rd/serde");
        assert_eq!(derive_index_path("a"), "1/a");
        assert!(!derive_index_path("serde").is_empty());
    }

    /// Build a `CargoState` that already holds `record` for `key` in
    /// memory, with a blob store under `tmp` and a **broken** persistence
    /// dir (a path under a regular file → `ENOTDIR` on save). This lets a
    /// yank / owner mutation reach `persist_locked`, fail the durable
    /// write, and exercise the R3-2 rollback path.
    fn state_with_seed_and_broken_persistence(
        tmp: &tempfile::TempDir,
        key: &str,
        record: CrateRecord,
    ) -> CargoState {
        let blob_dir = tmp.path().join("blobs");
        std::fs::create_dir_all(&blob_dir).unwrap();
        let blocker = tmp.path().join("blocker");
        std::fs::write(&blocker, b"file").unwrap();

        let mut map = BTreeMap::new();
        map.insert(key.to_owned(), record);

        CargoState {
            blobs: Arc::new(FsBlobStore::new(&blob_dir).unwrap()),
            crates: Arc::new(RwLock::new(map)),
            config: Arc::new(IndexConfig::new("http://localhost")),
            data_dir: Some(Arc::new(blocker.join("persist"))),
        }
    }

    fn seed_entry(vers: &str, yanked: bool) -> IndexEntry {
        IndexEntry {
            name: "foo".into(),
            vers: vers.into(),
            deps: vec![],
            cksum: Digest::sha256_of(b"seed").hex().to_owned(),
            features: BTreeMap::new(),
            yanked,
            links: None,
            v: Some(2),
            features2: None,
            rust_version: None,
        }
    }

    /// R3-2: a yank whose durable snapshot write fails must return a
    /// persistence error AND leave the in-memory `yanked` flag at its
    /// prior value (rolled back), not the attempted new value.
    #[tokio::test]
    async fn yank_rolls_back_in_memory_flag_when_persist_fails() {
        let tmp = tempfile::TempDir::new().unwrap();
        let record = CrateRecord {
            entries: vec![seed_entry("1.0.0", false)],
            tarballs: BTreeMap::new(),
            owners: vec![],
        };
        let state = state_with_seed_and_broken_persistence(&tmp, &canonical_name("foo"), record);

        let err = set_yanked(&state, "foo", "1.0.0", true)
            .await
            .expect_err("persist failure must surface as an error");
        assert_eq!(err.status(), axum::http::StatusCode::INTERNAL_SERVER_ERROR);

        // The flag must NOT have flipped — the change was not durable.
        let yanked = {
            let crates = state.crates.read().await;
            crates[&canonical_name("foo")].entries[0].yanked
        };
        assert!(!yanked, "failed yank must roll the yanked flag back to false");
    }

    /// R3-2: an owner mutation whose durable snapshot write fails must
    /// return a persistence error AND restore the prior owner list.
    #[tokio::test]
    async fn owner_add_rolls_back_when_persist_fails() {
        let tmp = tempfile::TempDir::new().unwrap();
        let record = CrateRecord {
            entries: vec![seed_entry("1.0.0", false)],
            tarballs: BTreeMap::new(),
            owners: vec![Owner {
                id: 1,
                login: "alice".into(),
                name: None,
            }],
        };
        let state = state_with_seed_and_broken_persistence(&tmp, &canonical_name("foo"), record);

        let err = mutate_owners(&state, "foo", &["bob".to_owned()], false)
            .await
            .expect_err("persist failure must surface as an error");
        assert_eq!(err.status(), axum::http::StatusCode::INTERNAL_SERVER_ERROR);

        // The owner list must be exactly the original — bob not added.
        let owners = {
            let crates = state.crates.read().await;
            crates[&canonical_name("foo")].owners.clone()
        };
        assert_eq!(owners.len(), 1, "failed add must roll the owner list back");
        assert_eq!(owners[0].login, "alice");
    }

    /// R3-2: with persistence **disabled** (`data_dir == None`, the
    /// in-memory / unit-test path) a yank still succeeds — the no-op
    /// `persist_locked` returns `Ok`, so behaviour is unchanged.
    #[tokio::test]
    async fn yank_succeeds_with_persistence_disabled() {
        let tmp = tempfile::TempDir::new().unwrap();
        let store = Arc::new(FsBlobStore::new(tmp.path()).unwrap());
        let state = CargoState::new(store, "http://localhost");
        {
            let mut crates = state.crates.write().await;
            crates.insert(
                canonical_name("foo"),
                CrateRecord {
                    entries: vec![seed_entry("1.0.0", false)],
                    tarballs: BTreeMap::new(),
                    owners: vec![],
                },
            );
        }
        set_yanked(&state, "foo", "1.0.0", true)
            .await
            .expect("yank succeeds without persistence");
        let yanked = {
            let crates = state.crates.read().await;
            crates[&canonical_name("foo")].entries[0].yanked
        };
        assert!(yanked);
    }
}
