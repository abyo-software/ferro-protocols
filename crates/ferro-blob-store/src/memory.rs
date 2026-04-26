// SPDX-License-Identifier: Apache-2.0
//! In-memory [`BlobStore`] reference implementation.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use bytes::Bytes;

use crate::{BlobStore, BlobStoreError, Digest, DigestAlgo, Result};

/// `Arc<RwLock<HashMap<Digest, Bytes>>>` reference implementation.
///
/// Cheap to clone; multiple handles share the same backing map. Useful
/// for tests, ephemeral caches, and a baseline for performance
/// comparison against custom backends.
#[derive(Debug, Clone, Default)]
pub struct InMemoryBlobStore {
    inner: Arc<RwLock<HashMap<Digest, Bytes>>>,
}

impl InMemoryBlobStore {
    /// Construct an empty store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of stored blobs.
    #[must_use]
    pub fn len(&self) -> usize {
        self.inner.read().expect("poisoned").len()
    }

    /// `true` when the store holds no blobs.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.inner.read().expect("poisoned").is_empty()
    }

    /// Drop every entry.
    pub fn clear(&self) {
        self.inner.write().expect("poisoned").clear();
    }
}

#[async_trait]
impl BlobStore for InMemoryBlobStore {
    async fn put(&self, digest: &Digest, bytes: Bytes) -> Result<()> {
        let computed = match digest.algo() {
            DigestAlgo::Sha256 => Digest::sha256_of(&bytes),
            DigestAlgo::Sha512 => Digest::sha512_of(&bytes),
        };
        if &computed != digest {
            return Err(BlobStoreError::DigestMismatch {
                expected: digest.to_string(),
                computed: computed.to_string(),
            });
        }
        self.inner
            .write()
            .expect("poisoned")
            .insert(digest.clone(), bytes);
        Ok(())
    }

    async fn get(&self, digest: &Digest) -> Result<Bytes> {
        self.inner
            .read()
            .expect("poisoned")
            .get(digest)
            .cloned()
            .ok_or_else(|| BlobStoreError::NotFound(digest.to_string()))
    }

    async fn contains(&self, digest: &Digest) -> Result<bool> {
        Ok(self.inner.read().expect("poisoned").contains_key(digest))
    }

    async fn delete(&self, digest: &Digest) -> Result<()> {
        self.inner.write().expect("poisoned").remove(digest);
        Ok(())
    }

    async fn list(&self) -> Result<Vec<Digest>> {
        Ok(self
            .inner
            .read()
            .expect("poisoned")
            .keys()
            .cloned()
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn put_get_round_trip() {
        let store = InMemoryBlobStore::new();
        let body = Bytes::from_static(b"hello");
        let d = Digest::sha256_of(&body);
        store.put(&d, body.clone()).await.unwrap();
        assert_eq!(store.get(&d).await.unwrap(), body);
    }

    #[tokio::test]
    async fn put_rejects_digest_mismatch() {
        let store = InMemoryBlobStore::new();
        let real_body = Bytes::from_static(b"hello");
        let lying_digest = Digest::sha256_of(b"goodbye");
        let err = store.put(&lying_digest, real_body).await.unwrap_err();
        assert!(matches!(err, BlobStoreError::DigestMismatch { .. }));
    }

    #[tokio::test]
    async fn contains_and_list() {
        let store = InMemoryBlobStore::new();
        let body = Bytes::from_static(b"x");
        let d = Digest::sha256_of(&body);
        assert!(!store.contains(&d).await.unwrap());
        store.put(&d, body).await.unwrap();
        assert!(store.contains(&d).await.unwrap());
        let listed = store.list().await.unwrap();
        assert_eq!(listed, vec![d]);
    }

    #[tokio::test]
    async fn get_returns_not_found() {
        let store = InMemoryBlobStore::new();
        let d = Digest::sha256_of(b"missing");
        let err = store.get(&d).await.unwrap_err();
        assert!(matches!(err, BlobStoreError::NotFound(_)));
    }

    #[tokio::test]
    async fn delete_missing_is_ok() {
        let store = InMemoryBlobStore::new();
        let d = Digest::sha256_of(b"never-stored");
        store.delete(&d).await.unwrap();
    }

    #[tokio::test]
    async fn clone_shares_storage() {
        let a = InMemoryBlobStore::new();
        let b = a.clone();
        let body = Bytes::from_static(b"shared");
        let d = Digest::sha256_of(&body);
        a.put(&d, body.clone()).await.unwrap();
        assert_eq!(b.get(&d).await.unwrap(), body);
        assert_eq!(b.len(), 1);
        b.clear();
        assert!(a.is_empty());
    }
}
