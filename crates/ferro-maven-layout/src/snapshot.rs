// SPDX-License-Identifier: Apache-2.0
//! SNAPSHOT version handling.
//!
//! Maven distinguishes two SNAPSHOT shapes:
//!
//! - the *base* SNAPSHOT the client uploads: `1.2.3-SNAPSHOT`, and
//! - the *timestamped* SNAPSHOT the server rewrites filenames to on
//!   storage: `1.2.3-20260423.123045-1`, where the trailing `-1` is an
//!   increasing build number per base version.
//!
//! Spec: Maven Repository Metadata —
//! <https://maven.apache.org/ref/3.9.6/maven-repository-metadata/repository-metadata.html>.

use chrono::{DateTime, Utc};

/// Returns `true` if `version` ends with the literal `-SNAPSHOT` suffix.
#[must_use]
pub fn is_snapshot_version(version: &str) -> bool {
    version.ends_with("-SNAPSHOT")
}

/// Strip the `-SNAPSHOT` suffix to obtain the base version, if present.
#[must_use]
pub fn base_version(version: &str) -> &str {
    version.strip_suffix("-SNAPSHOT").unwrap_or(version)
}

/// Format a SNAPSHOT timestamp as Maven wants:
/// `yyyyMMdd.HHmmss` in UTC.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SnapshotTimestamp(pub DateTime<Utc>);

impl SnapshotTimestamp {
    /// Format as `yyyyMMdd.HHmmss`.
    #[must_use]
    pub fn format(&self) -> String {
        self.0.format("%Y%m%d.%H%M%S").to_string()
    }

    /// Compose the timestamped version string:
    /// `{baseVersion}-{timestamp}-{buildNumber}`.
    ///
    /// Example: base `1.2.3-SNAPSHOT`, timestamp `20260423.123045`,
    /// build `1` -> `1.2.3-20260423.123045-1`.
    #[must_use]
    pub fn compose_version(&self, base_snapshot: &str, build_number: u32) -> String {
        let base = base_version(base_snapshot);
        format!("{base}-{}-{build_number}", self.format())
    }

    /// Current wall clock as a [`SnapshotTimestamp`].
    #[must_use]
    pub fn now() -> Self {
        Self(Utc::now())
    }
}

#[cfg(test)]
mod tests {
    use super::{SnapshotTimestamp, base_version, is_snapshot_version};
    use chrono::TimeZone;

    #[test]
    fn snapshot_suffix_detection() {
        assert!(is_snapshot_version("1.0-SNAPSHOT"));
        assert!(is_snapshot_version("1.2.3-SNAPSHOT"));
        assert!(!is_snapshot_version("1.0"));
        assert!(!is_snapshot_version("1.0-20260423.000000-1"));
    }

    #[test]
    fn base_version_strips_suffix() {
        assert_eq!(base_version("1.2.3-SNAPSHOT"), "1.2.3");
        assert_eq!(base_version("1.2.3"), "1.2.3");
    }

    #[test]
    fn timestamp_formats_as_maven_wants() {
        let t = SnapshotTimestamp(
            chrono::Utc
                .with_ymd_and_hms(2026, 4, 23, 12, 30, 45)
                .single()
                .expect("valid ts"),
        );
        assert_eq!(t.format(), "20260423.123045");
    }

    #[test]
    fn compose_version_inserts_timestamp_and_build_number() {
        let t = SnapshotTimestamp(
            chrono::Utc
                .with_ymd_and_hms(2026, 4, 23, 12, 30, 45)
                .single()
                .expect("valid ts"),
        );
        let v = t.compose_version("1.2.3-SNAPSHOT", 7);
        assert_eq!(v, "1.2.3-20260423.123045-7");
    }
}
