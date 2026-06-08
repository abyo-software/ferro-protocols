// SPDX-License-Identifier: Apache-2.0
//! Durable persistence for the in-memory crate index (DD R2-6).
//!
//! The filesystem-backed deployment stores `.crate` tarballs in the
//! content-addressed [`ferro_blob_store::BlobStore`], but the per-crate
//! index state — published versions, `cksum`s, `yanked` flags, and owners
//! — lives in [`CargoState::crates`](crate::router::CargoState). Without
//! durable mirroring that map is lost on restart, even though the blobs
//! survive, so a restarted server would serve an empty sparse index.
//!
//! This module mirrors the index map to a single JSON snapshot in the
//! data directory (`index-state.json`). The snapshot is written
//! **through** on every mutation (publish / yank / unyank / owner change)
//! and **loaded** on boot. It is the simplest design that captures the
//! *full* record — crucially including owners, which the on-disk sparse
//! index files do not carry.
//!
//! The hot path stays the in-memory [`BTreeMap`]; the snapshot is a
//! durable mirror. The version → tarball [`Digest`] map is **not**
//! serialized: a tarball's `cksum` (hex SHA-256) is exactly its blob
//! digest, so [`load`] reconstructs the `tarballs` map from each entry's
//! `cksum`.
//!
//! Robustness: a missing or corrupt snapshot is treated as "start empty"
//! with a logged warning — a damaged state file never prevents boot.

use std::collections::BTreeMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use ferro_blob_store::{Digest, DigestAlgo};
use serde::{Deserialize, Serialize};

use crate::index::IndexEntry;
use crate::owners::Owner;
use crate::router::CrateRecord;

/// Snapshot filename written under the data directory.
pub const STATE_FILE: &str = "index-state.json";

/// On-disk form of a single crate's durable state.
///
/// Deliberately omits the `tarballs` digest map — it is rebuilt from each
/// entry's `cksum` on [`load`] — so the snapshot is a pure function of the
/// publicly served index plus the owner list.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct PersistedRecord {
    /// Published index entries, oldest first.
    entries: Vec<IndexEntry>,
    /// Owner list.
    #[serde(default)]
    owners: Vec<Owner>,
}

/// Top-level snapshot: canonical-name → persisted record.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct Snapshot {
    crates: BTreeMap<String, PersistedRecord>,
}

/// Absolute path of the snapshot file inside `data_dir`.
#[must_use]
pub fn state_path(data_dir: &Path) -> PathBuf {
    data_dir.join(STATE_FILE)
}

/// Load the durable index snapshot from `data_dir`, reconstructing the
/// in-memory record map.
///
/// A missing snapshot yields an empty map (first boot). A snapshot that
/// cannot be read or parsed is logged and treated as empty, so a corrupt
/// state file never blocks startup. Each entry's `cksum` is parsed back
/// into the version → tarball [`Digest`] map.
///
/// An entry whose `cksum` is not a valid SHA-256 hex is treated as
/// **corrupt** (R3-4): the version entry is **dropped** — both from the
/// served index and from the download map — rather than retained, because
/// a retained-but-unmapped entry advertises a version that can never be
/// downloaded *and* blocks a clean republish (it looks like a duplicate).
/// If every version of a crate is corrupt the crate is dropped entirely.
/// Each drop is logged.
#[must_use]
pub fn load(data_dir: &Path) -> BTreeMap<String, CrateRecord> {
    let path = state_path(data_dir);
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            tracing::debug!(path = %path.display(), "no index snapshot; starting empty");
            return BTreeMap::new();
        }
        Err(err) => {
            tracing::warn!(path = %path.display(), %err, "failed to read index snapshot; starting empty");
            return BTreeMap::new();
        }
    };

    let snapshot: Snapshot = match serde_json::from_slice(&bytes) {
        Ok(s) => s,
        Err(err) => {
            tracing::warn!(path = %path.display(), %err, "corrupt index snapshot; starting empty");
            return BTreeMap::new();
        }
    };

    let mut out = BTreeMap::new();
    for (key, rec) in snapshot.crates {
        let mut tarballs = BTreeMap::new();
        let mut entries = Vec::with_capacity(rec.entries.len());
        for entry in rec.entries {
            match Digest::new(DigestAlgo::Sha256, entry.cksum.clone()) {
                Ok(digest) => {
                    tarballs.insert(entry.vers.clone(), digest);
                    entries.push(entry);
                }
                Err(err) => {
                    // R3-4: an un-parseable cksum makes the version
                    // undownloadable. Drop it from the index entirely
                    // rather than advertising a phantom version that also
                    // blocks republish.
                    tracing::warn!(
                        crate_key = %key,
                        version = %entry.vers,
                        %err,
                        "snapshot entry has an invalid cksum; dropping corrupt version"
                    );
                }
            }
        }
        if entries.is_empty() {
            // Every version was corrupt — drop the crate so it does not
            // appear as an empty, unservable record.
            tracing::warn!(
                crate_key = %key,
                "all versions had invalid cksums; dropping corrupt crate"
            );
            continue;
        }
        out.insert(
            key,
            CrateRecord {
                entries,
                tarballs,
                owners: rec.owners,
            },
        );
    }
    tracing::info!(crates = out.len(), "loaded durable index snapshot");
    out
}

