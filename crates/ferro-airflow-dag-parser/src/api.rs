// SPDX-License-Identifier: Apache-2.0
//! Stable public API for the static DAG extractor.
//!
//! Wraps the [`crate::ruff_impl`] backend behind four functions
//! ([`extract_static_dag`],
//! [`extract_all_static_dags`], [`parse_dag_path`], [`dynamic_markers_for`])
//! plus the [`ParseOutcome`] aggregate that bundles a parse result with
//! the dynamic-fallback markers, source hash, and parse timestamp the
//! caller will typically want to memoise alongside the DAGs.
//!
//! The backend is `ruff` (see the `parser-ruff` feature, on by
//! default).

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::cache::hash_source;
use crate::common::{ExtractedDag, ParseError};
use crate::dynamic_markers::{DynamicMarker, detect_dynamic_markers};

/// Outcome of parsing one DAG file. Combines the static [`ExtractedDag`]
/// list, the dynamic markers detected on the same source, a content
/// hash for cache-key invalidation, and a parse timestamp.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ParseOutcome {
    /// Resolved DAGs (zero or more per file).
    pub dags: Vec<ExtractedDag>,
    /// Markers that say "static analysis is incomplete; route this file
    /// through a runtime fallback (e.g. `PyO3` / `CPython` embedding) for
    /// full fidelity".
    pub dynamic_markers: Vec<DynamicMarker>,
    /// Stable `FxHash` of the source bytes. Used by [`crate::ParseCache`]
    /// to skip redundant re-parses across DAG-folder poll cycles.
    pub source_hash: u64,
    /// Wall-clock time at which the source was parsed, in UTC.
    pub parsed_at: DateTime<Utc>,
    /// Path on disk the source was loaded from. `None` for in-memory
    /// strings parsed via [`extract_static_dag`].
    pub source_path: Option<PathBuf>,
}

/// Parse a Python source string and return the first DAG (or default if
/// the file contains no DAG).
///
/// # Errors
///
/// Returns [`ParseError::Parse`] when the underlying Python parser
/// rejects the source, or [`ParseError::InvalidIdentifier`] when the
/// recovered identifier fails [`crate::DagId`] / [`crate::TaskId`]
/// validation.
pub fn extract_static_dag(src: &str) -> Result<ExtractedDag, ParseError> {
    extract_with_default_backend(src).map(|v| v.into_iter().next().unwrap_or_default())
}

/// Parse a Python source string and return every DAG found.
///
/// # Errors
///
/// Same surface as [`extract_static_dag`].
pub fn extract_all_static_dags(src: &str) -> Result<Vec<ExtractedDag>, ParseError> {
    extract_with_default_backend(src)
}

/// Read `path` from disk, parse it, and return the combined
/// [`ParseOutcome`].
///
/// # Errors
///
/// Returns [`ParseError::Io`] when the file cannot be read, or any of
/// the [`extract_static_dag`] error variants.
pub fn parse_dag_path(path: &Path) -> Result<ParseOutcome, ParseError> {
    let source = std::fs::read_to_string(path).map_err(|e| ParseError::Io {
        path: path.to_path_buf(),
        source: e,
    })?;
    let hash = hash_source(&source);
    parse_dag_file(path, &source, hash)
}

/// Internal entrypoint shared by [`crate::ParseCache::get_or_parse`] and
/// [`parse_dag_path`]. Takes a pre-computed hash so the cache does not
/// re-hash on a hit.
pub(crate) fn parse_dag_file(
    path: &Path,
    source: &str,
    hash: u64,
) -> Result<ParseOutcome, ParseError> {
    let dags = extract_with_default_backend(source)?;
    let dynamic_markers = detect_dynamic_markers(source).unwrap_or_default();
    Ok(ParseOutcome {
        dags,
        dynamic_markers,
        source_hash: hash,
        parsed_at: Utc::now(),
        source_path: Some(path.to_path_buf()),
    })
}

/// Detect dynamic markers in `src` without producing an
/// [`ExtractedDag`]. Useful for the "should I bypass the static
/// fast-path?" routing decision.
///
/// On a parse error the function returns an empty `Vec` rather than
/// [`Result`] so callers can fold the marker count into a metric without
/// extra plumbing. Use [`detect_dynamic_markers`] directly when the
/// parse error needs to surface.
#[must_use]
pub fn dynamic_markers_for(src: &str) -> Vec<DynamicMarker> {
    detect_dynamic_markers(src).unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Backend dispatch
// ---------------------------------------------------------------------------

#[cfg(feature = "parser-ruff")]
fn extract_with_default_backend(src: &str) -> Result<Vec<ExtractedDag>, ParseError> {
    crate::ruff_impl::extract_all(src)
}
