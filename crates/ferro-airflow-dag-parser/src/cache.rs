// SPDX-License-Identifier: Apache-2.0
//! Path-keyed parse cache for [`ParseOutcome`].
//!
//! `ferroair-dag-processor` re-scans the DAGs folder every few seconds
//! (the `dag_processor.poll_interval` setting). Re-parsing every file
//! every poll is wasteful — Airflow's reference implementation hashes
//! file content and skips the parse when the hash matches the cached
//! one. Phase 1 reproduces that behaviour using a `dashmap::DashMap`
//! keyed on the file's canonicalised path.
//!
//! The cache is intentionally process-local (no on-disk persistence)
//! and assumes the caller validated the path is inside the configured
//! DAGs folder. Cross-process invalidation is the dag-processor's job
//! (it owns the inotify / kqueue watcher).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use dashmap::DashMap;

use crate::api::{ParseOutcome, parse_dag_file};
use crate::common::ParseError;

/// Process-local parse cache. Cheap to clone (the inner [`DashMap`] is
/// behind an [`Arc`]).
#[derive(Debug, Clone, Default)]
pub struct ParseCache {
    inner: Arc<DashMap<PathBuf, CacheEntry>>,
}

#[derive(Debug, Clone)]
struct CacheEntry {
    /// Hash of the file contents at the time of the last parse.
    source_hash: u64,
    /// File mtime + size at the time of the last parse. Used as a
    /// cheap "did the file change?" pre-check so the hot path can skip
    /// the file read + hash on a stat-only match (the workspace target
    /// is < 5 µs / hit, which a full `read_to_string` + `FxHash` blows
    /// past on a 600-byte DAG).
    fingerprint: FileFingerprint,
    /// Memoised parse outcome.
    outcome: ParseOutcome,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FileFingerprint {
    /// File modification time as nanoseconds since the Unix epoch.
    /// Falls back to `0` when the platform reports no mtime.
    mtime_ns: u128,
    /// File size in bytes.
    size: u64,
}

impl FileFingerprint {
    fn from_metadata(meta: &std::fs::Metadata) -> Self {
        let mtime_ns = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
            .map_or(0u128, |d| d.as_nanos());
        Self {
            mtime_ns,
            size: meta.len(),
        }
    }
}

impl ParseCache {
    /// Create an empty cache.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of cached entries.
    #[must_use]
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// `true` when the cache holds no entries.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Retrieve a cached [`ParseOutcome`] without re-parsing. Returns
    /// `None` when the file has not been parsed yet (or was invalidated).
    /// Useful for read-only consumers like a `/metrics` exporter.
    #[must_use]
    pub fn peek(&self, path: &Path) -> Option<ParseOutcome> {
        let key = canonicalise(path);
        self.inner.get(&key).map(|e| e.outcome.clone())
    }

    /// Parse `path` if the cached hash differs from the on-disk hash, or
    /// return the cached [`ParseOutcome`] when they match.
    ///
    /// # Errors
    ///
    /// Returns [`ParseError::Io`] if the file cannot be read, or
    /// [`ParseError::Parse`] / [`ParseError::InvalidIdentifier`] if the
    /// underlying parse fails.
    pub fn get_or_parse(&self, path: &Path) -> Result<ParseOutcome, ParseError> {
        let key = canonicalise(path);
        // Cheap pre-check: stat the file and look up the fingerprint.
        // When the fingerprint matches the cached entry we return
        // immediately — no `read_to_string` + hash on the hot path.
        let meta = std::fs::metadata(&key).map_err(|e| ParseError::Io {
            path: key.clone(),
            source: e,
        })?;
        let fingerprint = FileFingerprint::from_metadata(&meta);
        if let Some(entry) = self.inner.get(&key)
            && entry.fingerprint == fingerprint
        {
            return Ok(entry.outcome.clone());
        }
        // Cold path: read + hash + parse.
        let source = std::fs::read_to_string(&key).map_err(|e| ParseError::Io {
            path: key.clone(),
            source: e,
        })?;
        let hash = hash_source(&source);
        if let Some(entry) = self.inner.get(&key)
            && entry.source_hash == hash
        {
            // Content unchanged but fingerprint drifted (e.g. `touch`).
            // Refresh the fingerprint to keep the fast path warm but
            // skip the parse.
            let mut e = entry.clone();
            drop(entry);
            e.fingerprint = fingerprint;
            self.inner.insert(key.clone(), e.clone());
            return Ok(e.outcome);
        }
        let outcome = parse_dag_file(&key, &source, hash)?;
        self.inner.insert(
            key,
            CacheEntry {
                source_hash: hash,
                fingerprint,
                outcome: outcome.clone(),
            },
        );
        Ok(outcome)
    }

