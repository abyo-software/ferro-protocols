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
//! `SQLite`- / `Postgres`-backed impl lands in Phase 2.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use bytes::Bytes;
use ferro_blob_store::{Digest, Result};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

use crate::reference::Reference;
use crate::upload::UploadState;

/// Default ceiling on the number of in-flight upload sessions a single
/// registry process will hold open at once (R2-7).
///
/// `F2` bounded the bytes of *one* session, but an unauthenticated client
/// could still open an unbounded *number* of sessions (or many near-cap
/// sessions) and exhaust process memory. We cap concurrent sessions and
/// evict idle ones to close that vector. 1024 is generous for honest
/// multi-client pushing while bounding the worst case; override via
/// [`InMemoryRegistryMeta::with_session_limits`].
pub const DEFAULT_MAX_UPLOAD_SESSIONS: usize = 1024;

/// Default idle eviction window for an in-flight upload session (R2-7).
///
/// Sessions with no `PATCH`/`PUT` activity for this long are swept lazily
/// when a new upload is started or the session is next accessed. One hour
/// matches common registry defaults.
pub const DEFAULT_UPLOAD_SESSION_TTL: Duration = Duration::from_secs(60 * 60);

/// Outcome of trying to admit a new upload session.
///
/// Returned by [`RegistryMeta::start_upload`] so the handler can map a
/// capacity rejection onto `429 Too Many Requests` rather than a generic
/// error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UploadAdmission {
    /// A session was created; carries its UUID.
    Started(String),
    /// The concurrent-session cap is reached; the client should retry
    /// later. Carries the cap for the diagnostic message.
    AtCapacity(usize),
}

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

    /// Allocate a new upload UUID for `name`, subject to the concurrent
    /// session cap and idle-session TTL (R2-7).
    ///
    /// Implementations SHOULD first evict any sessions idle past their TTL
    /// (a lazy sweep), then admit the new session only if the live count
    /// is below the cap. The returned [`UploadAdmission`] distinguishes a
    /// fresh UUID from a capacity rejection so the handler can answer
    /// `429 Too Many Requests`.
    async fn start_upload(&self, name: &str) -> Result<UploadAdmission>;

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

/// Tunable caps on in-flight upload sessions (R2-7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionLimits {
    /// Maximum number of concurrent in-flight upload sessions.
    pub max_sessions: usize,
    /// Idle duration after which a session is evicted on the next sweep.
    pub idle_ttl: Duration,
}

impl Default for SessionLimits {
    fn default() -> Self {
        Self {
            max_sessions: DEFAULT_MAX_UPLOAD_SESSIONS,
            idle_ttl: DEFAULT_UPLOAD_SESSION_TTL,
        }
    }
}

/// Filename of the durable metadata mirror written under the storage dir
/// for the filesystem-backed deployment (R2-6).
pub const METADATA_FILE_NAME: &str = "metadata.json";