/// Monotonic counter guaranteeing a distinct temp-file suffix per call
/// within a process, combined with the pid so concurrent processes never
/// collide on the temp name either.
static SAVE_SEQ: AtomicU64 = AtomicU64::new(0);

/// Write the in-memory index map through to the durable snapshot.
///
/// The write is **atomic and crash-durable** (R3-5):
///
/// 1. The snapshot is serialized into a uniquely-named temp file created
///    in the same directory with `O_CREAT | O_EXCL`
///    ([`create_new`](std::fs::OpenOptions::create_new)). The unique
///    suffix (`pid` + a per-process atomic counter, no `Date`/`rand`)
///    plus `O_EXCL` means an attacker who pre-places a symlink at the
///    temp path makes the create *fail* rather than following the link.
/// 2. The temp file is `write_all` + `sync_all` (fsync), so its bytes are
///    on stable storage before it is published.
/// 3. The temp file is `rename`d over the target (atomic replace).
/// 4. The parent directory is fsynced so the rename itself is durable —
///    otherwise a crash could lose the directory entry and leave the old
///    (or no) snapshot.
///
/// A crash at any point therefore leaves either the complete old snapshot
/// or the complete new one, never a truncated `index-state.json`.
///
/// # Errors
///
/// Returns an I/O or serialization error when the snapshot cannot be
/// written. On failure the temp file is best-effort removed so a failed
/// save leaves no orphan behind.
pub fn save(
    data_dir: &Path,
    crates: &BTreeMap<String, CrateRecord>,
) -> Result<(), std::io::Error> {
    let snapshot = Snapshot {
        crates: crates
            .iter()
            .map(|(k, v)| {
                (
                    k.clone(),
                    PersistedRecord {
                        entries: v.entries.clone(),
                        owners: v.owners.clone(),
                    },
                )
            })
            .collect(),
    };
    let json = serde_json::to_vec_pretty(&snapshot)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

    let target = state_path(data_dir);
    write_atomic_durable(data_dir, &target, &json)
}

/// Durably and atomically write `bytes` to `target` inside `dir`.
///
/// Shared by [`save`]; factored out so the temp-file lifecycle (create
/// exclusive → write → fsync → rename → fsync dir, with cleanup on the
/// error paths) lives in one place.
fn write_atomic_durable(dir: &Path, target: &Path, bytes: &[u8]) -> Result<(), std::io::Error> {
    // Try a few distinct unique names so a (vanishingly unlikely)
    // pre-existing temp — e.g. a leftover from a crash, or an attacker's
    // symlink — does not wedge the save permanently.
    let mut last_err = None;
    for _ in 0..16 {
        let seq = SAVE_SEQ.fetch_add(1, Ordering::Relaxed);
        let tmp = dir.join(format!(
            "{STATE_FILE}.tmp.{pid}.{seq}",
            pid = std::process::id()
        ));
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)
        {
            Ok(mut file) => {
                // From here on the temp file exists; make sure every error
                // path removes it.
                let result = (|| {
                    file.write_all(bytes)?;
                    file.sync_all()?;
                    drop(file);
                    std::fs::rename(&tmp, target)?;
                    sync_parent_dir(dir)
                })();
                if result.is_err() {
                    // Best-effort cleanup; ignore the removal error so the
                    // original failure is what surfaces.
                    let _ = std::fs::remove_file(&tmp);
                }
                return result;
            }
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                // Name collision (O_EXCL): retry with the next counter.
                last_err = Some(err);
            }
            Err(err) => return Err(err),
        }
    }
    Err(last_err.unwrap_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "could not allocate a unique snapshot temp file",
        )
    }))
}

