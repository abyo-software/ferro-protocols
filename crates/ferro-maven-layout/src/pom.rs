// SPDX-License-Identifier: Apache-2.0
//! Minimal POM (`pom.xml`) deserialization.
//!
//! The Maven POM is a large XML document; this module extracts only the
//! fields FerroRepo needs to validate uploads and build metadata index
//! records:
//!
//! - `modelVersion`
//! - `groupId` / `artifactId` / `version`
//! - `packaging`
//! - `parent` (used as a fallback for missing groupId / version)
//!
//! Spec: POM Reference —
//! <https://maven.apache.org/ref/3.9.6/maven-model/maven.html>.

use serde::Deserialize;

use crate::error::MavenError;

/// Parsed POM document (subset).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pom {
    /// `<modelVersion>` — always `"4.0.0"` in modern POMs.
    pub model_version: Option<String>,
    /// Effective `groupId`, with parent fallback already applied.
    pub group_id: String,
    /// `<artifactId>`.
    pub artifact_id: String,
    /// Effective `version`, with parent fallback already applied.
    pub version: String,
    /// `<packaging>`, defaulting to `"jar"` when absent.
    pub packaging: String,
    /// Parsed `<parent>` block, if present.
    pub parent: Option<PomParent>,
}

/// `<parent>` sub-document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PomParent {
    /// Parent `<groupId>`.
    pub group_id: String,
    /// Parent `<artifactId>`.
    pub artifact_id: String,
    /// Parent `<version>`.
    pub version: String,
}

/// Parse a POM document.
///
/// # Errors
///
/// Returns [`MavenError::InvalidPom`] if the XML is malformed or if
/// after parent-fallback resolution `groupId`, `artifactId`, or
/// `version` are still empty.
pub fn parse_pom(xml: &str) -> Result<Pom, MavenError> {
    let raw: RawPom = quick_xml::de::from_str(xml)
        .map_err(|e| MavenError::InvalidPom(format!("XML parse failed: {e}")))?;

    let parent = raw.parent.as_ref().map(|p| PomParent {
        group_id: p.group_id.clone().unwrap_or_default(),
        artifact_id: p.artifact_id.clone().unwrap_or_default(),
        version: p.version.clone().unwrap_or_default(),
    });

    // Maven rule: groupId and version may be inherited from the <parent>
    // block when absent.
    let group_id = raw
        .group_id
        .clone()
        .or_else(|| parent.as_ref().map(|p| p.group_id.clone()))
        .unwrap_or_default();
    let version = raw
        .version
        .clone()
        .or_else(|| parent.as_ref().map(|p| p.version.clone()))
        .unwrap_or_default();

    let artifact_id = raw.artifact_id.clone().unwrap_or_default();

    if group_id.is_empty() {
        return Err(MavenError::InvalidPom(
            "POM missing groupId (and no <parent><groupId>)".into(),
        ));
    }
    if artifact_id.is_empty() {
        return Err(MavenError::InvalidPom("POM missing artifactId".into()));
    }
    if version.is_empty() {
        return Err(MavenError::InvalidPom(
            "POM missing version (and no <parent><version>)".into(),
        ));
    }

    let packaging = raw.packaging.clone().unwrap_or_else(|| "jar".to_string());

    Ok(Pom {
        model_version: raw.model_version,
        group_id,
        artifact_id,
        version,
        packaging,
        parent,
    })
}

#[derive(Debug, Deserialize)]
struct RawPom {
    #[serde(rename = "modelVersion", default)]
    model_version: Option<String>,
    #[serde(rename = "groupId", default)]
    group_id: Option<String>,
    #[serde(rename = "artifactId", default)]
    artifact_id: Option<String>,
    #[serde(rename = "version", default)]
    version: Option<String>,
    #[serde(rename = "packaging", default)]
    packaging: Option<String>,
    #[serde(rename = "parent", default)]
    parent: Option<RawParent>,
}

#[derive(Debug, Deserialize)]
struct RawParent {
    #[serde(rename = "groupId", default)]
    group_id: Option<String>,
    #[serde(rename = "artifactId", default)]
    artifact_id: Option<String>,
    #[serde(rename = "version", default)]
    version: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::parse_pom;

    const FULL_POM: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<project xmlns="http://maven.apache.org/POM/4.0.0">
    <modelVersion>4.0.0</modelVersion>
    <groupId>com.example</groupId>
    <artifactId>foo</artifactId>
    <version>1.2.3</version>
    <packaging>jar</packaging>
</project>"#;

    const PARENT_INHERITED_POM: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<project xmlns="http://maven.apache.org/POM/4.0.0">
    <modelVersion>4.0.0</modelVersion>
    <parent>
        <groupId>com.example.parent</groupId>
        <artifactId>parent-pom</artifactId>
        <version>9.9.9</version>
    </parent>
    <artifactId>child</artifactId>
</project>"#;

    #[test]
    fn parses_full_pom() {
        let p = parse_pom(FULL_POM).expect("ok");
        assert_eq!(p.group_id, "com.example");
        assert_eq!(p.artifact_id, "foo");
        assert_eq!(p.version, "1.2.3");
        assert_eq!(p.packaging, "jar");
        assert_eq!(p.model_version.as_deref(), Some("4.0.0"));
    }

    #[test]
    fn inherits_group_and_version_from_parent() {
        let p = parse_pom(PARENT_INHERITED_POM).expect("ok");
        assert_eq!(p.group_id, "com.example.parent");
        assert_eq!(p.artifact_id, "child");
        assert_eq!(p.version, "9.9.9");
        assert_eq!(p.packaging, "jar");
        let parent = p.parent.expect("parent present");
        assert_eq!(parent.artifact_id, "parent-pom");
    }

    #[test]
    fn missing_artifact_id_fails() {
        let bad = r"<project><groupId>a</groupId><version>1</version></project>";
        let err = parse_pom(bad).expect_err("reject");
        assert!(err.to_string().contains("artifactId"));
    }

    #[test]
    fn malformed_xml_fails() {
        let err = parse_pom("<project><artifactId>oops").expect_err("reject");
        assert!(err.to_string().contains("XML parse failed"));
    }
}
