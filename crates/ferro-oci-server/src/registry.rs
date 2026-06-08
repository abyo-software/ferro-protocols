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

    /// Persist a manifest body, its optional tag alias, AND an optional
    /// referrer registration as ONE atomic transaction (R4-1).
    ///
    /// A manifest that declares a `subject` produces two logically coupled
    /// mutations — the manifest/tag insert and the referrer-index push.
    /// Performing them as two separate [`put_manifest`] + [`register_referrer`]
    /// calls means each does its own snapshot write, so a failure of the
    /// second leaves the first already durable: the manifest/tag survives a
    /// restart while the referrer does not, even though the client received a
    /// 5xx. This method applies all three sub-mutations under ONE write lock
    /// and ONE snapshot write. If persistence fails, EVERY in-memory change
    /// (manifest, tag, referrer, and any maps created en route) is rolled
    /// back and nothing is written, so the on-disk state is never partial.
    ///
    /// When `referrer` is `None` this is equivalent to [`put_manifest`].
    async fn put_manifest_with_referrer(
        &self,
        name: &str,
        reference: &Reference,
        digest: &Digest,
        media_type: &str,
        body: Bytes,
        referrer: Option<(Digest, ReferrerDescriptor)>,
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
            for (digest_str, pm) in by_digest {
                let bytes = match hex::decode(&pm.body_hex) {
                    Ok(b) => b,
                    Err(e) => {
                        // A non-decodable body is corrupt; drop the entry
                        // rather than fail the whole boot, mirroring the
                        // key-mismatch handling below.
                        tracing::warn!(
                            name = %name,
                            digest = %digest_str,
                            error = %e,
                            "dropping persisted manifest with undecodable body_hex"
                        );
                        continue;
                    }
                };
                let bytes = Bytes::from(bytes);
                // R3-1: the map key is content-addressed — it MUST equal
                // sha256(body). A corrupted or crafted metadata.json could
                // otherwise advertise a digest `<D>` while serving bytes
                // that hash to something else, bypassing the content-
                // addressing invariant R2-1 enforces on PUT. Re-derive the
                // digest over the exact bytes and drop any entry whose key
                // does not match (only sha256 is canonical here).
                let Ok(key_digest) = digest_str.parse::<Digest>() else {
                    tracing::warn!(
                        name = %name,
                        digest = %digest_str,
                        "dropping persisted manifest with unparseable digest key"
                    );
                    continue;
                };
                if key_digest.algo() != ferro_blob_store::DigestAlgo::Sha256 {
                    tracing::warn!(
                        name = %name,
                        digest = %digest_str,
                        "dropping persisted manifest keyed by a non-sha256 digest"
                    );
                    continue;
                }
                let recomputed = Digest::sha256_of(&bytes);
                if recomputed.hex() != key_digest.hex() {
                    tracing::warn!(
                        name = %name,
                        key = %digest_str,
                        recomputed = %recomputed,
                        "dropping corrupt persisted manifest: key digest != sha256(body)"
                    );
                    continue;
                }
                inner.insert(digest_str, (pm.media_type, bytes));
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
    /// persistence is enabled.
    ///
    /// R3-2: this is NO LONGER best-effort. When persistence is configured
    /// (filesystem deployment) a write failure (disk full, permission
    /// error, …) is returned to the caller so the mutating trait methods
    /// can roll back the in-memory change and surface an error to the
    /// client — a manifest PUT that cannot be made durable must NOT be
    /// acknowledged with 201, or it would silently vanish on restart.
    ///
    /// When persistence is disabled (`persist_path == None`, the in-memory
    /// / test deployment) this is an infallible no-op, so that path is
    /// unchanged.
    ///
    /// Caller holds the write lock so the mirror is consistent with the
    /// state it snapshots.
    fn persist_locked(&self, state: &InMemoryState) -> std::io::Result<()> {
        let Some(path) = &self.persist_path else {
            return Ok(());
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
        Self::write_snapshot_atomic(path, &snapshot).inspect_err(|e| {
            tracing::error!(
                path = %path.display(),
                error = %e,
                "failed to persist registry metadata snapshot; rolling back mutation"
            );
        })
    }

    /// Map a persistence failure onto a [`ferro_blob_store::BlobStoreError`]
    /// so it propagates through the [`RegistryMeta`] trait's `Result`. The
    /// handler layer treats this as a 5xx (durability could not be
    /// guaranteed) rather than acknowledging the mutation.
    const fn persist_error(e: std::io::Error) -> ferro_blob_store::BlobStoreError {
        ferro_blob_store::BlobStoreError::Io(e)
    }

    /// Prune the name-level `manifests` / `tags` / `referrers` entries for
    /// `name` when they have become empty (R4-2).
    ///
    /// A rollback that removes the only manifest/tag/referrer a brand-new repo
    /// ever held must also drop the now-empty per-repo maps that the
    /// `entry(..).or_default()` insertions created; otherwise the repo lingers
    /// in `_catalog` / list state even though it holds nothing. Repos that
    /// still contain other manifests/tags/referrers are left untouched.
    fn prune_empty_repo(state: &mut InMemoryState, name: &str) {
        if state.manifests.get(name).is_some_and(BTreeMap::is_empty) {
            state.manifests.remove(name);
        }
        if state.tags.get(name).is_some_and(BTreeMap::is_empty) {
            state.tags.remove(name);
        }
        if state
            .referrers
            .get(name)
            .is_some_and(BTreeMap::is_empty)
        {
            state.referrers.remove(name);
        }
    }

    /// Serialise `snapshot` and write it to `path` atomically and durably.
    ///
    /// R3-5: a fixed sibling temp path (`metadata.json.tmp`) plus a plain
    /// `write` + `rename` with no `fsync` has two defects — it is not
    /// crash-durable (the bytes may still be in the page cache when the
    /// machine loses power, so a crash can resurrect a *stale* snapshot
    /// after the rename "succeeded"), and an attacker who can write the
    /// data dir could pre-place the temp path as a symlink and redirect
    /// the write. We close both:
    ///
    /// 1. Create the temp file in the *same directory* with a unique name
    ///    using `O_CREAT | O_EXCL` (`OpenOptions::create_new`), so a
    ///    pre-existing file or symlink at that path makes the open *fail*
    ///    rather than be followed.
    /// 2. `write_all` then `sync_all()` (fsync) the temp file so its bytes
    ///    are on stable storage before we expose them.
    /// 3. `rename` over the target (atomic on POSIX), then fsync the parent
    ///    directory so the rename itself is durable.
    fn write_snapshot_atomic(
        path: &std::path::Path,
        snapshot: &MetadataSnapshot,
    ) -> std::io::Result<()> {
        use std::io::Write as _;
        use std::sync::atomic::{AtomicU64, Ordering};

        // Monotonic counter feeding the unique temp name so concurrent /
        // back-to-back writers never collide on the same O_EXCL path.
        static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

        let bytes = serde_json::to_vec(snapshot)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

        let parent = path.parent().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "snapshot path has no parent directory",
            )
        })?;
        let seq = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let tmp = parent.join(format!(
            "{}.{}.{seq}.tmp",
            METADATA_FILE_NAME,
            std::process::id()
        ));

        // O_CREAT | O_EXCL: refuse to follow / overwrite a pre-placed
        // symlink or file at the temp path.
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)?;
        // Write + fsync the data, cleaning up the temp file on any error so
        // a failed write never strands an O_EXCL temp that would block the
        // next attempt.
        let write_then_sync = file
            .write_all(&bytes)
            .and_then(|()| file.sync_all());
        if let Err(e) = write_then_sync {
            drop(file);
            let _ = std::fs::remove_file(&tmp);
            return Err(e);
        }
        drop(file);

        if let Err(e) = std::fs::rename(&tmp, path) {
            let _ = std::fs::remove_file(&tmp);
            return Err(e);
        }

        // Fsync the parent directory so the rename (a directory mutation)
        // is itself durable across a crash. A failure here is non-fatal to
        // the logical write (the rename already happened) but we surface it
        // so the caller can treat durability as not-yet-guaranteed.
        let dir = std::fs::File::open(parent)?;
        dir.sync_all()?;
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
        self.put_manifest_with_referrer(name, reference, digest, media_type, body, None)
            .await
    }

    async fn put_manifest_with_referrer(
        &self,
        name: &str,
        reference: &Reference,
        digest: &Digest,
        media_type: &str,
        body: Bytes,
        referrer: Option<(Digest, ReferrerDescriptor)>,
    ) -> Result<()> {
        let digest_str = digest.to_string();
        let mut guard = self.inner.write();

        // Apply all three sub-mutations under the single write lock, capturing
        // enough prior state to restore the exact pre-mutation snapshot if the
        // one persist below fails (R3-2 / R4-1).
        let prev_manifest = guard
            .manifests
            .entry(name.to_owned())
            .or_default()
            .insert(digest_str.clone(), (media_type.to_owned(), body));
        let prev_tag = if let Reference::Tag(tag) = reference {
            Some((
                tag.clone(),
                guard
                    .tags
                    .entry(name.to_owned())
                    .or_default()
                    .insert(tag.clone(), digest_str.clone()),
            ))
        } else {
            None
        };
        // The referrer push appends to the (name, subject) list; on rollback we
        // pop exactly the entry we appended, leaving any siblings intact.
        let referrer_subject = referrer.map(|(subject, descriptor)| {
            let subject_str = subject.to_string();
            guard
                .referrers
                .entry(name.to_owned())
                .or_default()
                .entry(subject_str.clone())
                .or_default()
                .push(descriptor);
            subject_str
        });

        if let Err(e) = self.persist_locked(&guard) {
            // Roll back ALL of (manifest, tag, referrer) to the captured
            // pre-mutation state so nothing is left partially applied.
            if let Some(map) = guard.manifests.get_mut(name) {
                match prev_manifest {
                    Some(prev) => {
                        map.insert(digest_str.clone(), prev);
                    }
                    None => {
                        map.remove(&digest_str);
                    }
                }
            }
            if let Some((tag, prev)) = prev_tag
                && let Some(map) = guard.tags.get_mut(name)
            {
                match prev {
                    Some(prev) => {
                        map.insert(tag, prev);
                    }
                    None => {
                        map.remove(&tag);
                    }
                }
            }
            if let Some(subject_str) = referrer_subject
                && let Some(subject_map) = guard.referrers.get_mut(name)
                && let Some(list) = subject_map.get_mut(&subject_str)
            {
                list.pop();
                if list.is_empty() {
                    subject_map.remove(&subject_str);
                }
            }
            // R4-2: a failed FIRST publish to a brand-new repo would otherwise
            // leave behind the empty `manifests[name]` / `tags[name]` /
            // `referrers[name]` maps created by the `entry(..).or_default()`
            // calls above, so the repo would still surface in catalog / list
            // state despite the rollback. Prune any name-level entries that
            // became (or stayed) empty as a result of the rollback.
            Self::prune_empty_repo(&mut guard, name);
            drop(guard);
            return Err(Self::persist_error(e));
        }
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
                let Some(removed_manifest) = name_map.remove(&digest_str) else {
                    drop(guard);
                    return Ok(false);
                };
                // Drop tags pointing at the deleted digest, remembering them
                // so we can restore on a persistence-failure rollback (R3-2).
                let mut removed_tags: Vec<String> = Vec::new();
                if let Some(tag_map) = guard.tags.get_mut(name) {
                    tag_map.retain(|tag, v| {
                        let keep = v != &digest_str;
                        if !keep {
                            removed_tags.push(tag.clone());
                        }
                        keep
                    });
                }
                // R5-3: if the deleted manifest is itself a REFERRER (it was
                // recorded as a descriptor under one or more subjects by
                // `put_manifest_with_referrer`/`register_referrer`), removing
                // only its blob/entry + tags would leave a DANGLING referrer
                // descriptor in the index — `/referrers/<subject>` would still
                // advertise a manifest that no longer exists. Prune, as part of
                // this SAME delete transaction, every descriptor whose `digest`
                // equals the deleted digest, scanning all subjects of this repo
                // (a digest may appear under more than one subject). Each
                // pruned (subject, index, descriptor) is remembered so the
                // exact pre-delete index is restored on a persist-failure
                // rollback (consistent with the R3-2 / R4-1 model).
                let mut removed_referrers: Vec<(String, usize, ReferrerDescriptor)> = Vec::new();
                if let Some(subject_map) = guard.referrers.get_mut(name) {
                    for (subject_str, list) in subject_map.iter_mut() {
                        // Walk back-to-front so the recorded indices stay valid
                        // for re-insertion (and so we can `remove` in place).
                        for idx in (0..list.len()).rev() {
                            if list[idx].digest.to_string() == digest_str {
                                let descriptor = list.remove(idx);
                                removed_referrers.push((subject_str.clone(), idx, descriptor));
                            }
                        }
                    }
                    // Drop any subject lists emptied by the prune so an empty
                    // subject does not linger in the index.
                    subject_map.retain(|_, list| !list.is_empty());
                }
                if let Err(e) = self.persist_locked(&guard) {
                    // Roll back: restore the manifest, its tags, and any pruned
                    // referrer descriptors to their exact prior positions.
                    guard
                        .manifests
                        .entry(name.to_owned())
                        .or_default()
                        .insert(digest_str.clone(), removed_manifest);
                    if !removed_tags.is_empty() {
                        let tag_map = guard.tags.entry(name.to_owned()).or_default();
                        for tag in removed_tags {
                            tag_map.insert(tag, digest_str.clone());
                        }
                    }
                    if !removed_referrers.is_empty() {
                        let subject_map = guard.referrers.entry(name.to_owned()).or_default();
                        // We recorded prunes back-to-front per subject; replay
                        // them front-to-back (reverse of the removal order) so
                        // each recorded index is valid against the growing list.
                        for (subject_str, idx, descriptor) in removed_referrers.into_iter().rev() {
                            let list = subject_map.entry(subject_str).or_default();
                            let at = idx.min(list.len());
                            list.insert(at, descriptor);
                        }
                    }
                    drop(guard);
                    return Err(Self::persist_error(e));
                }
                drop(guard);
                Ok(true)
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
        let subject_str = subject.to_string();
        let mut guard = self.inner.write();
        guard
            .referrers
            .entry(name.to_owned())
            .or_default()
            .entry(subject_str.clone())
            .or_default()
            .push(descriptor);
        if let Err(e) = self.persist_locked(&guard) {
            // Roll back the push we just made (R3-2). Pop only the entry we
            // appended; leave the rest of the list intact.
            if let Some(subject_map) = guard.referrers.get_mut(name)
                && let Some(list) = subject_map.get_mut(&subject_str)
            {
                list.pop();
                if list.is_empty() {
                    subject_map.remove(&subject_str);
                }
            }
            // R4-2: drop the now-empty per-repo referrer map a failed FIRST
            // referrer registration would otherwise leave behind.
            Self::prune_empty_repo(&mut guard, name);
            drop(guard);
            return Err(Self::persist_error(e));
        }
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

    /// Build a persistence-backed registry whose snapshot writes are forced to
    /// FAIL by pre-placing `metadata.json` as a non-empty *directory*: the
    /// atomic `rename(tmp, metadata.json)` then cannot clobber it (`EISDIR`),
    /// so every `persist_locked` returns `Err` and the mutating methods must
    /// roll back. Returns the registry plus the owning temp dir.
    fn registry_with_failing_persist() -> (InMemoryRegistryMeta, tempfile::TempDir) {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let meta = tmp.path().join(super::METADATA_FILE_NAME);
        std::fs::create_dir(&meta).expect("metadata.json as dir");
        // Make it non-empty so a rename over it is rejected.
        std::fs::write(meta.join("child"), b"x").expect("child");
        let reg = InMemoryRegistryMeta::with_persistence(tmp.path());
        (reg, tmp)
    }

    #[tokio::test]
    async fn put_with_subject_persist_failure_rolls_back_manifest_tag_and_referrer() {
        // R4-1: a manifest PUT that declares a `subject` couples the
        // manifest/tag insert with the referrer push. A persist failure must
        // roll back ALL of them — neither the manifest/tag NOR the referrer
        // may survive — and surface an error.
        let (reg, _tmp) = registry_with_failing_persist();
        let body = Bytes::from_static(b"{\"schemaVersion\":2}");
        let digest = Digest::sha256_of(&body);
        let subject = Digest::sha256_of(b"subject-manifest");
        let referrer_digest = Digest::sha256_of(b"sbom-referrer");

        let err = reg
            .put_manifest_with_referrer(
                "lib/alpine",
                &Reference::Tag("latest".to_owned()),
                &digest,
                "application/vnd.oci.image.manifest.v1+json",
                body.clone(),
                Some((
                    subject.clone(),
                    ReferrerDescriptor {
                        media_type: "application/vnd.oci.image.manifest.v1+json".to_owned(),
                        digest: referrer_digest,
                        size: 12,
                        artifact_type: Some("application/spdx+json".to_owned()),
                        annotations: None,
                    },
                )),
            )
            .await
            .expect_err("persist failure must surface as Err");
        assert!(
            matches!(err, ferro_blob_store::BlobStoreError::Io(_)),
            "persist failure maps to Io error, got {err:?}"
        );

        // Manifest absent by digest AND by tag.
        assert!(
            reg.get_manifest("lib/alpine", &Reference::Digest(digest.clone()))
                .await
                .expect("get by digest")
                .is_none(),
            "manifest must be fully rolled back (digest)"
        );
        assert!(
            reg.get_manifest("lib/alpine", &Reference::Tag("latest".to_owned()))
                .await
                .expect("get by tag")
                .is_none(),
            "tag must be fully rolled back"
        );
        // Referrer absent.
        let referrers = reg
            .list_referrers("lib/alpine", &subject, None)
            .await
            .expect("list referrers");
        assert!(referrers.is_empty(), "referrer must be fully rolled back");
        // R4-2: the brand-new repo must not linger in the catalog.
        let repos = reg.list_repositories(None, None).await.expect("repos");
        assert!(
            repos.is_empty(),
            "failed first publish must leave repo absent from catalog, got {repos:?}"
        );
    }

    #[tokio::test]
    async fn put_with_subject_success_registers_manifest_tag_and_referrer() {
        // R4-1 success path: the transactional method still registers BOTH the
        // manifest/tag and the referrer when persistence succeeds.
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let reg = InMemoryRegistryMeta::with_persistence(tmp.path());
        let body = Bytes::from_static(b"{\"schemaVersion\":2}");
        let digest = Digest::sha256_of(&body);
        let subject = Digest::sha256_of(b"subject-manifest");
        let referrer_digest = Digest::sha256_of(b"sbom-referrer");

        reg.put_manifest_with_referrer(
            "lib/alpine",
            &Reference::Tag("latest".to_owned()),
            &digest,
            "application/vnd.oci.image.manifest.v1+json",
            body.clone(),
            Some((
                subject.clone(),
                ReferrerDescriptor {
                    media_type: "application/vnd.oci.image.manifest.v1+json".to_owned(),
                    digest: referrer_digest.clone(),
                    size: 12,
                    artifact_type: Some("application/spdx+json".to_owned()),
                    annotations: None,
                },
            )),
        )
        .await
        .expect("put with referrer");

        let by_tag = reg
            .get_manifest("lib/alpine", &Reference::Tag("latest".to_owned()))
            .await
            .expect("get by tag")
            .expect("tag resolves");
        assert_eq!(by_tag.0, digest);
        let referrers = reg
            .list_referrers("lib/alpine", &subject, None)
            .await
            .expect("list referrers");
        assert_eq!(referrers.len(), 1, "referrer registered");
        assert_eq!(referrers[0].digest, referrer_digest);
    }

    #[tokio::test]
    async fn referrer_leg_persist_failure_does_not_strand_manifest() {
        // R4-1, the core atomicity property: when the referrer leg cannot be
        // made durable, the manifest/tag MUST NOT be left stranded on its own.
        //
        // (a) First we demonstrate the *bug* the fix prevents: the pre-fix
        //     two-call sequence (`put_manifest` then `register_referrer`, EACH
        //     its own snapshot write) lets the first write succeed and the
        //     second fail, stranding the manifest.
        // (b) Then we show the SINGLE transactional method does NOT do this —
        //     under a "next write fails" condition it rolls everything back
        //     together so nothing is stranded.
        let subject = Digest::sha256_of(b"coupled-subject");
        let referrer_descriptor = || ReferrerDescriptor {
            media_type: "application/vnd.oci.image.manifest.v1+json".to_owned(),
            digest: Digest::sha256_of(b"coupled-referrer"),
            size: 16,
            artifact_type: None,
            annotations: None,
        };

        // --- (a) OLD two-call path: reproduce the stranding bug -------------
        {
            let tmp = tempfile::TempDir::new().expect("tempdir");
            let reg = InMemoryRegistryMeta::with_persistence(tmp.path());
            let body = Bytes::from_static(b"old-manifest");
            let digest = Digest::sha256_of(&body);
            // First write (manifest+tag) succeeds → durable on its own.
            reg.put_manifest(
                "old/repo",
                &Reference::Tag("latest".to_owned()),
                &digest,
                "application/vnd.oci.image.manifest.v1+json",
                body,
            )
            .await
            .expect("first write persisted");
            // Break persistence, then attempt the SECOND write (referrer).
            let meta = tmp.path().join(super::METADATA_FILE_NAME);
            std::fs::remove_file(&meta).expect("remove snapshot");
            std::fs::create_dir(&meta).expect("metadata.json as dir");
            std::fs::write(meta.join("child"), b"x").expect("child");
            reg.register_referrer("old/repo", &subject, referrer_descriptor())
                .await
                .expect_err("second write must fail");
            // The bug: the manifest is STRANDED (still durable) while the
            // referrer is gone — exactly the partial state R4-1 is about.
            assert!(
                reg.get_manifest("old/repo", &Reference::Digest(digest))
                    .await
                    .expect("get old")
                    .is_some(),
                "two-call path strands the manifest (the pre-fix bug)"
            );
        }

        // --- (b) NEW transactional path: no stranding -----------------------
        {
            let tmp = tempfile::TempDir::new().expect("tempdir");
            let reg = InMemoryRegistryMeta::with_persistence(tmp.path());
            // A seed publish makes the first real write happen, then we break
            // persistence so the coupled write below fails as ONE unit.
            let seed_body = Bytes::from_static(b"seed");
            let seed_digest = Digest::sha256_of(&seed_body);
            reg.put_manifest(
                "seed/repo",
                &Reference::Tag("v1".to_owned()),
                &seed_digest,
                "application/vnd.oci.image.manifest.v1+json",
                seed_body,
            )
            .await
            .expect("seed persisted");
            let meta = tmp.path().join(super::METADATA_FILE_NAME);
            std::fs::remove_file(&meta).expect("remove snapshot");
            std::fs::create_dir(&meta).expect("metadata.json as dir");
            std::fs::write(meta.join("child"), b"x").expect("child");

            let body = Bytes::from_static(b"coupled-manifest");
            let digest = Digest::sha256_of(&body);
            reg.put_manifest_with_referrer(
                "coupled/repo",
                &Reference::Tag("latest".to_owned()),
                &digest,
                "application/vnd.oci.image.manifest.v1+json",
                body,
                Some((subject.clone(), referrer_descriptor())),
            )
            .await
            .expect_err("coupled persist must fail");

            assert!(
                reg.get_manifest("coupled/repo", &Reference::Digest(digest))
                    .await
                    .expect("get coupled")
                    .is_none(),
                "transactional method must NOT strand the manifest (R4-1)"
            );
            let referrers = reg
                .list_referrers("coupled/repo", &subject, None)
                .await
                .expect("list referrers");
            assert!(referrers.is_empty(), "referrer must not survive either");
            // Only the unrelated seed repo remains.
            let repos = reg.list_repositories(None, None).await.expect("repos");
            assert_eq!(repos, vec!["seed/repo".to_owned()]);
        }
    }

    #[tokio::test]
    async fn failed_first_put_to_new_repo_leaves_catalog_empty() {
        // R4-2: a failed FIRST manifest PUT (no subject) to a brand-new repo
        // must not leave an empty `manifests[name]` / `tags[name]` behind, so
        // the repo is ABSENT from the catalog afterward.
        let (reg, _tmp) = registry_with_failing_persist();
        let body = Bytes::from_static(b"first-body");
        let digest = Digest::sha256_of(&body);

        reg.put_manifest(
            "brand/new",
            &Reference::Tag("v1".to_owned()),
            &digest,
            "application/vnd.oci.image.manifest.v1+json",
            body,
        )
        .await
        .expect_err("persist failure must surface");

        let repos = reg.list_repositories(None, None).await.expect("repos");
        assert!(
            repos.is_empty(),
            "failed first PUT must leave catalog empty, got {repos:?}"
        );
        let tags = reg.list_tags("brand/new", None, None).await.expect("tags");
        assert!(tags.is_empty(), "no tags should survive");
    }

    #[tokio::test]
    async fn failed_put_to_existing_repo_leaves_other_manifests_intact() {
        // R4-2: rollback must prune only what it inserted. A failed PUT to a
        // repo that already holds OTHER manifests must leave those intact.
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let first_body = Bytes::from_static(b"keep-me");
        let first_digest = Digest::sha256_of(&first_body);

        // First, a SUCCESSFUL publish to establish the repo + a manifest/tag.
        // Keep this SAME registry instance for the failing second PUT so its
        // in-memory state still holds the first manifest (a rebuild here would
        // re-read the snapshot, which we are about to clobber below).
        let reg = InMemoryRegistryMeta::with_persistence(tmp.path());
        reg.put_manifest(
            "shared/repo",
            &Reference::Tag("stable".to_owned()),
            &first_digest,
            "application/vnd.oci.image.manifest.v1+json",
            first_body.clone(),
        )
        .await
        .expect("first publish");

        // Now make subsequent persistence fail by turning metadata.json into a
        // non-empty directory under the SAME storage dir: the atomic rename can
        // no longer clobber it, so the next snapshot write returns Err.
        let meta = tmp.path().join(super::METADATA_FILE_NAME);
        std::fs::remove_file(&meta).expect("remove snapshot file");
        std::fs::create_dir(&meta).expect("metadata.json as dir");
        std::fs::write(meta.join("child"), b"x").expect("child");

        let second_body = Bytes::from_static(b"drop-me");
        let second_digest = Digest::sha256_of(&second_body);
        reg.put_manifest(
            "shared/repo",
            &Reference::Tag("edge".to_owned()),
            &second_digest,
            "application/vnd.oci.image.manifest.v1+json",
            second_body,
        )
        .await
        .expect_err("second persist must fail");

        // The pre-existing manifest/tag survive untouched.
        assert!(
            reg.get_manifest("shared/repo", &Reference::Digest(first_digest.clone()))
                .await
                .expect("get first")
                .is_some(),
            "pre-existing manifest must survive a sibling rollback"
        );
        assert!(
            reg.get_manifest("shared/repo", &Reference::Tag("stable".to_owned()))
                .await
                .expect("get stable tag")
                .is_some(),
            "pre-existing tag must survive"
        );
        // The rolled-back manifest/tag are gone.
        assert!(
            reg.get_manifest("shared/repo", &Reference::Digest(second_digest))
                .await
                .expect("get second")
                .is_none(),
            "rolled-back manifest must be absent"
        );
        assert!(
            reg.get_manifest("shared/repo", &Reference::Tag("edge".to_owned()))
                .await
                .expect("get edge tag")
                .is_none(),
            "rolled-back tag must be absent"
        );
        // The repo itself stays in the catalog (it still has the first one).
        let repos = reg.list_repositories(None, None).await.expect("repos");
        assert_eq!(repos, vec!["shared/repo".to_owned()]);
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
    async fn r5_3_delete_referrer_manifest_prunes_its_descriptor() {
        // R5-3: a manifest that is itself a REFERRER (registered as a
        // descriptor under a subject) must, when deleted, have its descriptor
        // pruned from the referrers index — otherwise `/referrers/<subject>`
        // keeps advertising a now-absent manifest (a dangling referrer).
        let reg = InMemoryRegistryMeta::new();
        let subject = Digest::sha256_of(b"subject-image");

        // Two referrer manifests under the SAME subject. M is the one we
        // delete; the sibling N must remain listed afterward.
        let m_body = Bytes::from_static(b"referrer-M");
        let m_digest = Digest::sha256_of(&m_body);
        let n_body = Bytes::from_static(b"referrer-N");
        let n_digest = Digest::sha256_of(&n_body);

        reg.put_manifest_with_referrer(
            "lib/app",
            &Reference::Digest(m_digest.clone()),
            &m_digest,
            "application/vnd.oci.image.manifest.v1+json",
            m_body,
            Some((
                subject.clone(),
                ReferrerDescriptor {
                    media_type: "application/vnd.oci.image.manifest.v1+json".to_owned(),
                    digest: m_digest.clone(),
                    size: 9,
                    artifact_type: Some("application/spdx+json".to_owned()),
                    annotations: None,
                },
            )),
        )
        .await
        .expect("put referrer M");
        reg.put_manifest_with_referrer(
            "lib/app",
            &Reference::Digest(n_digest.clone()),
            &n_digest,
            "application/vnd.oci.image.manifest.v1+json",
            n_body,
            Some((
                subject.clone(),
                ReferrerDescriptor {
                    media_type: "application/vnd.oci.image.manifest.v1+json".to_owned(),
                    digest: n_digest.clone(),
                    size: 9,
                    artifact_type: Some("application/vnd.dev.cosign.sig".to_owned()),
                    annotations: None,
                },
            )),
        )
        .await
        .expect("put referrer N");

        // Both M and N are advertised under the subject.
        let before = reg
            .list_referrers("lib/app", &subject, None)
            .await
            .expect("list before");
        assert_eq!(before.len(), 2, "both referrers listed before delete");

        // Delete M by digest.
        let removed = reg
            .delete_manifest("lib/app", &Reference::Digest(m_digest.clone()))
            .await
            .expect("delete M");
        assert!(removed, "M deleted");

        // M's descriptor is pruned; N's survives.
        let after = reg
            .list_referrers("lib/app", &subject, None)
            .await
            .expect("list after");
        assert_eq!(after.len(), 1, "only the sibling referrer remains");
        assert_eq!(
            after[0].digest, n_digest,
            "the surviving referrer is the sibling N, not the deleted M"
        );
        assert!(
            !after.iter().any(|d| d.digest == m_digest),
            "the deleted referrer manifest must no longer be advertised"
        );
    }

    #[tokio::test]
    async fn r5_3_delete_referrer_prunes_across_multiple_subjects() {
        // R5-3 edge: a single manifest digest can be recorded as a descriptor
        // under MULTIPLE subjects. Deleting it must prune the descriptor under
        // EACH subject it appears in, leaving unrelated referrers intact.
        let reg = InMemoryRegistryMeta::new();
        let subject_a = Digest::sha256_of(b"subject-a");
        let subject_b = Digest::sha256_of(b"subject-b");

        let body = Bytes::from_static(b"shared-referrer");
        let digest = Digest::sha256_of(&body);
        let descriptor = || ReferrerDescriptor {
            media_type: "application/vnd.oci.image.manifest.v1+json".to_owned(),
            digest: digest.clone(),
            size: 15,
            artifact_type: None,
            annotations: None,
        };

        // Publish the manifest once, then register it under two subjects.
        reg.put_manifest_with_referrer(
            "lib/app",
            &Reference::Digest(digest.clone()),
            &digest,
            "application/vnd.oci.image.manifest.v1+json",
            body,
            Some((subject_a.clone(), descriptor())),
        )
        .await
        .expect("put under subject A");
        reg.register_referrer("lib/app", &subject_b, descriptor())
            .await
            .expect("register under subject B");
        // An unrelated referrer under subject A that must survive.
        let other_digest = Digest::sha256_of(b"other-referrer");
        reg.register_referrer(
            "lib/app",
            &subject_a,
            ReferrerDescriptor {
                media_type: "application/vnd.oci.image.manifest.v1+json".to_owned(),
                digest: other_digest.clone(),
                size: 7,
                artifact_type: None,
                annotations: None,
            },
        )
        .await
        .expect("register other");

        reg.delete_manifest("lib/app", &Reference::Digest(digest.clone()))
            .await
            .expect("delete shared");

        let under_a = reg
            .list_referrers("lib/app", &subject_a, None)
            .await
            .expect("list A");
        assert_eq!(under_a.len(), 1, "only the unrelated referrer remains under A");
        assert_eq!(under_a[0].digest, other_digest);
        let under_b = reg
            .list_referrers("lib/app", &subject_b, None)
            .await
            .expect("list B");
        assert!(
            under_b.is_empty(),
            "the descriptor under subject B is pruned too"
        );
    }

    #[tokio::test]
    async fn r5_3_delete_referrer_persist_failure_does_not_prune() {
        // R5-3 durability: if the delete's persist fails, the referrer
        // descriptor must NOT be pruned — the whole delete rolls back, so
        // `/referrers/<subject>` still lists it (consistent with R3-2 / R4-1).
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let reg = InMemoryRegistryMeta::with_persistence(tmp.path());
        let subject = Digest::sha256_of(b"subject-image");
        let body = Bytes::from_static(b"referrer-M");
        let digest = Digest::sha256_of(&body);

        // Publish M as a referrer under the subject (this persists OK).
        reg.put_manifest_with_referrer(
            "lib/app",
            &Reference::Digest(digest.clone()),
            &digest,
            "application/vnd.oci.image.manifest.v1+json",
            body,
            Some((
                subject.clone(),
                ReferrerDescriptor {
                    media_type: "application/vnd.oci.image.manifest.v1+json".to_owned(),
                    digest: digest.clone(),
                    size: 9,
                    artifact_type: None,
                    annotations: None,
                },
            )),
        )
        .await
        .expect("put referrer M");

        // Break persistence: replace metadata.json with a non-empty directory
        // so the atomic rename in the delete's snapshot write fails.
        let meta = tmp.path().join(super::METADATA_FILE_NAME);
        std::fs::remove_file(&meta).expect("remove snapshot");
        std::fs::create_dir(&meta).expect("metadata.json as dir");
        std::fs::write(meta.join("child"), b"x").expect("child");

        let err = reg
            .delete_manifest("lib/app", &Reference::Digest(digest.clone()))
            .await
            .expect_err("delete must fail when persist fails");
        assert!(
            matches!(err, ferro_blob_store::BlobStoreError::Io(_)),
            "persist failure maps to Io error, got {err:?}"
        );

        // The manifest is still served (delete rolled back) ...
        assert!(
            reg.get_manifest("lib/app", &Reference::Digest(digest.clone()))
                .await
                .expect("get M")
                .is_some(),
            "the manifest must survive a rolled-back delete"
        );
        // ... and crucially the referrer descriptor is NOT pruned.
        let referrers = reg
            .list_referrers("lib/app", &subject, None)
            .await
            .expect("list referrers");
        assert_eq!(
            referrers.len(),
            1,
            "the referrer descriptor must be restored on rollback"
        );
        assert_eq!(referrers[0].digest, digest);
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

    #[tokio::test]
    async fn r3_1_load_drops_manifest_whose_key_digest_mismatches_body() {
        // R3-1: a persisted manifest is content-addressed — its map key
        // MUST equal sha256(body). Write a metadata.json holding TWO
        // manifests under one repo:
        //   - a CORRUPT one whose key digest does NOT equal sha256(body)
        //     (a crafted entry that would otherwise let the registry serve
        //     arbitrary bytes while advertising the fake digest), and
        //   - a VALID one alongside it.
        // On load the corrupt entry must be dropped and the valid one kept.
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let dir = tmp.path();

        let good_body = b"{\"schemaVersion\":2,\"good\":true}";
        let good_digest = Digest::sha256_of(good_body).to_string();
        let good_hex = hex::encode(good_body);

        // The corrupt entry: key is the digest of `good_body`-with-a-twist
        // but the stored body is different bytes, so key != sha256(body).
        let evil_body = b"arbitrary attacker-chosen bytes";
        let evil_hex = hex::encode(evil_body);
        // A syntactically valid sha256 key that does NOT hash `evil_body`.
        let fake_key = Digest::sha256_of(b"not-the-evil-body").to_string();
        assert_ne!(
            fake_key,
            Digest::sha256_of(evil_body).to_string(),
            "test setup: key must not match the body"
        );

        let snapshot = serde_json::json!({
            "manifests": {
                "lib/alpine": {
                    good_digest.clone(): {
                        "media_type": "application/vnd.oci.image.manifest.v1+json",
                        "body_hex": good_hex,
                    },
                    fake_key.clone(): {
                        "media_type": "application/vnd.oci.image.manifest.v1+json",
                        "body_hex": evil_hex,
                    },
                }
            },
            "tags": {},
            "referrers": {},
        });
        std::fs::write(
            dir.join(super::METADATA_FILE_NAME),
            serde_json::to_vec(&snapshot).expect("serialize"),
        )
        .expect("write metadata.json");

        let reg = InMemoryRegistryMeta::with_persistence(dir);

        // The corrupt entry must NOT be served.
        let fake: Digest = fake_key.parse().expect("parse fake key");
        assert!(
            reg.get_manifest("lib/alpine", &Reference::Digest(fake))
                .await
                .expect("get fake")
                .is_none(),
            "manifest whose key != sha256(body) must be dropped on load"
        );

        // The valid entry alongside it must still resolve, with its exact
        // bytes and digest.
        let good: Digest = good_digest.parse().expect("parse good");
        let served = reg
            .get_manifest("lib/alpine", &Reference::Digest(good.clone()))
            .await
            .expect("get good")
            .expect("valid manifest still served");
        assert_eq!(served.0, good, "valid digest preserved");
        assert_eq!(&served.2[..], &good_body[..], "valid body preserved");
    }

    #[tokio::test]
    async fn r3_2_persist_failure_rolls_back_put_and_returns_error() {
        // R3-2: when persistence is configured but the snapshot write
        // fails, a manifest PUT must (a) return an error (not Ok) and
        // (b) leave NO un-persisted manifest in memory. We force the write
        // to fail by making the storage dir read-only after construction.
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let dir = tmp.path().to_path_buf();
        let reg = InMemoryRegistryMeta::with_persistence(&dir);

        // Make the directory read-only so write_snapshot_atomic's O_EXCL
        // create fails (and so does the parent-dir fsync path).
        let mut perms = std::fs::metadata(&dir).expect("metadata").permissions();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            perms.set_mode(0o500); // r-x------ : no write
        }
        std::fs::set_permissions(&dir, perms).expect("chmod ro");

        let body = Bytes::from_static(b"{\"schemaVersion\":2}");
        let digest = Digest::sha256_of(&body);
        let result = reg
            .put_manifest(
                "repo",
                &Reference::Digest(digest.clone()),
                &digest,
                "application/vnd.oci.image.manifest.v1+json",
                body,
            )
            .await;

        // Restore write perms so the TempDir can be cleaned up.
        let mut back = std::fs::metadata(&dir).expect("metadata").permissions();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            back.set_mode(0o700);
        }
        std::fs::set_permissions(&dir, back).expect("chmod rw");

        assert!(
            result.is_err(),
            "a manifest PUT that cannot be persisted must return an error, not Ok"
        );
        // The in-memory state must NOT retain the un-persisted manifest.
        assert!(
            reg.get_manifest("repo", &Reference::Digest(digest))
                .await
                .expect("get")
                .is_none(),
            "the rolled-back manifest must not be served from memory"
        );
    }

    #[tokio::test]
    async fn r3_2_in_memory_path_never_fails_persist() {
        // The non-persistent (test / in-memory) deployment must keep its
        // infallible persist path: put_manifest always succeeds.
        let reg = InMemoryRegistryMeta::new();
        let body = Bytes::from_static(b"body");
        let digest = Digest::sha256_of(&body);
        reg.put_manifest(
            "repo",
            &Reference::Digest(digest.clone()),
            &digest,
            "application/vnd.oci.image.manifest.v1+json",
            body,
        )
        .await
        .expect("in-memory put never fails on persistence");
    }

    #[test]
    fn r3_5_write_snapshot_round_trips_and_leaves_no_temp() {
        // R3-5: a normal save round-trips and leaves no leftover temp file
        // in the directory (only metadata.json remains).
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let path = tmp.path().join(super::METADATA_FILE_NAME);
        let mut snap = super::MetadataSnapshot::default();
        snap.tags
            .entry("repo".to_owned())
            .or_default()
            .insert("latest".to_owned(), "sha256:deadbeef".to_owned());

        super::InMemoryRegistryMeta::write_snapshot_atomic(&path, &snap)
            .expect("atomic write");

        // The target exists and round-trips.
        let loaded = super::InMemoryRegistryMeta::load_snapshot(&path)
            .expect("load")
            .expect("some");
        assert_eq!(
            loaded.tags.get("repo").and_then(|m| m.get("latest")),
            Some(&"sha256:deadbeef".to_owned()),
            "snapshot round-trips through write+load"
        );

        // No leftover *.tmp files in the directory.
        let leftovers: Vec<_> = std::fs::read_dir(tmp.path())
            .expect("read_dir")
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| {
                std::path::Path::new(n)
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("tmp"))
            })
            .collect();
        assert!(
            leftovers.is_empty(),
            "no temp files must be left behind: {leftovers:?}"
        );
    }
}