/// fsync `dir` so a preceding `rename` into it is durable.
///
/// Opening a directory read-only and `sync_all`-ing it is the POSIX
/// idiom for flushing the directory entry. Platforms that cannot fsync a
/// directory return an error which we tolerate, since the file data was
/// already fsynced.
fn sync_parent_dir(dir: &Path) -> Result<(), std::io::Error> {
    match std::fs::File::open(dir) {
        Ok(d) => match d.sync_all() {
            Ok(()) => Ok(()),
            // Some filesystems/platforms reject fsync on a directory
            // handle; the file bytes are already durable, so don't fail
            // the save over the (best-effort) directory flush.
            Err(err) if err.kind() == std::io::ErrorKind::InvalidInput => Ok(()),
            Err(err) => Err(err),
        },
        Err(err) => Err(err),
    }
}

#[cfg(test)]
mod tests {
    use super::{STATE_FILE, load, save, state_path};
    use crate::index::IndexEntry;
    use std::path::Path;
    use crate::owners::Owner;
    use crate::router::CrateRecord;
    use ferro_blob_store::Digest;
    use std::collections::BTreeMap;

    fn entry(name: &str, vers: &str, tarball: &[u8]) -> IndexEntry {
        IndexEntry {
            name: name.into(),
            vers: vers.into(),
            deps: vec![],
            cksum: Digest::sha256_of(tarball).hex().to_owned(),
            features: BTreeMap::new(),
            yanked: false,
            links: None,
            v: Some(2),
            features2: None,
            rust_version: None,
        }
    }

    /// A `tracing` writer that appends every emitted byte into a shared
    /// buffer, so a test can inspect *which* log line a code path produced.
    /// Used to distinguish the two empty-returning arms of [`load`] (the
    /// silent "no snapshot" debug vs the "failed to read" warn), which are
    /// otherwise observationally identical through the return value.
    #[derive(Clone)]
    struct CaptureWriter(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

    impl std::io::Write for CaptureWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for CaptureWriter {
        type Writer = Self;
        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    /// Run `f` with a thread-local tracing subscriber capturing at DEBUG
    /// level, returning everything it logged as a `String`. Synchronous
    /// (`with_default` on the current thread) so it cannot race a neighbour
    /// test's global subscriber.
    fn capture_logs(f: impl FnOnce()) -> String {
        let buf = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::DEBUG)
            .with_ansi(false)
            .with_writer(CaptureWriter(buf.clone()))
            .finish();
        tracing::subscriber::with_default(subscriber, f);
        let bytes = buf.lock().unwrap().clone();
        String::from_utf8(bytes).unwrap()
    }

    #[test]
    fn missing_snapshot_loads_empty() {
        let tmp = tempfile::TempDir::new().unwrap();
        assert!(load(tmp.path()).is_empty());
    }

    /// R3: an **absent** snapshot file (the `ErrorKind::NotFound` arm of the
    /// `std::fs::read` match) loads empty *silently* — it logs the debug
    /// "no index snapshot; starting empty" line and **not** the warn
    /// "failed to read" line.
    ///
    /// This pins the `NotFound` match guard at `load` (`if err.kind() ==
    /// ErrorKind::NotFound`). A guard forced to `false`, or `==` flipped to
    /// `!=`, would route an absent file through the *other* arm and emit the
    /// "failed to read index snapshot" warn instead.
    #[test]
    fn absent_snapshot_takes_notfound_arm_silently() {
        let tmp = tempfile::TempDir::new().unwrap();
        // No state file exists → read() fails with NotFound.
        let logs = capture_logs(|| {
            assert!(load(tmp.path()).is_empty());
        });
        assert!(
            logs.contains("no index snapshot; starting empty"),
            "absent snapshot must hit the NotFound (debug) arm; got: {logs}"
        );
        assert!(
            !logs.contains("failed to read index snapshot"),
            "absent snapshot must NOT hit the generic-IO-error (warn) arm; got: {logs}"
        );
    }

    /// R3 companion: a snapshot **path that is unreadable for a non-NotFound
    /// reason** (here it is a *directory*, so `std::fs::read` fails with a
    /// non-`NotFound` kind) loads empty via the *other* arm — it logs the
    /// "failed to read index snapshot" warn and **not** the `NotFound` debug.
    ///
    /// Together with [`absent_snapshot_takes_notfound_arm_silently`] this
    /// pins both branches of the `err.kind() == ErrorKind::NotFound` guard:
    /// a guard forced to `true` would route this present-but-unreadable case
    /// through the `NotFound` (silent debug) arm instead of the warn arm.
    #[test]
    fn unreadable_snapshot_takes_non_notfound_arm() {
        let tmp = tempfile::TempDir::new().unwrap();
        // Make the snapshot *path* a directory: reading it fails with a
        // non-NotFound error (e.g. IsADirectory), so the match falls through
        // to the generic `Err(err)` warn arm.
        std::fs::create_dir(state_path(tmp.path())).unwrap();
        let read_err_kind = std::fs::read(state_path(tmp.path())).unwrap_err().kind();
        assert_ne!(
            read_err_kind,
            std::io::ErrorKind::NotFound,
            "precondition: a directory read must be a non-NotFound error"
        );

        let logs = capture_logs(|| {
            assert!(load(tmp.path()).is_empty());
        });
        assert!(
            logs.contains("failed to read index snapshot"),
            "an unreadable (non-NotFound) snapshot must hit the warn arm; got: {logs}"
        );
        assert!(
            !logs.contains("no index snapshot; starting empty"),
            "an unreadable snapshot must NOT hit the silent NotFound arm; got: {logs}"
        );
    }

    #[test]
    fn corrupt_snapshot_loads_empty() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::write(state_path(tmp.path()), b"{ not json").unwrap();
        assert!(load(tmp.path()).is_empty());
    }

