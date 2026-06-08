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
use std::path::{Path, PathBuf};

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
/// into the version → tarball [`Digest`] map; an entry whose `cksum` is
/// not a valid SHA-256 hex is kept in the index but contributes no
/// download mapping (a warning is logged).
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
        for entry in &rec.entries {
            match Digest::new(DigestAlgo::Sha256, entry.cksum.clone()) {
                Ok(digest) => {
                    tarballs.insert(entry.vers.clone(), digest);
                }
                Err(err) => {
                    tracing::warn!(
                        crate_key = %key,
                        version = %entry.vers,
                        %err,
                        "snapshot entry has an invalid cksum; download mapping skipped"
                    );
                }
            }
        }
        out.insert(
            key,
            CrateRecord {
                entries: rec.entries,
                tarballs,
                owners: rec.owners,
            },
        );
    }
    tracing::info!(crates = out.len(), "loaded durable index snapshot");
    out
}

/// Write the in-memory index map through to the durable snapshot.
///
/// The write is atomic: the snapshot is serialized to a sibling temp file
/// and renamed over the target, so a crash mid-write never leaves a
/// truncated `index-state.json`.
///
/// # Errors
///
/// Returns an I/O or serialization error when the snapshot cannot be
/// written. Callers log-and-continue: a failed mirror must not fail the
/// originating request, but it is surfaced for observability.
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
    let tmp = target.with_extension("json.tmp");
    std::fs::write(&tmp, &json)?;
    std::fs::rename(&tmp, &target)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{load, save, state_path};
    use crate::index::IndexEntry;
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

    #[test]
    fn missing_snapshot_loads_empty() {
        let tmp = tempfile::TempDir::new().unwrap();
        assert!(load(tmp.path()).is_empty());
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

    #[test]
    fn save_is_atomic_no_tmp_left_behind() {
        let tmp = tempfile::TempDir::new().unwrap();
        let map = BTreeMap::new();
        save(tmp.path(), &map).unwrap();
        let leftover = tmp.path().join("index-state.json.tmp");
        assert!(!leftover.exists(), "temp file must be renamed away");
    }
}
