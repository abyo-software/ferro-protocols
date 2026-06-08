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

    let class = checksum.map_or(PathClass::Artifact, PathClass::Checksum);

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
    use super::{PathClass, layout_is_snapshot, parse_layout_path};
    use crate::checksum::ChecksumAlgo;

    #[test]
    fn three_segment_metadata_path_is_accepted() {
        // Exactly three segments: `{group}/{artifactId}/maven-metadata.xml`.
        // Distinguishes `segments.len() < 3` from `<= 3` at the top-level
        // guard (line 65) and inside `classify_metadata` (line 120):
        // `< 3` admits this path, `<= 3` / `== 3` would reject it.
        let p = parse_layout_path("com/example/maven-metadata.xml").expect("ok");
        assert_eq!(p.coordinate.group_id, "com");
        assert_eq!(p.coordinate.artifact_id, "example");
        assert!(matches!(
            p.class,
            PathClass::Metadata {
                version_level: false,
                checksum: None
            }
        ));
    }

    #[test]
    fn four_segment_artifact_path_is_accepted() {
        // Minimal artifact path with exactly four segments. Catches the
        // `segments.len() < 4` boundary (line 89): replacing `<` with
        // `<=` would reject this valid four-segment path.
        let p = parse_layout_path("g/foo/1.0/foo-1.0.jar").expect("ok");
        assert_eq!(p.coordinate.group_id, "g");
        assert_eq!(p.coordinate.artifact_id, "foo");
        assert_eq!(p.coordinate.version, "1.0");
        assert_eq!(p.coordinate.extension, "jar");
        assert_eq!(p.class, PathClass::Artifact);
    }

    #[test]
    fn three_segment_artifact_path_is_rejected() {
        // Exactly three non-metadata segments must fail the line-89
        // `< 4` guard. With `<` the path is rejected; with `==` (3 == 4
        // is false) it would slip through and fail later with a
        // different message, so assert the exact "fewer than 4" wording.
        let err = parse_layout_path("foo/1.0/foo-1.0.jar").expect_err("reject");
        assert!(
            err.to_string().contains("fewer than 4 segments"),
            "unexpected: {err}"
        );
    }

    #[test]
    fn deep_version_level_metadata_resolves_group_and_artifact() {
        // before_file has six entries, so `len - 2 = 4` differs from
        // `len / 2 = 3`. Asserting the exact artifactId / groupId catches
        // the `- 2` → `/ 2` subtraction mutants (lines 131 & 132): the
        // mutant would pick `before_file[3]` ("d") as the artifactId and
        // `a.b.c` as the groupId.
        let p =
            parse_layout_path("a/b/c/d/foo/1.0-SNAPSHOT/maven-metadata.xml").expect("ok");
        assert_eq!(p.coordinate.artifact_id, "foo");
        assert_eq!(p.coordinate.group_id, "a.b.c.d");
        assert_eq!(p.coordinate.version, "1.0-SNAPSHOT");
        assert!(matches!(
            p.class,
            PathClass::Metadata {
                version_level: true,
                ..
            }
        ));
    }

    #[test]
    fn empty_classifier_with_extension_is_rejected() {
        // Filename `foo-1.0-.jar` yields tail `.jar`, i.e. an empty
        // classifier with a non-empty extension. The `is_empty() ||
        // is_empty()` guard in `split_classifier_and_extension` must
        // reject it; an `&&` mutant would accept an empty classifier.
        let err = parse_layout_path("g/foo/1.0/foo-1.0-.jar").expect_err("reject");
        assert!(
            err.to_string().contains("classifier.extension"),
            "unexpected: {err}"
        );
    }

    #[test]
    fn layout_is_snapshot_reflects_version() {
        // Pin both truth values so the function body cannot be replaced
        // by a constant `true` or `false`.
        let snap = parse_layout_path(
            "com/example/foo/1.0-SNAPSHOT/foo-1.0-SNAPSHOT.jar",
        )
        .expect("ok");
        assert!(layout_is_snapshot(&snap));

        let release = parse_layout_path("com/example/foo/1.0/foo-1.0.jar").expect("ok");
        assert!(!layout_is_snapshot(&release));
    }

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

    #[test]
    fn fuzz_crash_path_traversal_input_is_rejected() {
        // Regression for the `parse_layout_path` fuzz crash
        // (crash-b8d04a21..): bytes `\x17/../\x00//..-\x00.t`. The path
        // contained a `..` version/artifact component plus NUL bytes that
        // previously slipped through `Coordinate::new` validation and let
        // `repository_path` re-render a `..` traversal segment — a
        // path-traversal security bug for the Maven repository layout.
        let crash = "\u{17}/../\u{0}//..-\u{0}.t";
        parse_layout_path(crash).expect_err("traversal crash input must be rejected");
    }

    #[test]
    fn dotdot_component_path_is_rejected() {
        // A clean `..` groupId / artifactId / version component must be
        // rejected so a re-rendered path cannot escape the repo root.
        for path in [
            "../foo/1.0/foo-1.0.jar",
            "g/../1.0/foo-1.0.jar",
            "g/foo/../foo-1.0.jar",
        ] {
            parse_layout_path(path).expect_err("`..` component path must be rejected");
        }
    }

    #[test]
    fn accepted_paths_never_render_a_traversal_segment() {
        // The invariant the fuzzer enforces: for every accepted artifact
        // or checksum path, the re-rendered repository path contains no
        // `..` (or `.`) traversal segment.
        for path in [
            "com/example/foo/1.0/foo-1.0.jar",
            "com/example/foo/1.0/foo-1.0-sources.jar",
            "com/example/foo/1.0/foo-1.0.jar.sha1",
            "com/example/foo/1.0-SNAPSHOT/foo-1.0-SNAPSHOT-dist.tar.gz",
        ] {
            let p = parse_layout_path(path).expect("ok");
            if matches!(p.class, PathClass::Artifact | PathClass::Checksum(_)) {
                for seg in p.coordinate.repository_path().split('/') {
                    assert_ne!(seg, "..", "traversal segment in {path}");
                    assert_ne!(seg, ".", "current-dir segment in {path}");
                }
            }
        }
    }
}