    #[test]
    fn save_then_load_round_trips_entries_owners_and_digests() {
        let tmp = tempfile::TempDir::new().unwrap();
        let tarball = b"round-trip-bytes";
        let mut e = entry("foo", "1.0.0", tarball);
        e.yanked = true;
        let mut rec = CrateRecord {
            entries: vec![e],
            tarballs: BTreeMap::new(),
            owners: vec![Owner {
                id: 1,
                login: "alice".into(),
                name: None,
            }],
        };
        rec.tarballs
            .insert("1.0.0".into(), Digest::sha256_of(tarball));
        let mut map = BTreeMap::new();
        map.insert("foo".to_owned(), rec);

        save(tmp.path(), &map).unwrap();
        let loaded = load(tmp.path());

        let got = loaded.get("foo").expect("crate present after reload");
        assert_eq!(got.entries.len(), 1);
        assert_eq!(got.entries[0].vers, "1.0.0");
        assert!(got.entries[0].yanked, "yanked flag survives");
        assert_eq!(got.owners.len(), 1);
        assert_eq!(got.owners[0].login, "alice");
        // tarballs map rebuilt from the entry cksum.
        let digest = got.tarballs.get("1.0.0").expect("digest rebuilt");
        assert_eq!(*digest, Digest::sha256_of(tarball));
    }

    /// R3-4: a snapshot carrying one valid version and one whose `cksum`
    /// is not a parseable SHA-256 digest must load with **only** the valid
    /// version. The corrupt entry is dropped from both the served index
    /// and the download map, so the version is no longer advertised and is
    /// freely republishable.
    #[test]
    fn load_drops_entry_with_invalid_cksum() {
        let tmp = tempfile::TempDir::new().unwrap();
        let valid = entry("foo", "1.0.0", b"good-bytes");
        // A non-hex / wrong-length cksum is not a valid SHA-256 digest.
        let mut corrupt = entry("foo", "2.0.0", b"ignored");
        corrupt.cksum = "not-a-real-digest".to_owned();

        let json = serde_json::json!({
            "crates": {
                "foo": {
                    "entries": [valid, corrupt],
                    "owners": []
                }
            }
        });
        std::fs::write(
            state_path(tmp.path()),
            serde_json::to_vec(&json).unwrap(),
        )
        .unwrap();

        let loaded = load(tmp.path());
        let got = loaded.get("foo").expect("crate still present");
        // Only the valid version survives, in the index and the map.
        assert_eq!(got.entries.len(), 1, "corrupt version dropped from index");
        assert_eq!(got.entries[0].vers, "1.0.0");
        assert!(
            got.tarballs.contains_key("1.0.0"),
            "valid version downloadable"
        );
        assert!(
            !got.tarballs.contains_key("2.0.0"),
            "corrupt version has no download mapping"
        );
        assert!(
            !got.entries.iter().any(|e| e.vers == "2.0.0"),
            "corrupt version no longer advertised → republishable"
        );
    }

