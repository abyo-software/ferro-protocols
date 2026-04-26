// SPDX-License-Identifier: Apache-2.0
//! Repository-name and reference parsing.
//!
//! Spec: OCI Distribution Spec v1.1 §2 "Definitions".
//!
//! - A repository `<name>` matches
//!   `[a-z0-9]+(?:(?:(?:[._]|__|[-]*)[a-z0-9]+)+)?(?:/[a-z0-9]+(?:(?:(?:[._]|__|[-]*)[a-z0-9]+)+)?)*`
//!   with a total length <= 255 characters.
//! - A `<reference>` is either a tag (matching
//!   `[a-zA-Z0-9_][a-zA-Z0-9._-]{0,127}`) or a digest (`<algo>:<hex>`).
//!
//! The conformance suite exercises the edge cases (double underscores,
//! leading uppercase, names ending with `-`) so we enforce the regex
//! manually rather than rely on `regex::Regex` (which would pull an
//! extra dep).

use std::fmt;
use std::str::FromStr;

use ferro_blob_store::Digest;

use crate::error::{OciError, OciErrorCode};

/// Maximum total length of a repository name (spec §2).
pub const MAX_NAME_LENGTH: usize = 255;

/// Maximum length of a tag reference (spec §2: 128 characters).
pub const MAX_TAG_LENGTH: usize = 128;

/// Validate a repository name against the OCI Distribution Spec v1.1
/// name grammar.
///
/// # Errors
///
/// Returns an [`OciError`] with code `NAME_INVALID` if the name violates
/// any part of the grammar.
pub fn validate_name(name: &str) -> Result<(), OciError> {
    if name.is_empty() {
        return Err(OciError::new(
            OciErrorCode::NameInvalid,
            "repository name must not be empty",
        ));
    }
    if name.len() > MAX_NAME_LENGTH {
        return Err(OciError::new(
            OciErrorCode::NameInvalid,
            format!("repository name exceeds {MAX_NAME_LENGTH} characters"),
        ));
    }

    // The name grammar is a `/`-joined sequence of path components.
    // Each component matches
    //     [a-z0-9]+(?:(?:(?:[._]|__|[-]*)[a-z0-9]+)+)?
    // which reduces to: starts with [a-z0-9]+, ends with [a-z0-9]+,
    // and any internal run of separators is one of `.`, `_`, `__`, or
    // one-or-more `-`.
    for component in name.split('/') {
        validate_component(component)
            .map_err(|msg| OciError::new(OciErrorCode::NameInvalid, msg))?;
    }
    Ok(())
}

fn validate_component(component: &str) -> Result<(), String> {
    if component.is_empty() {
        return Err("path component must not be empty".to_owned());
    }
    let bytes = component.as_bytes();
    // Must start with an alphanumeric.
    if !is_alnum(bytes[0]) {
        return Err(format!("component `{component}` must start with [a-z0-9]"));
    }
    // Must end with an alphanumeric.
    if !is_alnum(bytes[bytes.len() - 1]) {
        return Err(format!("component `{component}` must end with [a-z0-9]"));
    }

    // Walk the component. Between alphanumeric runs, the separator
    // must be one of: `.`, `_`, `__`, or `-+`.
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if is_alnum(c) {
            i += 1;
            continue;
        }
        // Separator run.
        let start = i;
        while i < bytes.len() && !is_alnum(bytes[i]) {
            i += 1;
        }
        let sep = &component[start..i];
        if !is_valid_separator(sep) {
            return Err(format!(
                "component `{component}` contains invalid separator `{sep}`"
            ));
        }
    }
    Ok(())
}

const fn is_alnum(b: u8) -> bool {
    b.is_ascii_digit() || b.is_ascii_lowercase()
}

fn is_valid_separator(s: &str) -> bool {
    if s == "." || s == "_" || s == "__" {
        return true;
    }
    // One-or-more `-`.
    !s.is_empty() && s.bytes().all(|b| b == b'-')
}

/// Validate a tag string.
///
/// Spec §2: `[a-zA-Z0-9_][a-zA-Z0-9._-]{0,127}`.
fn is_valid_tag(tag: &str) -> bool {
    if tag.is_empty() || tag.len() > MAX_TAG_LENGTH {
        return false;
    }
    let bytes = tag.as_bytes();
    let first_ok = bytes[0].is_ascii_alphanumeric() || bytes[0] == b'_';
    if !first_ok {
        return false;
    }
    bytes[1..]
        .iter()
        .all(|&b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'))
}

/// A parsed manifest reference: either a tag or a digest.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Reference {
    /// Human-readable tag (e.g. `latest`, `v1.2.3`).
    Tag(String),
    /// Content-addressed digest.
    Digest(Digest),
}

