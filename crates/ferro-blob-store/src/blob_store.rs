// SPDX-License-Identifier: Apache-2.0
//! The [`BlobStore`] trait.

use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;

use crate::{Digest, Result};

/// Convenience type alias: `Arc<dyn BlobStore>`. Use this when you
/// want to pass a [`BlobStore`] handle by value across async tasks
/// without re-introducing the dynamic-dispatch boilerplate at every
/// call site.
pub type SharedBlobStore = Arc<dyn BlobStore>;

/// A content-addressed blob store.
///
/// Implementations are `Send + Sync` and perform their own concurrency
/// control. Callers treat the trait as a key-value store keyed by
/// [`Digest`]; implementations are free to layer caches, compression,
/// or replication beneath it.
///
/// ### `put` semantics
///
/// `put` is responsible for verifying that the SHA-256 (or SHA-512)
/// of the supplied bytes matches the supplied [`Digest`]. On
/// mismatch implementations return
/// [`crate::BlobStoreError::DigestMismatch`].
///
/// Writes SHOULD be atomic: a partial write must not be observable by
/// a concurrent reader. The reference [`crate::FsBlobStore`]
/// achieves this via temp-file + atomic rename; the in-memory
/// implementation is atomic by virtue of holding the write lock for
/// the duration of the insert.
///
/// ### Future evolution
///
/// `v0.1` will add `put_stream` / `get_stream` and a paginated
/// `list` variant. The current method set is stable for the
/// `v0.0.x` series.
#[async_trait]
pub trait BlobStore: Send + Sync {
    /// Write `bytes` under `digest`. See [trait-level documentation](BlobStore)
    /// for atomicity and digest-verification requirements.
    async fn put(&self, digest: &Digest, bytes: Bytes) -> Result<()>;

    /// Read the bytes stored under `digest`.
    ///
    /// Returns [`crate::BlobStoreError::NotFound`] when the blob is
    /// absent.
    async fn get(&self, digest: &Digest) -> Result<Bytes>;

    /// Check for blob presence without fetching the body.
    async fn contains(&self, digest: &Digest) -> Result<bool>;

    /// Remove the blob keyed by `digest`. Deleting a missing blob is
    /// **not** an error.
    async fn delete(&self, digest: &Digest) -> Result<()>;

    /// Enumerate every blob currently stored.
    ///
    /// Returned in no particular order. The `v0.1` paginated variant
    /// will supersede this method for backends where the full set
    /// does not fit in memory.
    async fn list(&self) -> Result<Vec<Digest>>;
}