    /// R3-4 corollary: a crate whose *every* version has an invalid cksum
    /// is dropped entirely rather than loaded as an empty, unservable
    /// record.
    #[test]
    fn load_drops_crate_when_all_versions_corrupt() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut c1 = entry("bar", "1.0.0", b"x");
        c1.cksum = "zz".to_owned();
        let mut c2 = entry("bar", "2.0.0", b"y");
        c2.cksum = String::new();

        let json = serde_json::json!({
            "crates": {
                "bar": { "entries": [c1, c2], "owners": [] }
            }
        });
        std::fs::write(
            state_path(tmp.path()),
            serde_json::to_vec(&json).unwrap(),
        )
        .unwrap();

        let loaded = load(tmp.path());
        assert!(
            !loaded.contains_key("bar"),
            "fully-corrupt crate dropped, not loaded empty"
        );
    }

    /// R3-5: a successful save publishes the target snapshot and leaves
    /// **no** `index-state.json.tmp.*` temp file behind in the dir.
    #[test]
    fn save_is_atomic_no_tmp_left_behind() {
        let tmp = tempfile::TempDir::new().unwrap();
        let map = BTreeMap::new();
        save(tmp.path(), &map).unwrap();
        assert!(state_path(tmp.path()).exists(), "target written");
        assert!(
            no_temp_files(tmp.path()),
            "no snapshot temp file may be left behind"
        );
    }

    /// R3-5: repeated saves each round-trip and never accumulate temp
    /// files, exercising the per-call unique temp name + cleanup.
    #[test]
    fn repeated_saves_round_trip_without_temp_buildup() {
        let tmp = tempfile::TempDir::new().unwrap();
        let tarball = b"durable";
        let mut rec = CrateRecord {
            entries: vec![entry("foo", "1.0.0", tarball)],
            tarballs: BTreeMap::new(),
            owners: vec![],
        };
        rec.tarballs
            .insert("1.0.0".into(), Digest::sha256_of(tarball));
        let mut map = BTreeMap::new();
        map.insert("foo".to_owned(), rec);

        for _ in 0..5 {
            save(tmp.path(), &map).unwrap();
        }
        assert!(no_temp_files(tmp.path()), "no temp build-up across saves");
        let loaded = load(tmp.path());
        assert_eq!(loaded.get("foo").unwrap().entries[0].vers, "1.0.0");
    }

    /// R3-5: an attacker (or crash leftover) pre-placing a file at the
    /// *next* temp name must not wedge the save — `create_new` (`O_EXCL`)
    /// fails on that name and the writer retries the next unique suffix,
    /// so the save still succeeds and the squatted file is untouched.
    #[test]
    fn save_handles_create_new_temp_collision() {
        let tmp = tempfile::TempDir::new().unwrap();
        // Reconstruct the exact name the next save attempt will try.
        let seq = super::SAVE_SEQ.load(std::sync::atomic::Ordering::Relaxed);
        let squat = tmp.path().join(format!(
            "{STATE_FILE}.tmp.{pid}.{seq}",
            pid = std::process::id()
        ));
        std::fs::write(&squat, b"squatter").unwrap();

        let map = BTreeMap::new();
        save(tmp.path(), &map).expect("save retries past the squatted temp");

        assert!(state_path(tmp.path()).exists(), "target written");
        // The squatted file is left exactly as it was (not clobbered).
        assert_eq!(std::fs::read(&squat).unwrap(), b"squatter");
    }

    /// True when no snapshot temp file (`index-state.json.tmp.*`) remains
    /// in `dir`.
    fn no_temp_files(dir: &Path) -> bool {
        let prefix = format!("{STATE_FILE}.tmp.");
        !std::fs::read_dir(dir).unwrap().any(|e| {
            e.unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(&prefix)
        })
    }
}
