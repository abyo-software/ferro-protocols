// SPDX-License-Identifier: Apache-2.0
//! `maven-metadata.xml` reader and writer.
//!
//! Two flavours:
//!
//! 1. Artifact-index metadata at `{groupPath}/{artifactId}/maven-metadata.xml`
//!    containing `<release>`, `<latest>`, `<versions>/<version>` and
//!    `<lastUpdated>`.
//! 2. Version-level SNAPSHOT metadata at
//!    `{groupPath}/{artifactId}/{baseVersion}/maven-metadata.xml`
//!    containing `<snapshot>` with `<timestamp>`, `<buildNumber>`, and
//!    `<snapshotVersions>`.
//!
//! Spec: Maven Repository Metadata —
//! <https://maven.apache.org/ref/3.9.6/maven-repository-metadata/repository-metadata.html>.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::MavenError;
use crate::snapshot::is_snapshot_version;
use crate::xml::from_str_panic_safe;

/// The `maven-metadata.xml` document type.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MavenMetadata {
    /// `<groupId>`.
    pub group_id: String,
    /// `<artifactId>`.
    pub artifact_id: String,
    /// `<version>` — only populated for version-level SNAPSHOT metadata.
    pub version: Option<String>,
    /// `<versioning><release>`: last non-SNAPSHOT version.
    pub release: Option<String>,
    /// `<versioning><latest>`: most recently updated version.
    pub latest: Option<String>,
    /// `<versioning><versions>/<version>` children.
    pub versions: Vec<String>,
    /// `<versioning><lastUpdated>` in `yyyyMMddHHmmss` form.
    pub last_updated: Option<String>,
    /// `<versioning><snapshot>` block, if version-level.
    pub snapshot: Option<Snapshot>,
    /// `<versioning><snapshotVersions>` block, if version-level.
    pub snapshot_versions: Vec<SnapshotVersion>,
}

/// `<snapshot>` sub-block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    /// `yyyyMMdd.HHmmss` in UTC.
    pub timestamp: String,
    /// Monotonically increasing per-base-version build number.
    pub build_number: u32,
}

/// `<snapshotVersions>/<snapshotVersion>` entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotVersion {
    /// Optional `<classifier>`.
    pub classifier: Option<String>,
    /// `<extension>`.
    pub extension: String,
    /// `<value>` — timestamped version string.
    pub value: String,
    /// `<updated>` in `yyyyMMddHHmmss`.
    pub updated: String,
}

impl MavenMetadata {
    /// Build artifact-index metadata from a set of versions.
    ///
    /// `release` is set to the latest non-SNAPSHOT version, `latest` to
    /// the last version in input order (Maven uses the order present
    /// in the metadata file itself, not semantic ordering).
    #[must_use]
    pub fn artifact_index(
        group_id: impl Into<String>,
        artifact_id: impl Into<String>,
        versions: Vec<String>,
        last_updated: DateTime<Utc>,
    ) -> Self {
        let release = versions
            .iter()
            .rev()
            .find(|v| !is_snapshot_version(v))
            .cloned();
        let latest = versions.last().cloned();
        Self {
            group_id: group_id.into(),
            artifact_id: artifact_id.into(),
            version: None,
            release,
            latest,
            versions,
            last_updated: Some(format_last_updated(last_updated)),
            snapshot: None,
            snapshot_versions: Vec::new(),
        }
    }

    /// Build version-level SNAPSHOT metadata.
    #[must_use]
    pub fn snapshot_metadata(
        group_id: impl Into<String>,
        artifact_id: impl Into<String>,
        version: impl Into<String>,
        snapshot: Snapshot,
        snapshot_versions: Vec<SnapshotVersion>,
        last_updated: DateTime<Utc>,
    ) -> Self {
        Self {
            group_id: group_id.into(),
            artifact_id: artifact_id.into(),
            version: Some(version.into()),
            release: None,
            latest: None,
            versions: Vec::new(),
            last_updated: Some(format_last_updated(last_updated)),
            snapshot: Some(snapshot),
            snapshot_versions,
        }
    }

