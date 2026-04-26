// SPDX-License-Identifier: Apache-2.0
//! Maven layout path parser.
//!
//! Translates between:
//!
//! - an incoming URL path such as
//!   `com/example/foo/1.0/foo-1.0.jar`, and
//! - a structured [`Coordinate`] plus a [`PathClass`] marker that
//!   distinguishes the artifact itself, a checksum sidecar, or a
//!   `maven-metadata.xml` document.
//!
//! Spec: Maven Repository Layout —
//! <https://maven.apache.org/repository/layout.html>.

use crate::checksum::ChecksumAlgo;
use crate::coordinate::Coordinate;
use crate::error::MavenError;
use crate::snapshot::is_snapshot_version;

/// Result of parsing a Maven repository path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayoutPath {
    /// The coordinate identified by the path.
    pub coordinate: Coordinate,
    /// What kind of resource the path addresses.
    pub class: PathClass,
}

/// Classification of a Maven layout path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathClass {
    /// A main artifact (jar, pom, war, tar.gz, ...).
    Artifact,
    /// A checksum sidecar alongside an artifact.
    Checksum(ChecksumAlgo),
    /// A `maven-metadata.xml` or `maven-metadata.xml.sha1` etc. under
    /// the artifactId directory (groupLevel = false) or an optional
    /// version-level metadata for SNAPSHOT builds.
    Metadata {
        /// Whether the metadata path is under a `version/` directory
        /// (true = SNAPSHOT timestamp metadata, false = artifact index).
        version_level: bool,
        /// Optional checksum algorithm for metadata sidecars.
        checksum: Option<ChecksumAlgo>,
    },
}

/// Parse a layout path into a structured form.
///
/// Accepts paths with or without a leading slash. The path must contain
/// at least three segments: `{groupPath}/{artifactId}/{version}/{filename}`.
///
/// # Errors
///
/// Returns [`MavenError::InvalidPath`] if the path has fewer than four
/// segments or the filename does not match the expected
/// `{artifactId}-{version}[-{classifier}].{extension}` shape. Paths that
/// point at a `maven-metadata.xml` (with or without checksum sidecar
/// suffix) are classified with [`PathClass::Metadata`] regardless of
/// filename shape.
pub fn parse_layout_path(path: &str) -> Result<LayoutPath, MavenError> {
    let trimmed = path.trim_start_matches('/');
    let segments: Vec<&str> = trimmed.split('/').filter(|s| !s.is_empty()).collect();

    if segments.len() < 3 {
        return Err(MavenError::InvalidPath(format!(
            "path `{path}` has fewer than 3 segments"
        )));
    }

    let filename = segments
        .last()
        .copied()
        .ok_or_else(|| MavenError::InvalidPath("path has no filename".into()))?;

    // Detect metadata documents: the filename begins with
    // `maven-metadata.xml` and may optionally be followed by a checksum
    // extension.
    if let Some(kind) = maven_metadata_suffix(filename) {
        let checksum = match kind {
            MetadataKind::Raw => None,
            MetadataKind::Sidecar(a) => Some(a),
        };
        return classify_metadata(&segments, checksum);
    }

    // Otherwise the path is {groupPath..}/{artifactId}/{version}/{filename}.
    // Require at least 4 segments (group has >=1 segment).
    if segments.len() < 4 {
        return Err(MavenError::InvalidPath(format!(
            "artifact path `{path}` has fewer than 4 segments"
        )));
    }

    let version = segments[segments.len() - 2];
    let artifact_id = segments[segments.len() - 3];
    let group_segments = &segments[..segments.len() - 3];
    let group_id = group_segments.join(".");

    let (stripped, checksum) = strip_checksum_suffix(filename);
    let (classifier, extension) = split_filename(artifact_id, version, stripped)?;

    let coordinate = Coordinate::new(group_id, artifact_id, version, classifier, extension)
        .map_err(|e| MavenError::InvalidPath(format!("{e}")))?;

    let class = match checksum {
        Some(algo) => PathClass::Checksum(algo),
        None => PathClass::Artifact,
    };

    Ok(LayoutPath { coordinate, class })
}

