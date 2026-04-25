// SPDX-License-Identifier: Apache-2.0
//! Phase 1 public API contract: [`ParseCache`].
//!
//! Mirrors the unit-tests in `src/cache.rs` but at the public-API
//! boundary so the dag-processor can rely on the documented invariants:
//!
//! 1. First call parses; subsequent calls with unchanged content hit.
//! 2. Rewriting the file invalidates the cache via the source hash.
//! 3. `invalidate(path)` drops one entry; `clear()` drops everything.
//! 4. Concurrent reads are race-free (the `DashMap` shard handles it).
//! 5. `peek` is non-side-effecting.

#![cfg(feature = "parser-ruff")]

use std::sync::Arc;
use std::thread;

use ferro_airflow_dag_parser::ParseCache;

const DAG_BODY: &str = r#"
from airflow import DAG

with DAG(dag_id="cached"):
    pass
"#;

#[test]
fn first_then_hit() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("d.py");
    std::fs::write(&p, DAG_BODY).unwrap();
    let cache = ParseCache::new();
    assert!(cache.is_empty());
    let a = cache.get_or_parse(&p).unwrap();
    let b = cache.get_or_parse(&p).unwrap();
    assert_eq!(a.source_hash, b.source_hash);
    assert_eq!(cache.len(), 1);
    assert!(!cache.is_empty());
}

#[test]
fn rewrite_changes_hash_and_dag_id() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("d.py");
    std::fs::write(&p, DAG_BODY).unwrap();
    let cache = ParseCache::new();
    let a = cache.get_or_parse(&p).unwrap();
    std::fs::write(&p, DAG_BODY.replace("cached", "renamed")).unwrap();
    let b = cache.get_or_parse(&p).unwrap();
    assert_ne!(a.source_hash, b.source_hash);
    assert_eq!(b.dags[0].dag_id.as_ref().unwrap().as_str(), "renamed");
}

#[test]
fn invalidate_removes_one_entry() {
    let dir = tempfile::tempdir().unwrap();
    let p1 = dir.path().join("a.py");
    let p2 = dir.path().join("b.py");
    std::fs::write(&p1, DAG_BODY).unwrap();
    std::fs::write(&p2, DAG_BODY).unwrap();
    let cache = ParseCache::new();
    cache.get_or_parse(&p1).unwrap();
    cache.get_or_parse(&p2).unwrap();
    assert_eq!(cache.len(), 2);
    cache.invalidate(&p1);
    assert_eq!(cache.len(), 1);
}

#[test]
fn clear_drops_everything() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("c.py");
    std::fs::write(&p, DAG_BODY).unwrap();
    let cache = ParseCache::new();
    cache.get_or_parse(&p).unwrap();
    cache.clear();
    assert_eq!(cache.len(), 0);
}

#[test]
fn peek_does_not_populate() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("p.py");
    std::fs::write(&p, DAG_BODY).unwrap();
    let cache = ParseCache::new();
    assert!(cache.peek(&p).is_none());
    assert_eq!(cache.len(), 0);
    cache.get_or_parse(&p).unwrap();
    assert!(cache.peek(&p).is_some());
}

#[test]
fn concurrent_get_or_parse_is_race_free() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("conc.py");
    std::fs::write(&p, DAG_BODY).unwrap();
    let cache = Arc::new(ParseCache::new());
    let mut handles = Vec::new();
    for _ in 0..8 {
        let c = Arc::clone(&cache);
        let pc = p.clone();
        handles.push(thread::spawn(move || {
            for _ in 0..50 {
                let outcome = c.get_or_parse(&pc).unwrap();
                assert_eq!(outcome.dags.len(), 1);
            }
        }));
    }
    for h in handles {
        h.join().unwrap();
    }
    assert_eq!(cache.len(), 1);
}

#[test]
fn cache_is_clone_cheap_and_shared() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("share.py");
    std::fs::write(&p, DAG_BODY).unwrap();
    let cache_a = ParseCache::new();
    let cache_b = cache_a.clone();
    cache_a.get_or_parse(&p).unwrap();
    // Both views see the same entry (Arc<DashMap> inside).
    assert_eq!(cache_a.len(), 1);
    assert_eq!(cache_b.len(), 1);
}