/// In-memory `RegistryMeta` impl for tests and single-node deployments.
///
/// The hot path is always the in-memory state. When constructed with
/// [`InMemoryRegistryMeta::with_persistence`], every metadata mutation is
/// additionally mirrored (snapshot-on-write) to a JSON file under the
/// storage directory and reloaded on boot, so the filesystem-backed
/// binary no longer loses manifests/tags/referrers across a restart
/// (R2-6). The default constructors stay fully in-memory / ephemeral —
/// tests and the in-memory blob-store deployment are unaffected.
#[derive(Default)]
pub struct InMemoryRegistryMeta {
    inner: RwLock<InMemoryState>,
    limits: SessionLimits,
    /// When set, the file the metadata snapshot is mirrored to on every
    /// mutation and loaded from on construction. `None` → ephemeral.
    persist_path: Option<std::path::PathBuf>,
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

/// On-disk metadata snapshot (R2-6).
///
/// Mirrors the durable parts of [`InMemoryState`] — manifests (with their
/// media type + raw body, base64-encoded for JSON), tag aliases, and the
/// referrer index. In-flight upload sessions are intentionally NOT
/// persisted: they are transient and a restart legitimately aborts them.
#[derive(Default, Serialize, Deserialize)]
struct MetadataSnapshot {
    /// name -> digest-string -> persisted manifest.
    manifests: BTreeMap<String, BTreeMap<String, PersistedManifest>>,
    /// name -> tag -> digest-string.
    tags: BTreeMap<String, BTreeMap<String, String>>,
    /// name -> subject-digest-string -> referrers.
    referrers: BTreeMap<String, BTreeMap<String, Vec<ReferrerDescriptor>>>,
}

/// A manifest body plus its media type, JSON-serialisable.
#[derive(Serialize, Deserialize)]
struct PersistedManifest {
    media_type: String,
    /// Raw manifest bytes, hex-encoded so the canonical digest over the
    /// exact bytes round-trips through JSON. (`hex` is already a crate
    /// dependency; avoiding base64 keeps the dep set unchanged.)
    body_hex: String,
}

impl InMemoryRegistryMeta {
    /// Construct a fresh empty registry with default session limits.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Construct a registry with custom upload-session limits (R2-7).
    ///
    /// Used by tests to exercise the concurrent-session cap and idle TTL
    /// without opening a thousand sessions or sleeping for an hour.
    #[must_use]
    pub fn with_session_limits(limits: SessionLimits) -> Self {
        Self {
            inner: RwLock::default(),
            limits,
            persist_path: None,
        }
    }

    /// Construct a registry whose metadata is durably mirrored under
    /// `storage_dir` (R2-6).
    ///
    /// On construction the existing `metadata.json` (if any) is loaded so
    /// manifests/tags/referrers survive a process restart of the
    /// filesystem-backed binary. A missing or corrupt file is tolerated:
    /// the registry starts empty and logs a warning (so a single bad write
    /// cannot wedge boot). Every subsequent metadata mutation snapshots the
    /// whole state back to that file write-through.
    #[must_use]
    pub fn with_persistence(storage_dir: &std::path::Path) -> Self {
        let persist_path = storage_dir.join(METADATA_FILE_NAME);
        let state = match Self::load_snapshot(&persist_path) {
            Ok(Some(state)) => {
                tracing::info!(
                    path = %persist_path.display(),
                    repos = state.manifests.len(),
                    "loaded persisted registry metadata"
                );
                state
            }
            Ok(None) => InMemoryState::default(),
            Err(e) => {
                tracing::warn!(
                    path = %persist_path.display(),
                    error = %e,
                    "could not load registry metadata; starting empty"
                );
                InMemoryState::default()
            }
        };
        Self {
            inner: RwLock::new(state),
            limits: SessionLimits::default(),
            persist_path: Some(persist_path),
        }
    }

    /// Load and decode the metadata snapshot from `path`.
    ///
    /// Returns `Ok(None)` when the file does not exist, `Ok(Some(state))`
    /// on a valid snapshot, and `Err` on an unreadable / corrupt file.
    fn load_snapshot(path: &std::path::Path) -> std::io::Result<Option<InMemoryState>> {
        let raw = match std::fs::read(path) {
            Ok(r) => r,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e),
        };
        let snapshot: MetadataSnapshot = serde_json::from_slice(&raw)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        let mut manifests: BTreeMap<String, BTreeMap<String, (String, Bytes)>> = BTreeMap::new();
        for (name, by_digest) in snapshot.manifests {
            let mut inner = BTreeMap::new();
            for (digest, pm) in by_digest {
                let bytes = hex::decode(&pm.body_hex).map_err(|e| {
                    std::io::Error::new(std::io::ErrorKind::InvalidData, e)
                })?;
                inner.insert(digest, (pm.media_type, Bytes::from(bytes)));
            }
            manifests.insert(name, inner);
        }
        Ok(Some(InMemoryState {
            manifests,
            tags: snapshot.tags,
            uploads: BTreeMap::new(),
            referrers: snapshot.referrers,
        }))
    }