fn classify_metadata(
    segments: &[&str],
    checksum: Option<ChecksumAlgo>,
) -> Result<LayoutPath, MavenError> {
    // Metadata can live at either
    //   groupPath/artifactId/maven-metadata.xml   (>=2 segments before file)
    //   groupPath/artifactId/version/maven-metadata.xml (>=3 segments before file)
    // We heuristically classify by whether the penultimate segment looks
    // like a version (contains a digit).
    if segments.len() < 3 {
        return Err(MavenError::InvalidPath(
            "metadata path must have at least 2 path components before the filename".into(),
        ));
    }
    let before_file = &segments[..segments.len() - 1];
    let last = before_file.last().copied().unwrap_or_default();
    let version_level = last.chars().any(|c| c.is_ascii_digit());

    if version_level && before_file.len() >= 3 {
        let version = last.to_string();
        let artifact_id = before_file[before_file.len() - 2].to_string();
        let group_id = before_file[..before_file.len() - 2].join(".");
        let coordinate = Coordinate::new(group_id, artifact_id, version, None::<String>, "pom")
            .map_err(|e| MavenError::InvalidPath(format!("{e}")))?;
        Ok(LayoutPath {
            coordinate,
            class: PathClass::Metadata {
                version_level: true,
                checksum,
            },
        })
    } else {
        let artifact_id = last.to_string();
        let group_id = before_file[..before_file.len() - 1].join(".");
        // Use an "index" placeholder version for the coordinate since
        // artifactId-level metadata is not tied to a specific version.
        let coordinate = Coordinate::new(group_id, artifact_id, "index", None::<String>, "pom")
            .map_err(|e| MavenError::InvalidPath(format!("{e}")))?;
        Ok(LayoutPath {
            coordinate,
            class: PathClass::Metadata {
                version_level: false,
                checksum,
            },
        })
    }
}

/// Marker for a `maven-metadata.xml` classification decision.
enum MetadataKind {
    /// Raw metadata document.
    Raw,
    /// Checksum sidecar alongside the metadata.
    Sidecar(ChecksumAlgo),
}

fn maven_metadata_suffix(name: &str) -> Option<MetadataKind> {
    if name == "maven-metadata.xml" {
        return Some(MetadataKind::Raw);
    }
    let rest = name.strip_prefix("maven-metadata.xml.")?;
    ChecksumAlgo::from_extension(rest).map(MetadataKind::Sidecar)
}

fn strip_checksum_suffix(name: &str) -> (&str, Option<ChecksumAlgo>) {
    if let Some((stem, ext)) = name.rsplit_once('.')
        && let Some(algo) = ChecksumAlgo::from_extension(ext)
    {
        return (stem, Some(algo));
    }
    (name, None)
}

/// Split `{artifactId}-{version}[-{classifier}].{extension}` into its
/// classifier and extension.
fn split_filename(
    artifact_id: &str,
    version: &str,
    filename: &str,
) -> Result<(Option<String>, String), MavenError> {
    // Strip the prefix `{artifactId}-{version}` first.
    let prefix = format!("{artifact_id}-{version}");
    let rest = filename.strip_prefix(&prefix).ok_or_else(|| {
        MavenError::InvalidPath(format!(
            "filename `{filename}` does not start with `{prefix}`"
        ))
    })?;

    if let Some(tail) = rest.strip_prefix('-') {
        // Classifier present: `-{classifier}.{extension}`.
        // Extension is everything after the last dot; support
        // compound extensions (`tar.gz`, `tar.bz2`) by matching known
        // patterns.
        let (classifier, extension) = split_classifier_and_extension(tail).ok_or_else(|| {
            MavenError::InvalidPath(format!(
                "filename tail `{tail}` must be `classifier.extension`"
            ))
        })?;
        Ok((Some(classifier), extension))
    } else if let Some(tail) = rest.strip_prefix('.') {
        Ok((None, tail.to_string()))
    } else {
        Err(MavenError::InvalidPath(format!(
            "filename `{filename}` has no extension separator"
        )))
    }
}

