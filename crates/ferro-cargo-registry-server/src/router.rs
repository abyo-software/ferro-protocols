// SPDX-License-Identifier: Apache-2.0
//! Axum router for the Cargo registry protocol.
//!
//! Routes:
//!
//! | Method | Path | Purpose |
//! |---|---|---|
//! | `GET`    | `/config.json`                                           | Index configuration |
//! | `GET`    | `/index/{*path}`                                         | Sparse-index line files |
//! | `GET`    | `/index.git/{*path}`                                     | Phase 2 — 501 stub |
//! | `PUT`    | `/api/v1/crates/new`                                     | Publish |
//! | `GET`    | `/api/v1/crates/{name}/{version}/download`               | Download |
//! | `DELETE` | `/api/v1/crates/{name}/{version}/yank`                   | Yank |
//! | `PUT`    | `/api/v1/crates/{name}/{version}/unyank`                 | Unyank |
//! | `GET`    | `/api/v1/crates/{name}/owners`                           | Owner list |
//! | `PUT`    | `/api/v1/crates/{name}/owners`                           | Owner add |
//! | `DELETE` | `/api/v1/crates/{name}/owners`                           | Owner remove |
//!
//! TUF metadata (Phase 3) is served by `ferrorepo-tuf`; a future
//! `mount()` wires the directory here.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::routing::{delete, get, put};
use ferro_blob_store::BlobStore;
use ferro_blob_store::Digest;
use tokio::sync::RwLock;

use crate::config::IndexConfig;
use crate::handlers::{
    handle_config_json, handle_download, handle_git_index_stub, handle_owners_add,
    handle_owners_delete, handle_owners_list, handle_publish, handle_sparse_index,
    handle_sparse_index_root2, handle_sparse_index_root3, handle_unyank, handle_yank,
};
use crate::index::IndexEntry;
use crate::owners::Owner;

/// Maximum publish request body size, in bytes (20 MiB).
///
/// Axum's default extractor body limit is 2 MiB, which silently rejects
/// any `.crate` upload larger than that with `413 Payload Too Large`
/// before the publish handler runs. Cargo's publish pre-image carries
/// the JSON metadata *plus* the entire `.crate` tarball, and real crates
/// routinely exceed 2 MiB (crates.io permits ~10 MiB tarballs), so the
/// publish route installs an explicit, larger limit. 20 MiB leaves
/// generous headroom over crates.io's documented crate-size cap while
/// still bounding memory per request.
pub const MAX_PUBLISH_BODY_BYTES: usize = 20 * 1024 * 1024;

/// Per-crate state.
#[derive(Debug, Clone, Default)]
pub struct CrateRecord {
    /// Published versions ordered oldest-first.
    pub entries: Vec<IndexEntry>,
    /// Version → tarball digest.
    pub tarballs: BTreeMap<String, Digest>,
    /// Owners list.
    pub owners: Vec<Owner>,
}

/// Shared state held by every Cargo handler.
#[derive(Clone)]
pub struct CargoState {
    /// Content-addressed blob store for `.crate` tarballs.
    pub blobs: Arc<dyn BlobStore>,
    /// Canonical-name → record.
    pub crates: Arc<RwLock<BTreeMap<String, CrateRecord>>>,
    /// Index configuration (returned from `/config.json`).
    pub config: Arc<IndexConfig>,
    /// Data directory used for durable index persistence (DD R2-6).
    ///
    /// `None` means the index map is ephemeral (the in-process / unit-test
    /// path); `Some(dir)` enables write-through mirroring of the index map
    /// to `index-state.json` under `dir` on every mutation, and loading it
    /// on construction via [`with_persistence`](Self::with_persistence).
    pub data_dir: Option<Arc<PathBuf>>,
}

impl CargoState {
    /// Build new **ephemeral** state backed by `blobs` and the given
    /// `api_host`.
    ///
    /// The index map is held in memory only; no durable snapshot is
    /// written or read. Use [`with_persistence`](Self::with_persistence)
    /// for the filesystem-backed deployment that must survive restarts.
    #[must_use]
    pub fn new(blobs: Arc<dyn BlobStore>, api_host: impl Into<String>) -> Self {
        Self {
            blobs,
            crates: Arc::new(RwLock::new(BTreeMap::new())),
            config: Arc::new(IndexConfig::new(api_host)),
            data_dir: None,
        }
    }

