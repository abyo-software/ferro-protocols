// SPDX-License-Identifier: Apache-2.0
//! Registry metadata plane.
//!
//! Spec: OCI Distribution Spec v1.1 §3 "Namespaces" and §4 "Pulling /
//! Pushing".
//!
//! The blob bytes live in [`ferro_blob_store::BlobStore`]; this trait
//! covers everything else the registry needs to persist — manifests
//! (keyed by digest), tag -> digest aliases, in-flight upload
//! sessions, and referrer lookups.
//!
//! Phase 1 ships [`InMemoryRegistryMeta`], which uses
//! `parking_lot::RwLock` to guard a handful of `BTreeMap`s. A
//! SQLite- / Postgres-backed impl lands in Phase 2.

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use ferro_blob_store::{Digest, Result};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

use crate::reference::Reference;
use crate::upload::UploadState;

/// Descriptor returned by the referrers API.
///
/// Spec §3.3: the response body is an OCI image index whose
/// `manifests` array contains one of these per referrer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReferrerDescriptor {
    /// Media type of the referring manifest.
    #[serde(rename = "mediaType")]
    pub media_type: String,
    /// Digest of the referring manifest.
    pub digest: Digest,
    /// Size in bytes of the referring manifest.
    pub size: u64,
    /// Optional artifact type, used as the filter key.
    #[serde(
        rename = "artifactType",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub artifact_type: Option<String>,
    /// Optional annotations.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub annotations: Option<BTreeMap<String, String>>,
}

/// Metadata-plane operations required by the OCI handlers.
#[async_trait]
pub trait RegistryMeta: Send + Sync {
    /// Persist a manifest body under (name, reference).
    ///
    /// If `reference` is a tag, this also records the tag -> digest
    /// alias. The canonical key is always the digest, so a digest-keyed
    /// `get_manifest` must succeed after a tag-keyed put.
    async fn put_manifest(
        &self,
        name: &str,
        reference: &Reference,
        digest: &Digest,
        media_type: &str,
        body: Bytes,
    ) -> Result<()>;

    /// Fetch a manifest by (name, reference).
    ///
    /// Returns `(canonical-digest, media-type, body)`.
    async fn get_manifest(
        &self,
        name: &str,
        reference: &Reference,
    ) -> Result<Option<(Digest, String, Bytes)>>;

    /// Delete a manifest by (name, reference).
    ///
    /// Returns `true` if something was deleted. The handler layer is
    /// responsible for rejecting DELETE-by-tag per spec §4.9.
    async fn delete_manifest(&self, name: &str, reference: &Reference) -> Result<bool>;

    /// List tags for `name`, honouring `n` / `last` pagination.
    async fn list_tags(
        &self,
        name: &str,
        last: Option<&str>,
        n: Option<usize>,
    ) -> Result<Vec<String>>;

    /// List repositories (catalog endpoint).
    async fn list_repositories(&self, last: Option<&str>, n: Option<usize>) -> Result<Vec<String>>;

    /// Allocate a new upload UUID for `name`.
    async fn start_upload(&self, name: &str) -> Result<String>;

    /// Append `chunk` to the upload and return the new byte offset.
    async fn append_upload(&self, name: &str, uuid: &str, offset: u64, chunk: Bytes)
    -> Result<u64>;

    /// Mark the upload as complete and return the buffered bytes.
    async fn complete_upload(&self, name: &str, uuid: &str, digest: &Digest) -> Result<()>;

    /// Retrieve the current upload state (without consuming it).
    async fn get_upload_state(&self, name: &str, uuid: &str) -> Result<Option<UploadState>>;

    /// Cancel and discard an in-flight upload. Returns `true` when the
    /// upload existed.
    async fn cancel_upload(&self, name: &str, uuid: &str) -> Result<bool>;

    /// List referrers for `digest` in `name`, filtered by artifact type.
    async fn list_referrers(
        &self,
        name: &str,
        digest: &Digest,
        artifact_type: Option<&str>,
    ) -> Result<Vec<ReferrerDescriptor>>;

