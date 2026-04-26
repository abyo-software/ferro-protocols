// SPDX-License-Identifier: Apache-2.0
//! Content-addressed digest.

use std::fmt;
use std::str::FromStr;

use sha2::{Digest as _, Sha256, Sha512};
use thiserror::Error;

/// Hash algorithm used by a [`Digest`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum DigestAlgo {
    /// SHA-256 (32-byte output, 64 hex chars). Default for OCI / Maven /
    /// Cargo on the wire.
    Sha256,
    /// SHA-512 (64-byte output, 128 hex chars). Accepted by OCI manifests
    /// that advertise a `sha512:` prefix.
    Sha512,
}

impl DigestAlgo {
    /// Expected hex-encoded length in characters.
    #[must_use]
    pub const fn hex_len(self) -> usize {
        match self {
            Self::Sha256 => 64,
            Self::Sha512 => 128,
        }
    }

    /// Wire prefix used in `<algo>:<hex>` form.
    #[must_use]
    pub const fn prefix(self) -> &'static str {
        match self {
            Self::Sha256 => "sha256",
            Self::Sha512 => "sha512",
        }
    }

    fn parse_prefix(s: &str) -> Option<Self> {
        match s {
            "sha256" => Some(Self::Sha256),
            "sha512" => Some(Self::Sha512),
            _ => None,
        }
    }
}

impl fmt::Display for DigestAlgo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.prefix())
    }
}

/// Errors returned when a `<algo>:<hex>` string fails validation.
#[derive(Debug, Clone, Error, PartialEq, Eq)]
#[non_exhaustive]
pub enum DigestParseError {
    /// The input did not contain the `<algo>:<hex>` separator.
    #[error("digest must be of the form `<algo>:<hex>`")]
    MissingSeparator,
    /// The algorithm prefix is not recognised.
    #[error("unsupported digest algorithm prefix: {0:?}")]
    UnsupportedAlgo(String),
    /// The hex portion has the wrong length for the declared algorithm.
    #[error("invalid digest hex length for {algo}: expected {expected}, got {actual}")]
    BadLength {
        /// Algorithm reported by the prefix.
        algo: DigestAlgo,
        /// Expected hex length in characters.
        expected: usize,
        /// Actual hex length supplied.
        actual: usize,
    },
    /// The hex portion contains a non-hex character.
    #[error("invalid hex character {bad:?} in digest")]
    BadHex {
        /// Offending character.
        bad: char,
    },
}

/// Content-addressed identifier in `<algo>:<hex>` form.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Digest {
    algo: DigestAlgo,
    hex: String,
}

impl Digest {
    /// Construct from a known algorithm and hex string. Validates the
    /// hex length and character set.
    pub fn new(algo: DigestAlgo, hex: impl Into<String>) -> Result<Self, DigestParseError> {
        let hex = hex.into();
        Self::validate_hex(algo, &hex)?;
        Ok(Self {
            algo,
            hex: hex.to_ascii_lowercase(),
        })
    }

    /// Compute the SHA-256 digest of `bytes`.
    #[must_use]
    pub fn sha256_of(bytes: &[u8]) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        let result = hasher.finalize();
        Self {
            algo: DigestAlgo::Sha256,
            hex: hex::encode(result),
        }
    }

    /// Compute the SHA-512 digest of `bytes`.
    #[must_use]
    pub fn sha512_of(bytes: &[u8]) -> Self {
        let mut hasher = Sha512::new();
        hasher.update(bytes);
        let result = hasher.finalize();
        Self {
            algo: DigestAlgo::Sha512,
            hex: hex::encode(result),
        }
    }

    /// Algorithm this digest was produced with.
    #[must_use]
    pub const fn algo(&self) -> DigestAlgo {
        self.algo
    }

    /// Lower-case hex string of the digest body (no algorithm prefix).
    #[must_use]
    pub fn hex(&self) -> &str {
        &self.hex
    }

    fn validate_hex(algo: DigestAlgo, hex: &str) -> Result<(), DigestParseError> {
        let expected = algo.hex_len();
        if hex.len() != expected {
            return Err(DigestParseError::BadLength {
                algo,
                expected,
                actual: hex.len(),
            });
        }
        if let Some(bad) = hex.chars().find(|c| !c.is_ascii_hexdigit()) {
            return Err(DigestParseError::BadHex { bad });
        }
        Ok(())
    }
}

impl fmt::Display for Digest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.algo, self.hex)
    }
}

impl FromStr for Digest {
    type Err = DigestParseError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (prefix, hex) = s
            .split_once(':')
            .ok_or(DigestParseError::MissingSeparator)?;
        let algo = DigestAlgo::parse_prefix(prefix)
            .ok_or_else(|| DigestParseError::UnsupportedAlgo(prefix.to_string()))?;
        Self::new(algo, hex)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_of_round_trips() {
        let d = Digest::sha256_of(b"hello");
        assert_eq!(d.algo(), DigestAlgo::Sha256);
        assert_eq!(d.hex().len(), 64);
        let s = d.to_string();
        let parsed: Digest = s.parse().unwrap();
        assert_eq!(parsed, d);
    }

    #[test]
    fn sha512_of_round_trips() {
        let d = Digest::sha512_of(b"hello");
        assert_eq!(d.algo(), DigestAlgo::Sha512);
        assert_eq!(d.hex().len(), 128);
        let s = d.to_string();
        let parsed: Digest = s.parse().unwrap();
        assert_eq!(parsed, d);
    }

    #[test]
    fn parse_missing_separator() {
        assert!(matches!(
            "abc".parse::<Digest>(),
            Err(DigestParseError::MissingSeparator)
        ));
    }

    #[test]
    fn parse_unsupported_algo() {
        assert!(matches!(
            "md5:abc".parse::<Digest>(),
            Err(DigestParseError::UnsupportedAlgo(_))
        ));
    }

    #[test]
    fn parse_bad_length() {
        assert!(matches!(
            "sha256:deadbeef".parse::<Digest>(),
            Err(DigestParseError::BadLength { .. })
        ));
    }

    #[test]
    fn parse_bad_hex() {
        let bogus = format!("sha256:{}", "z".repeat(64));
        assert!(matches!(
            bogus.parse::<Digest>(),
            Err(DigestParseError::BadHex { bad: 'z' })
        ));
    }

    #[test]
    fn upper_case_hex_is_normalized() {
        let upper = format!("sha256:{}", "A".repeat(64));
        let d: Digest = upper.parse().unwrap();
        assert!(d.hex().chars().all(|c| !c.is_ascii_uppercase()));
    }
}