    /// Build durable state: load any existing index snapshot from
    /// `data_dir` and enable write-through persistence on every mutation
    /// (DD R2-6).
    ///
    /// A missing or corrupt snapshot starts empty (logged), so this never
    /// fails to construct on a damaged state file.
    #[must_use]
    pub fn with_persistence(
        blobs: Arc<dyn BlobStore>,
        api_host: impl Into<String>,
        data_dir: PathBuf,
    ) -> Self {
        let crates = crate::persist::load(&data_dir);
        Self {
            blobs,
            crates: Arc::new(RwLock::new(crates)),
            config: Arc::new(IndexConfig::new(api_host)),
            data_dir: Some(Arc::new(data_dir)),
        }
    }

    /// Mirror the in-memory index map to the durable snapshot, if
    /// persistence is enabled.
    ///
    /// Call this while holding the **write** guard on
    /// [`crates`](Self::crates) so the snapshot is consistent with the map
    /// that produced it, and so the caller can still roll the in-memory
    /// mutation back on failure before releasing the lock.
    ///
    /// When persistence is disabled (`data_dir == None`, the in-process /
    /// unit-test path) this is a no-op that returns `Ok(())`.
    ///
    /// # Errors
    ///
    /// Returns the underlying I/O / serialization error when the snapshot
    /// could not be written durably. Per DD R3-2 the caller must treat
    /// this as a failed request — roll back the in-memory mutation (and,
    /// for publish, delete the just-written tarball blob) and return a
    /// `5xx` — rather than acknowledging a change that did not survive.
    pub fn persist_locked(
        &self,
        crates: &BTreeMap<String, CrateRecord>,
    ) -> Result<(), std::io::Error> {
        let Some(dir) = &self.data_dir else {
            return Ok(());
        };
        if let Err(err) = crate::persist::save(dir, crates) {
            tracing::error!(
                data_dir = %dir.display(),
                %err,
                "failed to persist index snapshot; rolling back in-memory mutation"
            );
            return Err(err);
        }
        Ok(())
    }
}

/// Build the Cargo registry Axum router.
pub fn router(state: CargoState) -> Router {
    Router::new()
        .route("/config.json", get(handle_config_json))
        .route("/index/{*path}", get(handle_sparse_index))
        .route("/index.git/{*path}", get(handle_git_index_stub))
        .route(
            "/api/v1/crates/new",
            put(handle_publish).layer(DefaultBodyLimit::max(MAX_PUBLISH_BODY_BYTES)),
        )
        .route(
            "/api/v1/crates/{name}/{version}/download",
            get(handle_download),
        )
        .route("/api/v1/crates/{name}/{version}/yank", delete(handle_yank))
        .route("/api/v1/crates/{name}/{version}/unyank", put(handle_unyank))
        .route(
            "/api/v1/crates/{name}/owners",
            get(handle_owners_list)
                .put(handle_owners_add)
                .delete(handle_owners_delete),
        )
        // Root-relative sparse-index layout for `index = "sparse+http://host/"`.
        // Static `api`/`index`/`config.json` segments are matched first by
        // axum, so these only catch the canonical `{prefix}/{name}` shapes.
        .route("/{prefix}/{name}", get(handle_sparse_index_root2))
        .route("/{p0}/{p1}/{name}", get(handle_sparse_index_root3))
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::body::Body;
    use axum::http::{Method, Request, StatusCode};
    use ferro_blob_store::FsBlobStore;
    use tower::ServiceExt;

    use super::{CargoState, MAX_PUBLISH_BODY_BYTES, router};

    #[test]
    fn publish_body_limit_is_documented_size() {
        assert_eq!(MAX_PUBLISH_BODY_BYTES, 20 * 1024 * 1024);
    }

    /// The publish body limit is finite — a body past the configured cap
    /// is rejected with `413`, proving F3 raised (not removed) the limit.
    #[tokio::test]
    async fn publish_over_configured_limit_is_413() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let store = Arc::new(FsBlobStore::new(tmp.path()).expect("blob store"));
        let app = router(CargoState::new(store, "http://localhost"));

        let oversized = vec![0u8; MAX_PUBLISH_BODY_BYTES + 4096];
        let resp = app
            .oneshot(
                Request::builder()
                    .method(Method::PUT)
                    .uri("/api/v1/crates/new")
                    .body(Body::from(oversized))
                    .expect("req"),
            )
            .await
            .expect("resp");
        assert_eq!(resp.status(), StatusCode::PAYLOAD_TOO_LARGE);
    }
}