    /// Serialize to `maven-metadata.xml` body.
    #[must_use]
    pub fn to_xml(&self) -> String {
        use std::fmt::Write as _;
        let mut out = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
        out.push_str("<metadata>\n");
        let _ = writeln!(out, "  <groupId>{}</groupId>", escape(&self.group_id));
        let _ = writeln!(
            out,
            "  <artifactId>{}</artifactId>",
            escape(&self.artifact_id)
        );
        if let Some(v) = &self.version {
            let _ = writeln!(out, "  <version>{}</version>", escape(v));
        }

        out.push_str("  <versioning>\n");
        if let Some(r) = &self.release {
            let _ = writeln!(out, "    <release>{}</release>", escape(r));
        }
        if let Some(l) = &self.latest {
            let _ = writeln!(out, "    <latest>{}</latest>", escape(l));
        }
        if let Some(snap) = &self.snapshot {
            out.push_str("    <snapshot>\n");
            let _ = writeln!(
                out,
                "      <timestamp>{}</timestamp>",
                escape(&snap.timestamp)
            );
            let _ = writeln!(
                out,
                "      <buildNumber>{}</buildNumber>",
                snap.build_number
            );
            out.push_str("    </snapshot>\n");
        }
        if !self.versions.is_empty() {
            out.push_str("    <versions>\n");
            for v in &self.versions {
                let _ = writeln!(out, "      <version>{}</version>", escape(v));
            }
            out.push_str("    </versions>\n");
        }
        if !self.snapshot_versions.is_empty() {
            out.push_str("    <snapshotVersions>\n");
            for sv in &self.snapshot_versions {
                out.push_str("      <snapshotVersion>\n");
                if let Some(c) = &sv.classifier {
                    let _ = writeln!(out, "        <classifier>{}</classifier>", escape(c));
                }
                let _ = writeln!(
                    out,
                    "        <extension>{}</extension>",
                    escape(&sv.extension)
                );
                let _ = writeln!(out, "        <value>{}</value>", escape(&sv.value));
                let _ = writeln!(out, "        <updated>{}</updated>", escape(&sv.updated));
                out.push_str("      </snapshotVersion>\n");
            }
            out.push_str("    </snapshotVersions>\n");
        }
        if let Some(u) = &self.last_updated {
            let _ = writeln!(out, "    <lastUpdated>{}</lastUpdated>", escape(u));
        }
        out.push_str("  </versioning>\n");
        out.push_str("</metadata>\n");
        out
    }

    /// Parse a `maven-metadata.xml` body.
    ///
    /// # Errors
    ///
    /// Returns [`MavenError::InvalidMetadata`] when the XML is
    /// malformed or missing the top-level `<metadata>` / `<groupId>` /
    /// `<artifactId>` elements.
    ///
    /// # Panic safety
    ///
    /// Like [`crate::parse_pom`], the deserialisation step is routed
    /// through [`crate::xml::from_str_panic_safe`] so that the
    /// `quick_xml::de` 0.39.2 `unreachable!()` panic (`de/mod.rs:2903`)
    /// on malformed input becomes a clean [`MavenError::InvalidMetadata`]
    /// rather than aborting the request thread. This boundary is
    /// network-reachable through the Maven `maven-metadata.xml`
    /// `PUT`/`GET` handler, so an attacker-supplied body must never
    /// crash the registry process.
    pub fn from_xml(xml: &str) -> Result<Self, MavenError> {
        let raw: RawMetadata = from_str_panic_safe(xml, MavenError::InvalidMetadata)?;
        let group_id = raw.group_id.unwrap_or_default();
        let artifact_id = raw.artifact_id.unwrap_or_default();
        if group_id.is_empty() || artifact_id.is_empty() {
            return Err(MavenError::InvalidMetadata(
                "metadata missing groupId or artifactId".into(),
            ));
        }

        let versioning = raw.versioning.unwrap_or_default();
        let versions = versioning.versions.map(|v| v.version).unwrap_or_default();
        let snapshot = versioning.snapshot.map(|s| Snapshot {
            timestamp: s.timestamp.unwrap_or_default(),
            build_number: s.build_number.unwrap_or(0),
        });
        let snapshot_versions = versioning
            .snapshot_versions
            .map(|sv| sv.snapshot_version)
            .unwrap_or_default()
            .into_iter()
            .map(|e| SnapshotVersion {
                classifier: e.classifier,
                extension: e.extension.unwrap_or_default(),
                value: e.value.unwrap_or_default(),
                updated: e.updated.unwrap_or_default(),
            })
            .collect();

        Ok(Self {
            group_id,
            artifact_id,
            version: raw.version,
            release: versioning.release,
            latest: versioning.latest,
            versions,
            last_updated: versioning.last_updated,
            snapshot,
            snapshot_versions,
        })
    }
}

