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
use std::sync::Arc;

use axum::Router;
use axum::routing::{delete, get, put};
use ferro_blob_store::BlobStore;
use ferro_blob_store::Digest;
use tokio::sync::RwLock;

use crate::config::IndexConfig;
use crate::handlers::{
    handle_config_json, handle_download, handle_git_index_stub, handle_owners_add,
    handle_owners_delete, handle_owners_list, handle_publish, handle_sparse_index, handle_unyank,
    handle_yank,
};
use crate::index::IndexEntry;
use crate::owners::Owner;

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
}

impl CargoState {
    /// Build new state backed by `blobs` and the given `api_host`.
    #[must_use]
    pub fn new(blobs: Arc<dyn BlobStore>, api_host: impl Into<String>) -> Self {
        Self {
            blobs,
            crates: Arc::new(RwLock::new(BTreeMap::new())),
            config: Arc::new(IndexConfig::new(api_host)),
        }
    }
}

/// Build the Cargo registry Axum router.
pub fn router(state: CargoState) -> Router {
    Router::new()
        .route("/config.json", get(handle_config_json))
        .route("/index/{*path}", get(handle_sparse_index))
        .route("/index.git/{*path}", get(handle_git_index_stub))
        .route("/api/v1/crates/new", put(handle_publish))
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
        .with_state(state)
}
