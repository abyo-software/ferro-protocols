// SPDX-License-Identifier: Apache-2.0
//! Axum router for the Maven 2/3 protocol.
//!
//! The mount point is conventionally `/repository/{repo}` so FerroRepo
//! can host many named Maven repositories side-by-side. The wildcard
//! `{*path}` captures the complete Maven layout path below that.
//!
//! Spec: Maven Repository Layout —
//! <https://maven.apache.org/repository/layout.html>.

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::Router;
use axum::routing::get;
use ferro_blob_store::BlobStore;
use tokio::sync::RwLock;

use crate::handlers::{handle_delete, handle_get, handle_head, handle_put};
use crate::metadata::MavenMetadata;

/// Shape of a metadata cache key.
///
/// The first two entries are always `(repo, groupPath, artifactId)`;
/// the optional `baseVersion` — when present — scopes the entry to
/// version-level SNAPSHOT metadata at
/// `{groupPath}/{artifactId}/{baseVersion}-SNAPSHOT/maven-metadata.xml`.
pub type MetadataKey = (String, String, String, Option<String>);
/// `MetadataKey` → [`MavenMetadata`] cache.
pub type MetadataCache = Arc<RwLock<BTreeMap<MetadataKey, MavenMetadata>>>;
/// `(repo, groupPath, artifactId, baseVersion)` → monotonic build
/// number.
pub type SnapshotCounter = Arc<RwLock<BTreeMap<(String, String, String, String), u32>>>;

/// Shared state held by every Maven handler.
///
/// Wraps a content-addressed [`BlobStore`] and three in-memory indices:
///
/// - `layout`: maps the Maven layout path (e.g.
///   `com/example/foo/1.0/foo-1.0.jar`) to a
///   `ferro_blob_store::Digest`, so incoming `GET`s can find the right
///   blob.
/// - `metadata`: caches the parsed [`MavenMetadata`] for each
///   `{repo}/{groupPath}/{artifactId}` so regeneration on `PUT` does
///   not need a full scan.
/// - `snapshot_counter`: monotonic per-base-version build numbers for
///   SNAPSHOT timestamping.
#[derive(Clone)]
pub struct MavenState {
    /// Shared blob store (typically `Arc<FsBlobStore>`).
    pub blobs: Arc<dyn BlobStore>,
    /// `{repo}/{layout-path}` → digest index.
    pub layout: Arc<RwLock<BTreeMap<String, ferro_blob_store::Digest>>>,
    /// `(repo, groupPath, artifactId, Option<baseVersion>)` → metadata
    /// cache.
    pub metadata: MetadataCache,
    /// `(repo, groupPath, artifactId, baseVersion)` → monotonic build
    /// counter.
    pub snapshot_counter: SnapshotCounter,
}

impl MavenState {
    /// Build a new state backed by `blobs`.
    #[must_use]
    pub fn new(blobs: Arc<dyn BlobStore>) -> Self {
        Self {
            blobs,
            layout: Arc::new(RwLock::new(BTreeMap::new())),
            metadata: Arc::new(RwLock::new(BTreeMap::new())),
            snapshot_counter: Arc::new(RwLock::new(BTreeMap::new())),
        }
    }
}

/// Build the Maven Axum router.
///
/// Routes all HTTP verbs on `/repository/{repo}/{*path}` to the
/// appropriate handler.
pub fn router(state: MavenState) -> Router {
    Router::new()
        .route(
            "/repository/{repo}/{*path}",
            get(handle_get)
                .head(handle_head)
                .put(handle_put)
                .delete(handle_delete),
        )
        .with_state(state)
}
