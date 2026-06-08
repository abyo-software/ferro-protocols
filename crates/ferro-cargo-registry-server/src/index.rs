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

/// A single dependency declaration as it appears in a **publish**
/// request's metadata (the `PUT /api/v1/crates/new` JSON pre-image).
///
/// This is deliberately distinct from [`IndexDep`]: cargo's publish
/// metadata names the requirement field `version_req` (not `req`) and
/// carries `explicit_name_in_toml` to express dependency renames, where
/// the sparse index instead splits the renamed alias across the `name`
/// and `package` fields.
///
/// Reference:
/// <https://doc.rust-lang.org/cargo/reference/registry-web-api.html#publish>.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PublishDep {
    /// The dependency name as cargo resolves it. For a renamed
    /// dependency this is the *local alias* used in `Cargo.toml`, and
    /// `explicit_name_in_toml` carries the same alias.
    pub name: String,
    /// Version requirement string (`^1.0`, `=2.0.0`, ...). Note: the
    /// **index** entry calls this `req`.
    #[serde(default)]
    pub version_req: String,
    /// Enabled features.
    #[serde(default)]
    pub features: Vec<String>,
    /// Whether the dependency is optional.
    #[serde(default)]
    pub optional: bool,
    /// Whether default features are enabled.
    #[serde(default = "default_true")]
    pub default_features: bool,
    /// Dependency target (`cfg(...)` or target triple).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    /// Dependency kind — `normal`, `build`, or `dev`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// Registry URL the dependency is sourced from (`None` ⇒ this
    /// registry / crates.io).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registry: Option<String>,
    /// When the dependency is renamed, this is the alias used in the
    /// dependent's `Cargo.toml`; `name` then holds the registry name of
    /// the actual crate. When absent the dependency is not renamed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub explicit_name_in_toml: Option<String>,
}

const fn default_true() -> bool {
    true
}

/// Publish-request metadata, as deserialized from the JSON pre-image of
/// `PUT /api/v1/crates/new`.
///
/// This mirrors cargo's publish payload — which differs from the sparse
/// index entry in dependency field naming (`version_req`), rename
/// handling (`explicit_name_in_toml`), and the absence of a
/// registry-trusted `cksum` (the checksum is computed by the registry
/// from the uploaded tarball, never taken from the client). Convert it
/// into an [`IndexEntry`] with [`PublishManifest::into_index_entry`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PublishManifest {
    /// Crate name (canonical display case as published).
    pub name: String,
    /// Crate version. Cargo sends `vers`.
    #[serde(default)]
    pub vers: String,
    /// Legacy/alternate version key. Some older drafts and tooling send
    /// `version` instead of (or alongside) `vers`; when `vers` is empty
    /// this is used as a fallback. Kept as a separate field rather than a
    /// serde `alias` so a payload carrying *both* keys does not trip a
    /// duplicate-field error.
    #[serde(default, rename = "version", skip_serializing)]
    version: String,
    /// Dependencies, in publish-metadata shape.
    #[serde(default)]
    pub deps: Vec<PublishDep>,
    /// Feature map.
    #[serde(default)]
    pub features: std::collections::BTreeMap<String, Vec<String>>,
    /// Native library linkage name, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub links: Option<String>,
    /// Minimum Rust version required.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rust_version: Option<String>,
}

impl PublishDep {
    /// Convert a publish dependency into its sparse-index form,
    /// mapping `version_req` → `req` and resolving the rename split.
    ///
    /// For a renamed dependency (`explicit_name_in_toml` present and
    /// different from `name`), the index `name` is the dependency's
    /// registry name and `package` is `None` only when the names match;
    /// per the index schema the index `name` is the *explicit* alias and
    /// `package` is the original registry name. Cargo populates the
    /// publish payload such that `name` is the registry name and
    /// `explicit_name_in_toml` is the alias, so we map:
    /// - index `name`    ⇐ `explicit_name_in_toml` (alias) when renamed,
    ///   else the registry name;
    /// - index `package` ⇐ the registry name when renamed, else `None`.
    #[must_use]
    pub fn into_index_dep(self) -> IndexDep {
        let renamed = self
            .explicit_name_in_toml
            .as_ref()
            .is_some_and(|alias| alias != &self.name);
        let (name, package) = if renamed {
            // self.name is the registry name; the alias is the index name.
            let alias = self
                .explicit_name_in_toml
                .clone()
                .unwrap_or_else(|| self.name.clone());
            (alias, Some(self.name.clone()))
        } else {
            (self.name.clone(), None)
        };
        IndexDep {
            name,
            req: self.version_req,
            features: self.features,
            optional: self.optional,
            default_features: self.default_features,
            target: self.target,
            kind: self.kind,
            registry: self.registry,
            package,
        }
    }
}