impl Reference {
    /// True when this reference is a tag.
    #[must_use]
    pub const fn is_tag(&self) -> bool {
        matches!(self, Self::Tag(_))
    }

    /// True when this reference is a digest.
    #[must_use]
    pub const fn is_digest(&self) -> bool {
        matches!(self, Self::Digest(_))
    }

    /// Borrow the digest, if this reference is one.
    #[must_use]
    pub const fn as_digest(&self) -> Option<&Digest> {
        match self {
            Self::Digest(d) => Some(d),
            Self::Tag(_) => None,
        }
    }

    /// Borrow the tag string, if this reference is one.
    #[must_use]
    pub fn as_tag(&self) -> Option<&str> {
        match self {
            Self::Tag(t) => Some(t.as_str()),
            Self::Digest(_) => None,
        }
    }
}

impl fmt::Display for Reference {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Tag(t) => f.write_str(t),
            Self::Digest(d) => fmt::Display::fmt(d, f),
        }
    }
}

impl FromStr for Reference {
    type Err = OciError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // Digest references always contain a `:` separator followed by
        // a known algorithm prefix. Tag references do not contain `:`.
        if let Some((algo, _hex)) = s.split_once(':') {
            if algo == "sha256" || algo == "sha512" {
                let d: Digest = s.parse().map_err(|e: ferro_blob_store::DigestParseError| {
                    OciError::new(OciErrorCode::DigestInvalid, e.to_string())
                })?;
                return Ok(Self::Digest(d));
            }
            // A `:` without a known algorithm is an invalid reference.
            return Err(OciError::new(
                OciErrorCode::ManifestInvalid,
                format!("invalid reference: `{s}`"),
            ));
        }
        if !is_valid_tag(s) {
            return Err(OciError::new(
                OciErrorCode::ManifestInvalid,
                format!("invalid tag: `{s}`"),
            ));
        }
        Ok(Self::Tag(s.to_owned()))
    }
}

#[cfg(test)]
mod tests {
    use super::{Reference, validate_name};

    #[test]
    fn simple_single_component_name_is_valid() {
        assert!(validate_name("alpine").is_ok());
    }

    #[test]
    fn nested_path_name_is_valid() {
        assert!(validate_name("library/alpine").is_ok());
        assert!(validate_name("my-org/sub-project/app").is_ok());
    }

    #[test]
    fn underscore_and_dot_and_dash_separators_are_valid() {
        assert!(validate_name("foo_bar").is_ok());
        assert!(validate_name("foo__bar").is_ok());
        assert!(validate_name("foo.bar").is_ok());
        assert!(validate_name("foo-bar").is_ok());
        assert!(validate_name("foo---bar").is_ok());
    }

    #[test]
    fn uppercase_is_rejected() {
        let err = validate_name("Alpine").expect_err("uppercase invalid");
        assert_eq!(err.code.as_str(), "NAME_INVALID");
    }

    #[test]
    fn leading_separator_is_rejected() {
        assert!(validate_name("-alpine").is_err());
        assert!(validate_name(".alpine").is_err());
        assert!(validate_name("_alpine").is_err());
    }

    #[test]
    fn trailing_separator_is_rejected() {
        assert!(validate_name("alpine-").is_err());
        assert!(validate_name("alpine.").is_err());
    }

    #[test]
    fn empty_component_is_rejected() {
        assert!(validate_name("foo//bar").is_err());
        assert!(validate_name("/foo").is_err());
        assert!(validate_name("foo/").is_err());
    }

    #[test]
    fn too_long_name_is_rejected() {
        let s = "a".repeat(256);
        assert!(validate_name(&s).is_err());
    }

    #[test]
    fn tag_reference_parses() {
        let r: Reference = "v1.2.3-rc1".parse().expect("tag parse");
        assert!(r.is_tag());
        assert_eq!(r.as_tag(), Some("v1.2.3-rc1"));
    }

    #[test]
    fn digest_reference_parses() {
        let digest = format!("sha256:{}", "a".repeat(64));
        let r: Reference = digest.parse().expect("digest parse");
        assert!(r.is_digest());
        assert_eq!(r.to_string(), digest);
    }

    #[test]
    fn bad_digest_reference_is_rejected() {
        // Known algorithm prefix + wrong hex length.
        assert!("sha256:beef".parse::<Reference>().is_err());
    }

    #[test]
    fn tag_with_colon_is_rejected_as_invalid_reference() {
        // Unknown "algo" prefix before the colon.
        assert!("some:weird".parse::<Reference>().is_err());
    }
}