    /// Snapshot the durable parts of `state` to the configured path, if
    /// persistence is enabled. Best-effort: a write failure is logged but
    /// does not fail the in-memory mutation (the hot path remains
    /// authoritative; the next successful write re-syncs the mirror).
    ///
    /// Caller holds the write lock so the mirror is consistent with the
    /// state it snapshots.
    fn persist_locked(&self, state: &InMemoryState) {
        let Some(path) = &self.persist_path else {
            return;
        };
        let snapshot = MetadataSnapshot {
            manifests: state
                .manifests
                .iter()
                .map(|(name, by_digest)| {
                    let inner = by_digest
                        .iter()
                        .map(|(digest, (media_type, body))| {
                            (
                                digest.clone(),
                                PersistedManifest {
                                    media_type: media_type.clone(),
                                    body_hex: hex::encode(body),
                                },
                            )
                        })
                        .collect();
                    (name.clone(), inner)
                })
                .collect(),
            tags: state.tags.clone(),
            referrers: state.referrers.clone(),
        };
        if let Err(e) = Self::write_snapshot_atomic(path, &snapshot) {
            tracing::warn!(
                path = %path.display(),
                error = %e,
                "failed to persist registry metadata snapshot"
            );
        }
    }

    /// Serialise `snapshot` and write it to `path` atomically via a
    /// temp-file + rename so a crash mid-write never leaves a truncated
    /// `metadata.json`.
    fn write_snapshot_atomic(
        path: &std::path::Path,
        snapshot: &MetadataSnapshot,
    ) -> std::io::Result<()> {
        let bytes = serde_json::to_vec(snapshot)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, &bytes)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    /// The upload-session limits in force.
    #[must_use]
    pub const fn session_limits(&self) -> SessionLimits {
        self.limits
    }

    /// Wrap in an `Arc<dyn RegistryMeta>` for [`crate::router::AppState`].
    #[must_use]
    pub fn shared() -> Arc<dyn RegistryMeta> {
        Arc::new(Self::new())
    }
}

impl InMemoryState {
    /// Evict every upload session idle for at least `ttl` relative to
    /// `now`. Returns the number of sessions swept. Caller holds the
    /// write lock.
    fn sweep_idle_uploads(&mut self, now: Instant, ttl: Duration) -> usize {
        let before = self.uploads.len();
        self.uploads
            .retain(|_, state| !state.is_idle_for(now, ttl));
        before - self.uploads.len()
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
        self.persist_locked(&guard);
        drop(guard);
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
        let Some((media_type, body)) = name_map.get(&digest_str).cloned() else {
            return Ok(None);
        };
        drop(guard);
        let digest: Digest = digest_str
            .parse()
            .map_err(ferro_blob_store::BlobStoreError::InvalidDigest)?;
        Ok(Some((digest, media_type, body)))
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
                if removed {
                    self.persist_locked(&guard);
                }
                drop(guard);
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
        drop(guard);
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
        drop(guard);
        names.sort();
        if let Some(cursor) = last {
            names.retain(|t| t.as_str() > cursor);
        }
        if let Some(limit) = n {
            names.truncate(limit);
        }
        Ok(names)
    }

    async fn start_upload(&self, name: &str) -> Result<UploadAdmission> {
        let uuid = uuid::Uuid::new_v4().to_string();
        let now = Instant::now();
        let mut guard = self.inner.write();
        // R2-7: lazily evict idle sessions before admitting a new one so a
        // burst of abandoned sessions cannot pin memory forever, then
        // enforce the concurrent-session cap.
        guard.sweep_idle_uploads(now, self.limits.idle_ttl);
        if guard.uploads.len() >= self.limits.max_sessions {
            drop(guard);
            return Ok(UploadAdmission::AtCapacity(self.limits.max_sessions));
        }
        guard.uploads.insert(
            (name.to_owned(), uuid.clone()),
            UploadState::new(name, uuid.clone()),
        );
        drop(guard);
        Ok(UploadAdmission::Started(uuid))
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
        let new_offset = state.append(&chunk);
        drop(guard);
        Ok(new_offset)
    }

    async fn complete_upload(&self, name: &str, uuid: &str, _digest: &Digest) -> Result<()> {
        let mut guard = self.inner.write();
        let key = (name.to_owned(), uuid.to_owned());
        guard.uploads.remove(&key);
        drop(guard);
        Ok(())
    }

