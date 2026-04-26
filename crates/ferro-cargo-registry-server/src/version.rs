// SPDX-License-Identifier: Apache-2.0
//! Lightweight semver validator for Cargo registry versions.
//!
//! Cargo versions follow SemVer 2.0 exactly. This module implements the
//! validator surface Phase 1 needs (cheap `is_valid_semver`); a full
//! semver type lives in `ferrorepo-common` in Phase 2 once the npm and
//! Cargo crates can share it.
//!
//! Reference: <https://semver.org/> and `cargo::util::semver`.

use regex::Regex;
use std::sync::LazyLock;

static SEMVER: LazyLock<Regex> = LazyLock::new(|| {
    // SemVer 2.0 grammar, condensed:
    //   MAJOR.MINOR.PATCH(-<prerelease>)?(+<build>)?
    Regex::new(concat!(
        r"^(?P<major>0|[1-9]\d*)\.(?P<minor>0|[1-9]\d*)\.(?P<patch>0|[1-9]\d*)",
        r"(?:-(?P<prerelease>",
        r"(?:0|[1-9]\d*|\d*[a-zA-Z-][0-9a-zA-Z-]*)",
        r"(?:\.(?:0|[1-9]\d*|\d*[a-zA-Z-][0-9a-zA-Z-]*))*",
        r"))?",
        r"(?:\+(?P<build>[0-9a-zA-Z-]+(?:\.[0-9a-zA-Z-]+)*))?$",
    ))
    .expect("static semver regex compiles")
});

/// Return `true` iff `v` is a legal SemVer 2.0 version.
#[must_use]
pub fn is_valid_semver(v: &str) -> bool {
    SEMVER.is_match(v)
}

#[cfg(test)]
mod tests {
    use super::is_valid_semver;

    #[test]
    fn accepts_common_versions() {
        assert!(is_valid_semver("1.0.0"));
        assert!(is_valid_semver("0.1.0"));
        assert!(is_valid_semver("1.2.3-alpha.1"));
        assert!(is_valid_semver("1.2.3+build.5"));
        assert!(is_valid_semver("1.2.3-rc.1+sha.abc"));
    }

    #[test]
    fn rejects_non_semver() {
        assert!(!is_valid_semver(""));
        assert!(!is_valid_semver("1"));
        assert!(!is_valid_semver("1.0"));
        assert!(!is_valid_semver("v1.0.0"));
        assert!(!is_valid_semver("1.0.0-"));
    }
}