    /// Take the accumulated upload buffer. The handler needs this on
    /// finalize to hand the bytes to the blob store.
    async fn take_upload_bytes(&self, name: &str, uuid: &str) -> Result<Option<Bytes>>;

    /// Register a referrer descriptor against a subject digest.
    ///
    /// Phase 1 uses this from the manifest PUT handler when the
    /// manifest has a `subject` field, so the referrers API can surface
    /// it without a scan.
    async fn register_referrer(
        &self,
        name: &str,
        subject: &Digest,
        descriptor: ReferrerDescriptor,
    ) -> Result<()>;
}

/// In-memory `RegistryMeta` impl for tests and single-node deployments.
#[derive(Default)]
pub struct InMemoryRegistryMeta {
    inner: RwLock<InMemoryState>,
}

#[derive(Default)]
struct InMemoryState {
    // name -> digest-string -> (media-type, body)
    manifests: BTreeMap<String, BTreeMap<String, (String, Bytes)>>,
    // name -> tag -> digest-string
    tags: BTreeMap<String, BTreeMap<String, String>>,
    // (name, uuid) -> UploadState
    uploads: BTreeMap<(String, String), UploadState>,
    // name -> subject-digest-string -> [referrer]
    referrers: BTreeMap<String, BTreeMap<String, Vec<ReferrerDescriptor>>>,
}

impl InMemoryRegistryMeta {
    /// Construct a fresh empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Wrap in an `Arc<dyn RegistryMeta>` for [`crate::router::AppState`].
    #[must_use]
    pub fn shared() -> Arc<dyn RegistryMeta> {
        Arc::new(Self::new())
    }
}

#[async_trait]
impl RegistryMeta for InMemoryRegistryMeta {
    async fn put_manifest(
        &self,
        name: &str,
        reference: &Reference,
        digest: &Digest,
        media_type: &str,
        body: Bytes,
    ) -> Result<()> {
        let mut guard = self.inner.write();
        guard
            .manifests
            .entry(name.to_owned())
            .or_default()
            .insert(digest.to_string(), (media_type.to_owned(), body));
        if let Reference::Tag(tag) = reference {
            guard
                .tags
                .entry(name.to_owned())
                .or_default()
                .insert(tag.clone(), digest.to_string());
        }
        Ok(())
    }

    async fn get_manifest(
        &self,
        name: &str,
        reference: &Reference,
    ) -> Result<Option<(Digest, String, Bytes)>> {
        let guard = self.inner.read();
        let Some(name_map) = guard.manifests.get(name) else {
            return Ok(None);
        };
        let digest_str: String = match reference {
            Reference::Digest(d) => d.to_string(),
            Reference::Tag(t) => match guard.tags.get(name).and_then(|m| m.get(t)) {
                Some(s) => s.clone(),
                None => return Ok(None),
            },
        };
        let Some((media_type, body)) = name_map.get(&digest_str) else {
            return Ok(None);
        };
        let digest: Digest = digest_str
            .parse()
            .map_err(ferro_blob_store::BlobStoreError::InvalidDigest)?;
        Ok(Some((digest, media_type.clone(), body.clone())))
    }

    async fn delete_manifest(&self, name: &str, reference: &Reference) -> Result<bool> {
        let mut guard = self.inner.write();
        match reference {
            Reference::Digest(d) => {
                let digest_str = d.to_string();
                let Some(name_map) = guard.manifests.get_mut(name) else {
                    return Ok(false);
                };
                let removed = name_map.remove(&digest_str).is_some();
                if removed && let Some(tag_map) = guard.tags.get_mut(name) {
                    tag_map.retain(|_, v| v != &digest_str);
                }
                Ok(removed)
            }
            Reference::Tag(_) => Ok(false),
        }
    }

    async fn list_tags(
        &self,
        name: &str,
        last: Option<&str>,
        n: Option<usize>,
    ) -> Result<Vec<String>> {
        let guard = self.inner.read();
        let Some(tag_map) = guard.tags.get(name) else {
            return Ok(Vec::new());
        };
        let mut names: Vec<String> = tag_map.keys().cloned().collect();
        names.sort();
        if let Some(cursor) = last {
            names.retain(|t| t.as_str() > cursor);
        }
        if let Some(limit) = n {
            names.truncate(limit);
        }
        Ok(names)
    }