fn format_last_updated(dt: DateTime<Utc>) -> String {
    dt.format("%Y%m%d%H%M%S").to_string()
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(default)]
struct RawMetadata {
    #[serde(rename = "groupId")]
    group_id: Option<String>,
    #[serde(rename = "artifactId")]
    artifact_id: Option<String>,
    #[serde(rename = "version")]
    version: Option<String>,
    versioning: Option<RawVersioning>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(default)]
struct RawVersioning {
    release: Option<String>,
    latest: Option<String>,
    snapshot: Option<RawSnapshot>,
    versions: Option<RawVersions>,
    #[serde(rename = "snapshotVersions")]
    snapshot_versions: Option<RawSnapshotVersions>,
    #[serde(rename = "lastUpdated")]
    last_updated: Option<String>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(default)]
struct RawSnapshot {
    timestamp: Option<String>,
    #[serde(rename = "buildNumber")]
    build_number: Option<u32>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(default)]
struct RawVersions {
    version: Vec<String>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(default)]
struct RawSnapshotVersions {
    #[serde(rename = "snapshotVersion")]
    snapshot_version: Vec<RawSnapshotVersion>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(default)]
struct RawSnapshotVersion {
    classifier: Option<String>,
    extension: Option<String>,
    value: Option<String>,
    updated: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::{MavenMetadata, Snapshot, SnapshotVersion};
    use chrono::TimeZone;

    #[test]
    fn artifact_index_picks_release_and_latest() {
        let versions = vec![
            "1.0".to_string(),
            "1.1-SNAPSHOT".to_string(),
            "1.1".to_string(),
            "1.2-SNAPSHOT".to_string(),
        ];
        let dt = chrono::Utc
            .with_ymd_and_hms(2026, 4, 23, 1, 2, 3)
            .single()
            .expect("dt");
        let m = MavenMetadata::artifact_index("com.example", "foo", versions, dt);
        assert_eq!(m.release.as_deref(), Some("1.1"));
        assert_eq!(m.latest.as_deref(), Some("1.2-SNAPSHOT"));
        assert_eq!(m.last_updated.as_deref(), Some("20260423010203"));
    }

    #[test]
    fn serialise_contains_metadata_root() {
        let dt = chrono::Utc
            .with_ymd_and_hms(2026, 4, 23, 1, 2, 3)
            .single()
            .expect("dt");
        let m = MavenMetadata::artifact_index("com.example", "foo", vec!["1.0".to_string()], dt);
        let xml = m.to_xml();
        assert!(xml.starts_with("<?xml"));
        assert!(xml.contains("<metadata>"));
        assert!(xml.contains("<artifactId>foo</artifactId>"));
        assert!(xml.contains("<release>1.0</release>"));
    }

    #[test]
    fn snapshot_metadata_roundtrip() {
        let dt = chrono::Utc
            .with_ymd_and_hms(2026, 4, 23, 12, 30, 45)
            .single()
            .expect("dt");
        let snap = Snapshot {
            timestamp: "20260423.123045".into(),
            build_number: 3,
        };
        let sv = SnapshotVersion {
            classifier: None,
            extension: "jar".into(),
            value: "1.2.3-20260423.123045-3".into(),
            updated: "20260423123045".into(),
        };
        let m = MavenMetadata::snapshot_metadata(
            "com.example",
            "foo",
            "1.2.3-SNAPSHOT",
            snap,
            vec![sv],
            dt,
        );
        let xml = m.to_xml();
        let parsed = MavenMetadata::from_xml(&xml).expect("roundtrip");
        assert_eq!(parsed.version.as_deref(), Some("1.2.3-SNAPSHOT"));
        assert_eq!(parsed.snapshot.expect("snap").build_number, 3);
        assert_eq!(parsed.snapshot_versions.len(), 1);
    }

    #[test]
    fn malformed_xml_fails() {
        let err = MavenMetadata::from_xml("<metadata><artifactId>oops")
            .expect_err("reject truncated metadata");
        assert!(err.to_string().contains("XML parse failed"));
    }