    /// Drop the cache entry for `path`, if any. Used by the
    /// dag-processor when its watcher reports a delete / unlink.
    pub fn invalidate(&self, path: &Path) {
        let key = canonicalise(path);
        self.inner.remove(&key);
    }

    /// Drop every entry. Used on configuration reload.
    pub fn clear(&self) {
        self.inner.clear();
    }
}

/// Best-effort path canonicalisation. Falls back to the input path when
/// canonicalisation fails (e.g. the file does not yet exist on disk —
/// the caller is about to parse a synthetic source string).
fn canonicalise(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Stable, machine-independent `FxHash` of the file contents. Stable
/// because the dag-processor compares hashes across scheduler restarts
/// to decide whether to re-emit `ImportError` rows.
#[must_use]
pub(crate) fn hash_source(src: &str) -> u64 {
    // We use a simple FxHash implementation rather than DefaultHasher
    // (which is randomised per-process). This matters for cache-warm
    // restarts in tests; production code only relies on within-process
    // consistency.
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &byte in src.as_bytes() {
        h ^= u64::from(byte);
        h = h.wrapping_mul(0x100_0000_01b3);
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn write_dag(path: &Path, body: &str) {
        std::fs::write(path, body).unwrap();
    }

    const DAG_BODY: &str = r#"
from airflow import DAG

with DAG(dag_id="cached"):
    pass
"#;

    #[test]
    fn first_call_parses_second_call_hits_cache() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("d1.py");
        write_dag(&p, DAG_BODY);
        let cache = ParseCache::new();
        let a = cache.get_or_parse(&p).unwrap();
        assert_eq!(cache.len(), 1);
        let b = cache.get_or_parse(&p).unwrap();
        assert_eq!(a.source_hash, b.source_hash);
        assert_eq!(a.dags.len(), b.dags.len());
    }

    #[test]
    fn rewrite_invalidates_cache_via_hash() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("d2.py");
        write_dag(&p, DAG_BODY);
        let cache = ParseCache::new();
        let a = cache.get_or_parse(&p).unwrap();
        // Rewrite with different content.
        write_dag(&p, &DAG_BODY.replace("cached", "renamed"));
        let b = cache.get_or_parse(&p).unwrap();
        assert_ne!(a.source_hash, b.source_hash);
        assert_eq!(b.dags[0].dag_id.as_ref().unwrap().as_str(), "renamed");
    }

    #[test]
    fn invalidate_drops_entry() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("d3.py");
        write_dag(&p, DAG_BODY);
        let cache = ParseCache::new();
        cache.get_or_parse(&p).unwrap();
        assert_eq!(cache.len(), 1);
        cache.invalidate(&p);
        assert_eq!(cache.len(), 0);
    }

    #[test]
    fn clear_drops_everything() {
        let dir = tempdir().unwrap();
        let p1 = dir.path().join("a.py");
        let p2 = dir.path().join("b.py");
        write_dag(&p1, DAG_BODY);
        write_dag(&p2, DAG_BODY);
        let cache = ParseCache::new();
        cache.get_or_parse(&p1).unwrap();
        cache.get_or_parse(&p2).unwrap();
        assert_eq!(cache.len(), 2);
        cache.clear();
        assert_eq!(cache.len(), 0);
    }

    #[test]
    fn peek_returns_none_when_unparsed() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("never.py");
        write_dag(&p, DAG_BODY);
        let cache = ParseCache::new();
        assert!(cache.peek(&p).is_none());
        cache.get_or_parse(&p).unwrap();
        assert!(cache.peek(&p).is_some());
    }

    #[test]
    fn missing_file_surfaces_io_error() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("does_not_exist.py");
        let cache = ParseCache::new();
        let err = cache.get_or_parse(&p).unwrap_err();
        assert!(matches!(err, ParseError::Io { .. }));
    }

    #[test]
    fn hash_is_stable_across_runs() {
        // Stable hash means two runs of the test process will produce
        // the same value for the same input. We rely on this so the
        // dag-processor can dedupe ImportError rows across restarts.
        let h1 = hash_source("hello");
        let h2 = hash_source("hello");
        assert_eq!(h1, h2);
        let h3 = hash_source("hellp");
        assert_ne!(h1, h3);
    }
}