    async fn list_repositories(&self, last: Option<&str>, n: Option<usize>) -> Result<Vec<String>> {
        let guard = self.inner.read();
        let mut names: Vec<String> = guard.manifests.keys().cloned().collect();
        names.sort();
        if let Some(cursor) = last {
            names.retain(|t| t.as_str() > cursor);
        }
        if let Some(limit) = n {
            names.truncate(limit);
        }
        Ok(names)
    }

    async fn start_upload(&self, name: &str) -> Result<String> {
        let uuid = uuid::Uuid::new_v4().to_string();
        let mut guard = self.inner.write();
        guard.uploads.insert(
            (name.to_owned(), uuid.clone()),
            UploadState::new(name, uuid.clone()),
        );
        Ok(uuid)
    }

    async fn append_upload(
        &self,
        name: &str,
        uuid: &str,
        offset: u64,
        chunk: Bytes,
    ) -> Result<u64> {
        let mut guard = self.inner.write();
        let key = (name.to_owned(), uuid.to_owned());
        let Some(state) = guard.uploads.get_mut(&key) else {
            return Err(ferro_blob_store::BlobStoreError::NotFound(format!(
                "unknown upload uuid: {uuid}"
            )));
        };
        // Spec §4.3: chunked uploads must be sequential — the next
        // chunk's `Content-Range` start must equal the current offset.
        if offset != state.offset() {
            return Err(ferro_blob_store::BlobStoreError::NotFound(format!(
                "out-of-order upload chunk: expected offset {}, got {offset}",
                state.offset()
            )));
        }
        Ok(state.append(&chunk))
    }

    async fn complete_upload(&self, name: &str, uuid: &str, _digest: &Digest) -> Result<()> {
        let mut guard = self.inner.write();
        let key = (name.to_owned(), uuid.to_owned());
        guard.uploads.remove(&key);
        Ok(())
    }

    async fn get_upload_state(&self, name: &str, uuid: &str) -> Result<Option<UploadState>> {
        let guard = self.inner.read();
        let key = (name.to_owned(), uuid.to_owned());
        Ok(guard.uploads.get(&key).cloned())
    }

    async fn cancel_upload(&self, name: &str, uuid: &str) -> Result<bool> {
        let mut guard = self.inner.write();
        let key = (name.to_owned(), uuid.to_owned());
        Ok(guard.uploads.remove(&key).is_some())
    }

    async fn list_referrers(
        &self,
        name: &str,
        digest: &Digest,
        artifact_type: Option<&str>,
    ) -> Result<Vec<ReferrerDescriptor>> {
        let guard = self.inner.read();
        let Some(name_map) = guard.referrers.get(name) else {
            return Ok(Vec::new());
        };
        let Some(list) = name_map.get(&digest.to_string()) else {
            return Ok(Vec::new());
        };
        let out = match artifact_type {
            Some(at) => list
                .iter()
                .filter(|d| d.artifact_type.as_deref() == Some(at))
                .cloned()
                .collect(),
            None => list.clone(),
        };
        Ok(out)
    }

    async fn take_upload_bytes(&self, name: &str, uuid: &str) -> Result<Option<Bytes>> {
        let mut guard = self.inner.write();
        let key = (name.to_owned(), uuid.to_owned());
        Ok(guard.uploads.get_mut(&key).map(UploadState::take_bytes))
    }

