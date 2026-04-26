// SPDX-License-Identifier: Apache-2.0
//! Sparse-index line-oriented JSON format.
//!
//! Reference: `doc.rust-lang.org/cargo/reference/registries.html#index-format`.
//!
//! One `IndexEntry` per line, oldest first. The file is served with
//! `Content-Type: text/plain` (or `application/json`) and Cargo iterates
//! it line-by-line, so a correct serialiser MUST NOT embed newlines
//! inside JSON values.

use std::fmt::Write as _;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A single dependency declaration inside an index entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexDep {
    /// Dependency crate name.
    pub name: String,
    /// Version requirement as a string (`^1.0`, `=2.0.0`, ...).
    pub req: String,
    /// Enabled features.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub features: Vec<String>,
    /// Whether the dependency is optional.
    #[serde(default)]
    pub optional: bool,
    /// Whether default features are enabled.
    pub default_features: bool,
    /// Dependency target.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    /// Dependency kind — `normal`, `build`, or `dev`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// Optional registry URL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registry: Option<String>,
    /// Renamed-from, if the dep has a different local alias.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package: Option<String>,
}

/// A single line in the sparse index.
///
/// Reference: spec fields — name / vers / deps / cksum / features /
/// yanked / links / v / features2 / rust_version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexEntry {
    /// Crate name (canonical-case, as published).
    pub name: String,
    /// Crate version.
    pub vers: String,
    /// Dependencies.
    #[serde(default)]
    pub deps: Vec<IndexDep>,
    /// SHA-256 of the `.crate` tarball (hex, lowercase).
    #[serde(default)]
    pub cksum: String,
    /// Feature map.
    #[serde(default)]
    pub features: std::collections::BTreeMap<String, Vec<String>>,
    /// Yanked state.
    #[serde(default)]
    pub yanked: bool,
    /// Native library linkage name, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub links: Option<String>,
    /// Index format version — `2` is the most recent as of Cargo 1.75.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub v: Option<u32>,
    /// Extended feature map (supports dep: prefixes).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub features2: Option<std::collections::BTreeMap<String, Vec<String>>>,
    /// Minimum Rust version required.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rust_version: Option<String>,
}

impl IndexEntry {
    /// Serialize one entry as a single JSON line.
    ///
    /// # Errors
    /// Returns [`serde_json::Error`] if serialisation fails (practically
    /// never — every field is either a primitive or a map keyed by
    /// `String`).
    pub fn to_line(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }
}

/// Serialize a slice of entries to the on-wire line-oriented format.
#[must_use]
pub fn render_lines(entries: &[IndexEntry]) -> String {
    let mut out = String::new();
    for e in entries {
        if let Ok(line) = e.to_line() {
            let _ = writeln!(out, "{line}");
        }
    }
    out
}

/// Parse line-oriented JSON back into [`IndexEntry`] values.
///
/// # Errors
/// Returns an error on the first malformed line (the caller can embed
/// the position if needed).
pub fn parse_lines(text: &str) -> Result<Vec<IndexEntry>, serde_json::Error> {
    let mut out = Vec::new();
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let entry: IndexEntry = serde_json::from_str(line)?;
        out.push(entry);
    }
    Ok(out)
}

/// Build an [`IndexEntry`] from a publish manifest `Value` plus an
/// externally-computed cksum.
///
/// The manifest comes from the publish request body after the 4-byte
/// `metadata_len` prefix.
///
/// # Errors
/// Returns the serde error verbatim when the manifest is missing
/// required fields (`name`, `vers`).
pub fn entry_from_manifest(
    manifest: &Value,
    cksum: String,
) -> Result<IndexEntry, serde_json::Error> {
    let mut entry: IndexEntry = serde_json::from_value(manifest.clone())?;
    entry.cksum = cksum;
    // Cargo's publish manifest uses `vers` already; older drafts used
    // `version`. Respect either.
    if entry.vers.is_empty()
        && let Some(v) = manifest.get("version").and_then(Value::as_str)
    {
        v.clone_into(&mut entry.vers);
    }
    Ok(entry)
}

#[cfg(test)]
mod tests {
    use super::{IndexDep, IndexEntry, entry_from_manifest, parse_lines, render_lines};
    use serde_json::json;

    fn sample_entry() -> IndexEntry {
        IndexEntry {
            name: "foo".into(),
            vers: "1.0.0".into(),
            deps: vec![IndexDep {
                name: "bar".into(),
                req: "^1".into(),
                features: vec![],
                optional: false,
                default_features: true,
                target: None,
                kind: Some("normal".into()),
                registry: None,
                package: None,
            }],
            cksum: "00".repeat(32),
            features: std::collections::BTreeMap::new(),
            yanked: false,
            links: None,
            v: Some(2),
            features2: None,
            rust_version: Some("1.70.0".into()),
        }
    }

    #[test]
    fn line_serialisation_has_no_embedded_newlines() {
        let e = sample_entry();
        let line = e.to_line().unwrap();
        assert!(!line.contains('\n'));
    }

    #[test]
    fn render_and_parse_round_trip() {
        let a = sample_entry();
        let mut b = a.clone();
        b.vers = "1.1.0".into();
        let text = render_lines(&[a.clone(), b.clone()]);
        let parsed = parse_lines(&text).unwrap();
        assert_eq!(parsed, vec![a, b]);
    }

    #[test]
    fn entry_from_manifest_pulls_cksum_in() {
        let manifest = json!({
            "name": "foo",
            "vers": "1.0.0",
            "deps": [],
            "features": {}
        });
        let e = entry_from_manifest(&manifest, "aa".repeat(32)).unwrap();
        assert_eq!(e.name, "foo");
        assert_eq!(e.vers, "1.0.0");
        assert_eq!(e.cksum, "aa".repeat(32));
    }

    #[test]
    fn entry_accepts_version_key_fallback() {
        let manifest = json!({
            "name": "foo",
            "vers": "",
            "version": "2.0.0",
            "features": {}
        });
        let e = entry_from_manifest(&manifest, "00".into()).unwrap();
        assert_eq!(e.vers, "2.0.0");
    }

    #[test]
    fn empty_lines_are_skipped_on_parse() {
        let text = "\n\n";
        assert!(parse_lines(text).unwrap().is_empty());
    }
}