fn split_classifier_and_extension(tail: &str) -> Option<(String, String)> {
    // Recognise compound extensions first.
    const COMPOUND: &[&str] = &["tar.gz", "tar.bz2", "tar.xz", "tar.zst"];
    for compound in COMPOUND {
        let dotted = format!(".{compound}");
        if let Some(classifier) = tail.strip_suffix(&dotted)
            && !classifier.is_empty()
        {
            return Some((classifier.to_string(), (*compound).to_string()));
        }
    }
    let dot = tail.rfind('.')?;
    let classifier = &tail[..dot];
    let extension = &tail[dot + 1..];
    if classifier.is_empty() || extension.is_empty() {
        return None;
    }
    Some((classifier.to_string(), extension.to_string()))
}

/// Convenience: check whether a parsed [`LayoutPath`] sits on a SNAPSHOT
/// version.
#[must_use]
pub fn layout_is_snapshot(path: &LayoutPath) -> bool {
    is_snapshot_version(&path.coordinate.version)
}

#[cfg(test)]
mod tests {
    use super::{PathClass, parse_layout_path};
    use crate::checksum::ChecksumAlgo;

    #[test]
    fn parses_simple_jar_path() {
        let p = parse_layout_path("com/example/foo/1.0/foo-1.0.jar").expect("ok");
        assert_eq!(p.coordinate.group_id, "com.example");
        assert_eq!(p.coordinate.artifact_id, "foo");
        assert_eq!(p.coordinate.version, "1.0");
        assert_eq!(p.coordinate.extension, "jar");
        assert_eq!(p.coordinate.classifier, None);
        assert_eq!(p.class, PathClass::Artifact);
    }

    #[test]
    fn parses_classifier_jar() {
        let p = parse_layout_path("com/example/foo/1.0/foo-1.0-sources.jar").expect("ok");
        assert_eq!(p.coordinate.classifier.as_deref(), Some("sources"));
        assert_eq!(p.coordinate.extension, "jar");
    }

    #[test]
    fn parses_sha1_sidecar() {
        let p = parse_layout_path("com/example/foo/1.0/foo-1.0.jar.sha1").expect("ok");
        assert_eq!(p.coordinate.extension, "jar");
        assert_eq!(p.class, PathClass::Checksum(ChecksumAlgo::Sha1));
    }

    #[test]
    fn parses_pom() {
        let p = parse_layout_path("com/example/foo/1.0/foo-1.0.pom").expect("ok");
        assert_eq!(p.coordinate.extension, "pom");
    }

    #[test]
    fn parses_metadata_under_artifact_id() {
        let p = parse_layout_path("com/example/foo/maven-metadata.xml").expect("ok");
        assert!(matches!(
            p.class,
            PathClass::Metadata {
                version_level: false,
                checksum: None
            }
        ));
    }

    #[test]
    fn parses_metadata_under_version() {
        let p = parse_layout_path("com/example/foo/1.0-SNAPSHOT/maven-metadata.xml").expect("ok");
        assert!(matches!(
            p.class,
            PathClass::Metadata {
                version_level: true,
                ..
            }
        ));
    }

    #[test]
    fn rejects_too_short_path() {
        let err = parse_layout_path("foo/1.0").expect_err("reject");
        assert!(err.to_string().contains("fewer than 3 segments"));
    }

    #[test]
    fn compound_tar_gz_extension_preserved() {
        let p = parse_layout_path("com/example/foo/1.0/foo-1.0-dist.tar.gz").expect("ok");
        assert_eq!(p.coordinate.extension, "tar.gz");
        assert_eq!(p.coordinate.classifier.as_deref(), Some("dist"));
    }

    #[test]
    fn round_trip_path_to_coordinate_and_back() {
        let p = parse_layout_path("com/example/foo/1.0/foo-1.0-sources.jar").expect("ok");
        assert_eq!(
            p.coordinate.repository_path(),
            "com/example/foo/1.0/foo-1.0-sources.jar"
        );
    }
}