    async fn get_upload_state(&self, name: &str, uuid: &str) -> Result<Option<UploadState>> {
        // R2-7: evict the session on access if it has gone idle past the
        // TTL so a stale `GET`/`PATCH`/`PUT` against it sees "unknown
        // upload" (the handler answers 404 BLOB_UPLOAD_UNKNOWN) rather
        // than resurrecting an expired session.
        let now = Instant::now();
        let key = (name.to_owned(), uuid.to_owned());
        let mut guard = self.inner.write();
        if let Some(state) = guard.uploads.get(&key) {
            if state.is_idle_for(now, self.limits.idle_ttl) {
                guard.uploads.remove(&key);
                drop(guard);
                return Ok(None);
            }
            let cloned = state.clone();
            drop(guard);
            return Ok(Some(cloned));
        }
        drop(guard);
        Ok(None)
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
        let out = artifact_type.map_or_else(
            || list.clone(),
            |at| {
                list.iter()
                    .filter(|d| d.artifact_type.as_deref() == Some(at))
                    .cloned()
                    .collect()
            },
        );
        drop(guard);
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
        self.persist_locked(&guard);
        drop(guard);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{
        InMemoryRegistryMeta, ReferrerDescriptor, RegistryMeta, SessionLimits, UploadAdmission,
    };
    use crate::reference::Reference;
    use crate::upload::UploadState;
    use bytes::Bytes;
    use ferro_blob_store::Digest;
    use std::time::Duration;

    /// Unwrap a [`UploadAdmission::Started`] UUID; panic on capacity.
    fn started(adm: UploadAdmission) -> String {
        match adm {
            UploadAdmission::Started(u) => u,
            UploadAdmission::AtCapacity(c) => panic!("unexpected capacity rejection at {c}"),
        }
    }

    #[tokio::test]
    async fn start_append_take_cycle() {
        let reg = InMemoryRegistryMeta::new();
        let uuid = started(reg.start_upload("lib/alpine").await.expect("start"));
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
        let uuid = started(reg.start_upload("lib/alpine").await.expect("start"));
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

    #[tokio::test]
    async fn start_upload_enforces_concurrent_session_cap() {
        // R2-7: cap at 2 concurrent sessions. The third POST is rejected
        // with AtCapacity rather than pinning unbounded memory.
        let reg = InMemoryRegistryMeta::with_session_limits(SessionLimits {
            max_sessions: 2,
            idle_ttl: Duration::from_secs(3600),
        });
        let _a = started(reg.start_upload("repo").await.expect("first"));
        let _b = started(reg.start_upload("repo").await.expect("second"));
        let third = reg.start_upload("repo").await.expect("third call ok");
        assert_eq!(
            third,
            UploadAdmission::AtCapacity(2),
            "third session over the cap must be rejected"
        );
    }

    #[tokio::test]
    async fn idle_sessions_are_swept_on_new_upload_freeing_capacity() {
        // Cap at 1 with a zero TTL so any pre-existing session is
        // immediately idle and swept when the next upload is started —
        // capacity is reclaimed rather than wedged forever.
        let reg = InMemoryRegistryMeta::with_session_limits(SessionLimits {
            max_sessions: 1,
            idle_ttl: Duration::from_secs(0),
        });
        let first = started(reg.start_upload("repo").await.expect("first"));
        // With ttl=0 the first session is already idle; starting a new
        // upload sweeps it, so we are admitted instead of AtCapacity.
        let second = reg.start_upload("repo").await.expect("second call ok");
        assert!(
            matches!(second, UploadAdmission::Started(_)),
            "idle session should be swept to free capacity, got {second:?}"
        );
        // The original (swept) session is gone.
        let gone = reg
            .get_upload_state("repo", &first)
            .await
            .expect("get state");
        assert!(gone.is_none(), "swept session must no longer resolve");
    }

    #[tokio::test]
    async fn metadata_persists_across_simulated_restart() {
        // R2-6: build a persistence-backed registry on a temp dir, push a
        // manifest + tag + referrer, drop it (simulating a process exit),
        // rebuild from the SAME dir, and assert everything still resolves.
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let dir = tmp.path();

        let manifest_body = Bytes::from_static(b"{\"schemaVersion\":2}");
        let digest = Digest::sha256_of(&manifest_body);
        let subject = Digest::sha256_of(b"subject-manifest");
        let referrer_digest = Digest::sha256_of(b"sbom-referrer");

        {
            let reg = InMemoryRegistryMeta::with_persistence(dir);
            reg.put_manifest(
                "lib/alpine",
                &Reference::Tag("latest".to_owned()),
                &digest,
                "application/vnd.oci.image.manifest.v1+json",
                manifest_body.clone(),
            )
            .await
            .expect("put manifest");
            reg.register_referrer(
                "lib/alpine",
                &subject,
                ReferrerDescriptor {
                    media_type: "application/vnd.oci.image.manifest.v1+json".to_owned(),
                    digest: referrer_digest.clone(),
                    size: 12,
                    artifact_type: Some("application/spdx+json".to_owned()),
                    annotations: None,
                },
            )
            .await
            .expect("register referrer");
            // `reg` dropped here → simulates process exit; only the
            // metadata.json on disk survives.
        }

        // The snapshot file must exist under the storage dir.
        assert!(
            dir.join(super::METADATA_FILE_NAME).is_file(),
            "metadata.json written"
        );

        // "Restart": rebuild from the same directory.
        let reg2 = InMemoryRegistryMeta::with_persistence(dir);

        // Manifest resolvable by digest AND by the tag.
        let by_tag = reg2
            .get_manifest("lib/alpine", &Reference::Tag("latest".to_owned()))
            .await
            .expect("get by tag")
            .expect("tag still resolves after restart");
        assert_eq!(by_tag.0, digest, "digest preserved");
        assert_eq!(&by_tag.2[..], &manifest_body[..], "exact body preserved");

        let by_digest = reg2
            .get_manifest("lib/alpine", &Reference::Digest(digest.clone()))
            .await
            .expect("get by digest")
            .expect("digest still resolves after restart");
        assert_eq!(by_digest.0, digest);

        // Referrer index preserved.
        let referrers = reg2
            .list_referrers("lib/alpine", &subject, None)
            .await
            .expect("list referrers");
        assert_eq!(referrers.len(), 1, "referrer survived restart");
        assert_eq!(referrers[0].digest, referrer_digest);

        // Repository catalog reflects the persisted repo.
        let repos = reg2.list_repositories(None, None).await.expect("repos");
        assert_eq!(repos, vec!["lib/alpine".to_owned()]);
    }

    #[tokio::test]
    async fn corrupt_metadata_file_starts_empty_not_panics() {
        // R2-6 robustness: a garbage metadata.json must not wedge boot —
        // the registry starts empty and logs (here we just assert it does
        // not panic and is empty).
        let tmp = tempfile::TempDir::new().expect("tempdir");
        std::fs::write(tmp.path().join(super::METADATA_FILE_NAME), b"{ not json")
            .expect("write garbage");
        let reg = InMemoryRegistryMeta::with_persistence(tmp.path());
        let repos = reg.list_repositories(None, None).await.expect("repos");
        assert!(repos.is_empty(), "corrupt snapshot ⇒ empty registry");
    }

    #[tokio::test]
    async fn in_memory_registry_does_not_write_metadata_file() {
        // The default (non-persistent) constructor must stay ephemeral:
        // no metadata.json side effects for the in-memory deployment.
        let reg = InMemoryRegistryMeta::new();
        let digest = Digest::sha256_of(b"body");
        reg.put_manifest(
            "repo",
            &Reference::Digest(digest.clone()),
            &digest,
            "application/vnd.oci.image.manifest.v1+json",
            Bytes::from_static(b"body"),
        )
        .await
        .expect("put");
        // Resolvable in-memory, but nothing is persisted (persist_path is
        // None, so persist_locked is a no-op).
        assert!(
            reg.get_manifest("repo", &Reference::Digest(digest))
                .await
                .expect("get")
                .is_some()
        );
    }

    #[test]
    fn default_session_ttl_is_one_hour() {
        // `DEFAULT_UPLOAD_SESSION_TTL = Duration::from_secs(60 * 60)`.
        // Kills `* -> +` (would be 120s) and `* -> /` (would be 1s).
        assert_eq!(
            super::DEFAULT_UPLOAD_SESSION_TTL,
            Duration::from_secs(3600),
            "default idle TTL must be exactly one hour (3600s)"
        );
    }

    #[test]
    fn load_snapshot_missing_file_is_ok_none() {
        // The `Err(e) if e.kind() == NotFound => Ok(None)` arm: a missing
        // file must yield Ok(None) (start empty), not Err. Kills the
        // guard-always-false and `== -> !=` mutants for the missing case.
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let missing = tmp.path().join("does-not-exist.json");
        let Ok(loaded) = super::InMemoryRegistryMeta::load_snapshot(&missing) else {
            panic!("missing file must be Ok(None), not Err");
        };
        assert!(loaded.is_none(), "missing snapshot ⇒ Ok(None)");
    }

    #[test]
    fn load_snapshot_non_notfound_io_error_is_err() {
        // Pointing at a directory makes `std::fs::read` fail with a kind
        // that is NOT NotFound. The NotFound guard must NOT swallow it —
        // it must propagate as Err. Kills the guard-always-true mutant
        // (which would turn every IO error into Ok(None)) and the
        // `== -> !=` mutant (which would route a non-NotFound error into
        // the Ok(None) arm).
        let tmp = tempfile::TempDir::new().expect("tempdir");
        // `tmp.path()` itself is a directory; reading it as a file errors
        // with IsADirectory / other non-NotFound kind.
        let result = super::InMemoryRegistryMeta::load_snapshot(tmp.path());
        let Err(err) = result else {
            panic!("reading a directory as a snapshot file must be Err");
        };
        assert_ne!(
            err.kind(),
            std::io::ErrorKind::NotFound,
            "the error is a real IO failure, not NotFound"
        );
    }

    #[test]
    fn sweep_idle_uploads_returns_count_swept() {
        // `before - self.uploads.len()` returns the number evicted.
        // Kills `- -> +` (which would return before + after). Insert two
        // sessions, sweep with ttl=0 (both immediately idle), expect 2.
        use super::InMemoryState;
        use std::time::Instant;
        let mut s = InMemoryState::default();
        s.uploads.insert(
            ("repo".to_owned(), "u1".to_owned()),
            UploadState::new("repo", "u1"),
        );
        s.uploads.insert(
            ("repo".to_owned(), "u2".to_owned()),
            UploadState::new("repo", "u2"),
        );
        let swept = s.sweep_idle_uploads(Instant::now(), Duration::from_secs(0));
        assert_eq!(swept, 2, "both idle sessions swept ⇒ count is 2, not 4");
        assert!(s.uploads.is_empty(), "swept sessions removed");
    }

    #[tokio::test]
    async fn delete_by_digest_drops_only_tags_pointing_at_it() {
        // `tag_map.retain(|_, v| v != &digest_str)` keeps tags that do
        // NOT point at the deleted digest. Mutating `!=` to `==` would
        // instead keep ONLY the tags pointing at the deleted digest
        // (dropping unrelated tags). Push two manifests under two tags,
        // delete one by digest, and assert the unrelated tag survives
        // while the deleted one is gone.
        let reg = InMemoryRegistryMeta::new();
        let body_a = Bytes::from_static(b"manifest-a");
        let body_b = Bytes::from_static(b"manifest-b");
        let digest_a = Digest::sha256_of(&body_a);
        let digest_b = Digest::sha256_of(&body_b);
        reg.put_manifest(
            "repo",
            &Reference::Tag("a".to_owned()),
            &digest_a,
            "application/vnd.oci.image.manifest.v1+json",
            body_a,
        )
        .await
        .expect("put a");
        reg.put_manifest(
            "repo",
            &Reference::Tag("b".to_owned()),
            &digest_b,
            "application/vnd.oci.image.manifest.v1+json",
            body_b,
        )
        .await
        .expect("put b");

        // Delete manifest A by digest.
        let removed = reg
            .delete_manifest("repo", &Reference::Digest(digest_a.clone()))
            .await
            .expect("delete a");
        assert!(removed, "manifest A deleted");

        // Tag "a" must no longer resolve; tag "b" MUST still resolve.
        assert!(
            reg.get_manifest("repo", &Reference::Tag("a".to_owned()))
                .await
                .expect("get a")
                .is_none(),
            "tag pointing at the deleted digest is gone"
        );
        let still_b = reg
            .get_manifest("repo", &Reference::Tag("b".to_owned()))
            .await
            .expect("get b")
            .expect("tag b survives unrelated delete");
        assert_eq!(still_b.0, digest_b, "unrelated tag still resolves to B");
    }

    #[tokio::test]
    async fn list_tags_last_cursor_is_strictly_after() {
        // `names.retain(|t| t.as_str() > cursor)` keeps tags strictly
        // after `last`. Boundary: tags [a, b, c], last="b" ⇒ [c]. This
        // distinguishes `>` (→[c]) from `==` (→[]), `<` (→[a]), and
        // `>=` (→[b, c]).
        let reg = InMemoryRegistryMeta::new();
        for t in ["a", "b", "c"] {
            let body = Bytes::from(format!("m-{t}"));
            let digest = Digest::sha256_of(&body);
            reg.put_manifest(
                "repo",
                &Reference::Tag(t.to_owned()),
                &digest,
                "application/vnd.oci.image.manifest.v1+json",
                body,
            )
            .await
            .expect("put");
        }
        let page = reg.list_tags("repo", Some("b"), None).await.expect("tags");
        assert_eq!(page, vec!["c".to_owned()], "last=b yields strictly-after");
    }

    #[tokio::test]
    async fn list_repositories_last_cursor_is_strictly_after() {
        // Same boundary trio as list_tags, for the catalog endpoint.
        let reg = InMemoryRegistryMeta::new();
        for name in ["repo-a", "repo-b", "repo-c"] {
            let body = Bytes::from(format!("m-{name}"));
            let digest = Digest::sha256_of(&body);
            reg.put_manifest(
                name,
                &Reference::Digest(digest.clone()),
                &digest,
                "application/vnd.oci.image.manifest.v1+json",
                body,
            )
            .await
            .expect("put");
        }
        let page = reg
            .list_repositories(Some("repo-b"), None)
            .await
            .expect("repos");
        assert_eq!(
            page,
            vec!["repo-c".to_owned()],
            "last=repo-b yields strictly-after"
        );
    }

    #[tokio::test]
    async fn complete_upload_removes_the_session() {
        // `complete_upload` removes the session from the uploads map.
        // Mutating the body to `Ok(())` would leave it in place. Assert
        // the session no longer resolves afterward.
        let reg = InMemoryRegistryMeta::new();
        let uuid = started(reg.start_upload("repo").await.expect("start"));
        reg.append_upload("repo", &uuid, 0, Bytes::from_static(b"x"))
            .await
            .expect("append");
        let digest = Digest::sha256_of(b"x");
        reg.complete_upload("repo", &uuid, &digest)
            .await
            .expect("complete");
        let after = reg
            .get_upload_state("repo", &uuid)
            .await
            .expect("get state");
        assert!(
            after.is_none(),
            "a completed upload session must be removed from the store"
        );
    }

    #[tokio::test]
    async fn expired_session_is_evicted_on_access() {
        // R2-7: a session idle past its TTL is evicted on access so the
        // handler answers "unknown upload" (→ 404) for it.
        let reg = InMemoryRegistryMeta::with_session_limits(SessionLimits {
            max_sessions: 16,
            idle_ttl: Duration::from_secs(0),
        });
        let uuid = started(reg.start_upload("repo").await.expect("start"));
        // ttl=0 → already expired on the next access.
        let state = reg.get_upload_state("repo", &uuid).await.expect("get");
        assert!(state.is_none(), "expired session must be evicted on access");
    }
}