    async fn register_referrer(
        &self,
        name: &str,
        subject: &Digest,
        descriptor: ReferrerDescriptor,
    ) -> Result<()> {
        let mut guard = self.inner.write();
        guard
            .referrers
            .entry(name.to_owned())
            .or_default()
            .entry(subject.to_string())
            .or_default()
            .push(descriptor);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{InMemoryRegistryMeta, ReferrerDescriptor, RegistryMeta};
    use crate::reference::Reference;
    use crate::upload::UploadState;
    use bytes::Bytes;
    use ferro_blob_store::Digest;

    #[tokio::test]
    async fn start_append_take_cycle() {
        let reg = InMemoryRegistryMeta::new();
        let uuid = reg.start_upload("lib/alpine").await.expect("start");
        let new_off = reg
            .append_upload("lib/alpine", &uuid, 0, Bytes::from_static(b"hello"))
            .await
            .expect("append");
        assert_eq!(new_off, 5);
        let state: UploadState = reg
            .get_upload_state("lib/alpine", &uuid)
            .await
            .expect("get")
            .expect("state present");
        assert_eq!(state.offset(), 5);
        let body = reg
            .take_upload_bytes("lib/alpine", &uuid)
            .await
            .expect("take")
            .expect("bytes present");
        assert_eq!(&body[..], b"hello");
    }

    #[tokio::test]
    async fn out_of_order_chunk_is_rejected() {
        let reg = InMemoryRegistryMeta::new();
        let uuid = reg.start_upload("lib/alpine").await.expect("start");
        reg.append_upload("lib/alpine", &uuid, 0, Bytes::from_static(b"ab"))
            .await
            .expect("first chunk");
        let err = reg
            .append_upload("lib/alpine", &uuid, 10, Bytes::from_static(b"cd"))
            .await
            .expect_err("out-of-order chunk must fail");
        assert!(matches!(err, ferro_blob_store::BlobStoreError::NotFound(_)));
    }

    #[tokio::test]
    async fn manifest_put_and_lookup_by_digest_and_tag() {
        let reg = InMemoryRegistryMeta::new();
        let digest = Digest::sha256_of(b"manifest-body");
        reg.put_manifest(
            "lib/alpine",
            &Reference::Tag("latest".to_owned()),
            &digest,
            "application/vnd.oci.image.manifest.v1+json",
            Bytes::from_static(b"manifest-body"),
        )
        .await
        .expect("put manifest");
        let by_tag = reg
            .get_manifest("lib/alpine", &Reference::Tag("latest".to_owned()))
            .await
            .expect("get by tag")
            .expect("present");
        assert_eq!(by_tag.0, digest);
        let by_digest = reg
            .get_manifest("lib/alpine", &Reference::Digest(digest.clone()))
            .await
            .expect("get by digest")
            .expect("present");
        assert_eq!(by_digest.0, digest);
    }

    #[tokio::test]
    async fn delete_by_tag_returns_false() {
        let reg = InMemoryRegistryMeta::new();
        let digest = Digest::sha256_of(b"manifest-body");
        reg.put_manifest(
            "lib/alpine",
            &Reference::Tag("latest".to_owned()),
            &digest,
            "application/vnd.oci.image.manifest.v1+json",
            Bytes::from_static(b"manifest-body"),
        )
        .await
        .expect("put manifest");
        let removed = reg
            .delete_manifest("lib/alpine", &Reference::Tag("latest".to_owned()))
            .await
            .expect("delete by tag");
        assert!(!removed);
    }

    #[tokio::test]
    async fn referrers_filter_by_artifact_type() {
        let reg = InMemoryRegistryMeta::new();
        let subject = Digest::sha256_of(b"subject");
        let d1 = Digest::sha256_of(b"sbom");
        let d2 = Digest::sha256_of(b"sig");
        reg.register_referrer(
            "lib/alpine",
            &subject,
            ReferrerDescriptor {
                media_type: "application/vnd.oci.image.manifest.v1+json".to_owned(),
                digest: d1,
                size: 10,
                artifact_type: Some("application/spdx+json".to_owned()),
                annotations: None,
            },
        )
        .await
        .expect("register sbom referrer");
        reg.register_referrer(
            "lib/alpine",
            &subject,
            ReferrerDescriptor {
                media_type: "application/vnd.oci.image.manifest.v1+json".to_owned(),
                digest: d2,
                size: 10,
                artifact_type: Some("application/vnd.dev.cosign.sig".to_owned()),
                annotations: None,
            },
        )
        .await
        .expect("register sig referrer");
        let all = reg
            .list_referrers("lib/alpine", &subject, None)
            .await
            .expect("list all");
        assert_eq!(all.len(), 2);
        let sboms = reg
            .list_referrers("lib/alpine", &subject, Some("application/spdx+json"))
            .await
            .expect("list sboms");
        assert_eq!(sboms.len(), 1);
    }
}