impl PublishManifest {
    /// Convert publish metadata into a sparse-[`IndexEntry`], stamping in
    /// the registry-computed `cksum` (hex SHA-256 of the uploaded
    /// `.crate` tarball). The `vers` is taken verbatim; dependency
    /// `version_req` becomes `req` and renames are resolved per
    /// [`PublishDep::into_index_dep`].
    #[must_use]
    pub fn into_index_entry(self, cksum: String) -> IndexEntry {
        let vers = if self.vers.is_empty() {
            self.version
        } else {
            self.vers
        };
        IndexEntry {
            name: self.name,
            vers,
            deps: self.deps.into_iter().map(PublishDep::into_index_dep).collect(),
            cksum,
            features: self.features,
            yanked: false,
            links: self.links,
            v: Some(2),
            features2: None,
            rust_version: self.rust_version,
        }
    }
}

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
/// yanked / links / v / features2 / `rust_version`.
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

/// Build an [`IndexEntry`] from a publish manifest `Value` plus the
/// registry-computed cksum.
///
/// The manifest comes from the publish request body after the 4-byte
/// `metadata_len` prefix. It is parsed through the dedicated
/// [`PublishManifest`] model (publish-metadata shape) and explicitly
/// converted to the sparse-index entry shape, so that:
/// - dependency `version_req` is mapped to the index `req` field;
/// - renamed dependencies (`explicit_name_in_toml`) are split across the
///   index `name`/`package` fields;
/// - `cksum` is the registry-computed SHA-256 of the `.crate` tarball,
///   never a client-supplied value.
///
/// # Errors
/// Returns the serde error verbatim when the manifest is missing
/// required fields (`name`, `vers`) or has a malformed dependency.
pub fn entry_from_manifest(
    manifest: &Value,
    cksum: String,
) -> Result<IndexEntry, serde_json::Error> {
    let publish: PublishManifest = serde_json::from_value(manifest.clone())?;
    Ok(publish.into_index_entry(cksum))
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

    /// F4 regression: publish metadata uses `version_req` for the
    /// requirement and `explicit_name_in_toml` for renamed deps. The
    /// index entry must map `version_req` → `req` and split a renamed
    /// dep across `name` (alias) / `package` (registry name), with the
    /// `cksum` coming from the registry (not the client).
    #[test]
    fn entry_from_manifest_maps_version_req_and_rename() {
        // A normal crate WITH a renamed dependency, in cargo's publish
        // metadata shape. Note `version_req` (not `req`) and
        // `explicit_name_in_toml`.
        let manifest = json!({
            "name": "consumer",
            "vers": "1.2.3",
            "deps": [{
                "name": "serde",
                "version_req": "^1.0",
                "features": [],
                "optional": false,
                "default_features": true,
                "kind": "normal",
                "explicit_name_in_toml": "my_serde"
            }],
            "features": {},
            // A malicious/wrong client-supplied cksum must be ignored —
            // entry_from_manifest stamps the registry-computed value.
            "cksum": "ff".repeat(32)
        });
        let registry_cksum = "ab".repeat(32);
        let e = entry_from_manifest(&manifest, registry_cksum.clone()).unwrap();
        assert_eq!(e.cksum, registry_cksum, "cksum must be registry-computed");
        assert_eq!(e.deps.len(), 1);
        let dep = &e.deps[0];
        // version_req → req
        assert_eq!(dep.req, "^1.0", "version_req must map to req");
        // renamed: index name = alias, package = registry name
        assert_eq!(dep.name, "my_serde", "index name is the toml alias");
        assert_eq!(
            dep.package.as_deref(),
            Some("serde"),
            "package is the registry crate name when renamed"
        );
    }

    /// F4 regression: a non-renamed dependency must NOT emit a `package`
    /// field, and its `version_req` still maps to `req`.
    #[test]
    fn entry_from_manifest_non_renamed_dep_has_no_package() {
        let manifest = json!({
            "name": "consumer",
            "vers": "0.1.0",
            "deps": [{
                "name": "anyhow",
                "version_req": "=1.0.86",
                "default_features": true,
                "kind": "normal"
            }],
            "features": {}
        });
        let e = entry_from_manifest(&manifest, "00".repeat(32)).unwrap();
        let dep = &e.deps[0];
        assert_eq!(dep.name, "anyhow");
        assert_eq!(dep.req, "=1.0.86");
        assert_eq!(dep.package, None, "non-renamed dep must omit package");
        // And the serialized index line uses `req`, not `version_req`.
        let line = e.to_line().unwrap();
        assert!(line.contains("\"req\":\"=1.0.86\""), "line: {line}");
        assert!(
            !line.contains("version_req"),
            "index line must not leak publish field name: {line}"
        );
    }
}