    /// DoS-parity regression with `pom::tests::
    /// quick_xml_unreachable_panic_caught_2026_05_15`. The
    /// `maven-metadata.xml` `from_xml` boundary is network-reachable
    /// through the Maven `PUT`/`GET` metadata handler, so the same
    /// `quick_xml::de` 0.39.2 `unreachable!()` panic (`de/mod.rs:2903`)
    /// that aborts on malformed POM bodies must be panic-shielded here
    /// too. We feed the exact 173-byte fuzz artifact
    /// `crash-1ceeadf1` verbatim and assert an `Err`, not an abort.
    ///
    /// Before the `from_str_panic_safe` wrap this test panicked
    /// ("entered unreachable code") and aborted the test process;
    /// after the wrap it returns `Err(InvalidMetadata)`. If quick-xml
    /// is updated and stops panicking, this still passes (it lands in
    /// the `Ok(Err(_))` / "XML parse failed" branch instead).
    #[test]
    fn quick_xml_unreachable_panic_caught_metadata() {
        // Same bytes as the POM fuzz artifact
        // `fuzz/known-crash/maven_pom_parse/crash-1ceeadf1-...` (173 B);
        // the panic is in shared quick-xml deserialisation, independent
        // of the target struct, so the POM crash input reproduces it
        // against `RawMetadata` too.
        let bytes: &[u8] = &[
            0x3c, 0x3e, 0x3c, 0x67, 0x72, 0x6f, 0x75, 0x70, 0x49, 0x64, 0x09, 0x70, 0x0a, 0x2e,
            0x0a, 0x24, 0x0a, 0x02, 0x0a, 0x2e, 0x3e, 0x3e, 0x23, 0x3e, 0x3c, 0x21, 0x44, 0x4f,
            0x43, 0x54, 0x59, 0x50, 0x65, 0x09, 0x3b, 0x3a, 0x3d, 0x22, 0x22, 0x3a, 0x0d, 0x2f,
            0x3e, 0x3c, 0x21, 0x44, 0x4f, 0x43, 0x54, 0x59, 0x50, 0x65, 0x09, 0x3b, 0x3a, 0x3d,
            0x22, 0x30, 0x22, 0x31, 0x22, 0x2e, 0x3c, 0x2f, 0x3e, 0x3c, 0x22, 0x3e, 0x23, 0x3e,
            0x3c, 0x21, 0x44, 0x4f, 0x43, 0x54, 0x59, 0x50, 0x65, 0x09, 0x3b, 0x3a, 0x3d, 0x22,
            0x22, 0x3a, 0x0d, 0x2f, 0x3e, 0x3c, 0x21, 0x44, 0x4f, 0x43, 0x54, 0x59, 0x50, 0x65,
            0x09, 0x3b, 0x3a, 0x3d, 0x22, 0x30, 0x22, 0x31, 0x22, 0x2e, 0x3c, 0x2f, 0x3e, 0x3c,
            0x2f, 0x3e, 0x3c, 0x2f, 0x22, 0x3e, 0x3e, 0x0a, 0x05, 0x0a, 0x23, 0x0a, 0x70, 0x0a,
            0x05, 0x0a, 0x02, 0x0a, 0x70, 0x0a, 0x00, 0x0a, 0x05, 0x0a, 0x57, 0x0a, 0x02, 0x0a,
            0x70, 0x0a, 0x23, 0x0a, 0x05, 0x0a, 0x02, 0x0a, 0x2e, 0x0a, 0x05, 0x0a, 0x23, 0x0a,
            0x70, 0x0a, 0x05, 0x0a, 0x02, 0x0a, 0x2e, 0x0a, 0x05, 0x0a, 0x57, 0x0a, 0x2e, 0x0a,
            0x49, 0x0a, 0x49, 0x0a, 0x3e,
        ];
        let s = std::str::from_utf8(bytes).expect("artifact bytes are valid UTF-8");
        let err = MavenMetadata::from_xml(s).expect_err("must reject without panicking");
        let msg = err.to_string();
        assert!(
            msg.contains("XML parse failed") || msg.contains("XML parser panicked"),
            "unexpected error: {msg}"
        );
    }
}
