// SPDX-License-Identifier: Apache-2.0
//! Maven Group-Artifact-Version (GAV) coordinates.
//!
//! A Maven coordinate uniquely identifies an artifact by `groupId`,
//! `artifactId`, `version`, optional `classifier`, and `extension` (also
//! called `packaging` in a POM). The canonical string form is:
//!
//! ```text
//! groupId:artifactId:extension[:classifier]:version
//! ```
//!
//! See Maven Repository Layout:
//! <https://maven.apache.org/repository/layout.html>.

use std::fmt;

/// Error returned when a coordinate fails validation.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CoordinateParseError {
    /// A mandatory field (`groupId`, `artifactId`, `version`) was empty.
    #[error("coordinate field `{0}` must not be empty")]
    EmptyField(&'static str),
    /// A field contained a character disallowed by Maven layout.
    #[error("coordinate field `{field}` contains illegal character `{ch}`")]
    IllegalCharacter {
        /// Which field contained the illegal character.
        field: &'static str,
        /// The offending character.
        ch: char,
    },
}

/// A fully-qualified Maven coordinate.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Coordinate {
    /// Dotted group identifier, e.g. `com.example.foo`.
    pub group_id: String,
    /// Artifact identifier, e.g. `bar-api`.
    pub artifact_id: String,
    /// Version string, e.g. `1.2.3` or `1.2.3-SNAPSHOT`.
    pub version: String,
    /// Optional classifier, e.g. `sources`, `javadoc`, `linux-x86_64`.
    pub classifier: Option<String>,
    /// File extension / packaging type, e.g. `jar`, `pom`, `war`.
    pub extension: String,
}

impl Coordinate {
    /// Construct a JAR coordinate with no classifier. Convenience for
    /// tests and the common mainline artifact case.
    pub fn new_jar(
        group_id: impl Into<String>,
        artifact_id: impl Into<String>,
        version: impl Into<String>,
    ) -> Result<Self, CoordinateParseError> {
        Self::new(group_id, artifact_id, version, None::<String>, "jar")
    }

    /// Construct a coordinate from components, validating each field.
    ///
    /// # Errors
    ///
    /// Returns a [`CoordinateParseError`] if any mandatory field is
    /// empty or contains a Maven-illegal character (slash / backslash /
    /// colon).
    pub fn new(
        group_id: impl Into<String>,
        artifact_id: impl Into<String>,
        version: impl Into<String>,
        classifier: Option<impl Into<String>>,
        extension: impl Into<String>,
    ) -> Result<Self, CoordinateParseError> {
        let group_id = group_id.into();
        let artifact_id = artifact_id.into();
        let version = version.into();
        let classifier = classifier.map(Into::into);
        let extension = extension.into();

        validate_field("groupId", &group_id)?;
        validate_field("artifactId", &artifact_id)?;
        validate_field("version", &version)?;
        validate_field("extension", &extension)?;
        if let Some(ref c) = classifier {
            validate_field("classifier", c)?;
        }

        Ok(Self {
            group_id,
            artifact_id,
            version,
            classifier,
            extension,
        })
    }

    /// Group id with dots replaced by `/` as used in the repository path.
    #[must_use]
    pub fn group_path(&self) -> String {
        self.group_id.replace('.', "/")
    }

    /// Canonical file name for this coordinate:
    /// `{artifactId}-{version}[-{classifier}].{extension}`.
    #[must_use]
    pub fn filename(&self) -> String {
        match &self.classifier {
            Some(c) => format!(
                "{}-{}-{}.{}",
                self.artifact_id, self.version, c, self.extension
            ),
            None => format!("{}-{}.{}", self.artifact_id, self.version, self.extension),
        }
    }

    /// Full repository path (without leading slash):
    /// `{groupPath}/{artifactId}/{version}/{filename}`.
    #[must_use]
    pub fn repository_path(&self) -> String {
        format!(
            "{}/{}/{}/{}",
            self.group_path(),
            self.artifact_id,
            self.version,
            self.filename()
        )
    }
}

fn validate_field(name: &'static str, value: &str) -> Result<(), CoordinateParseError> {
    if value.is_empty() {
        return Err(CoordinateParseError::EmptyField(name));
    }
    for ch in value.chars() {
        if matches!(ch, '/' | '\\' | ':') {
            return Err(CoordinateParseError::IllegalCharacter { field: name, ch });
        }
    }
    Ok(())
}

impl fmt::Display for Coordinate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.classifier {
            Some(c) => write!(
                f,
                "{}:{}:{}:{}:{}",
                self.group_id, self.artifact_id, self.extension, c, self.version
            ),
            None => write!(
                f,
                "{}:{}:{}:{}",
                self.group_id, self.artifact_id, self.extension, self.version
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Coordinate, CoordinateParseError};

    #[test]
    fn jar_filename_without_classifier() {
        let c = Coordinate::new_jar("com.example", "foo", "1.2.3").expect("ok");
        assert_eq!(c.filename(), "foo-1.2.3.jar");
    }

    #[test]
    fn repository_path_dots_become_slashes() {
        let c = Coordinate::new_jar("com.example.foo", "bar", "1.0").expect("ok");
        assert_eq!(c.repository_path(), "com/example/foo/bar/1.0/bar-1.0.jar");
    }

    #[test]
    fn classifier_is_inserted_before_extension() {
        let c = Coordinate::new("com.example", "foo", "1.0", Some("sources"), "jar").expect("ok");
        assert_eq!(c.filename(), "foo-1.0-sources.jar");
    }

    #[test]
    fn empty_group_id_rejected() {
        let err = Coordinate::new_jar("", "foo", "1.0").expect_err("reject");
        assert!(matches!(err, CoordinateParseError::EmptyField("groupId")));
    }

    #[test]
    fn slash_in_artifact_id_rejected() {
        let err = Coordinate::new_jar("com.example", "foo/bar", "1.0").expect_err("reject");
        assert!(matches!(
            err,
            CoordinateParseError::IllegalCharacter {
                field: "artifactId",
                ch: '/'
            }
        ));
    }

    #[test]
    fn display_uses_colon_form() {
        let c = Coordinate::new_jar("com.example", "foo", "1.0").expect("ok");
        assert_eq!(c.to_string(), "com.example:foo:jar:1.0");
    }
}
