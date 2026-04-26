// SPDX-License-Identifier: Apache-2.0
//! Checksum sidecar formats.
//!
//! Maven clients publish and consume `*.md5`, `*.sha1`, `*.sha256`, and
//! `*.sha512` sidecars alongside main artifacts. These are ASCII files
//! containing the lower-case hex digest with an optional trailing
//! newline or `" *filename"` comment.
//!
//! Spec: Maven Repository Layout —
//! <https://maven.apache.org/repository/layout.html>.

use sha1::{Digest as _, Sha1};
use sha2::{Sha256, Sha512};

use crate::error::MavenError;

/// Checksum algorithms a Maven client may request sidecars for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ChecksumAlgo {
    /// MD5 — legacy. Gated behind `legacy-md5` feature on the caller
    /// side; parsing accepts it unconditionally because clients still
    /// emit it by default in 2026.
    Md5,
    /// SHA-1 — the historical default, still shipped by Central.
    Sha1,
    /// SHA-256 — modern default for new deployments.
    Sha256,
    /// SHA-512 — highest-strength modern option.
    Sha512,
}

impl ChecksumAlgo {
    /// Map a file extension (without the dot) to an algorithm.
    #[must_use]
    pub fn from_extension(ext: &str) -> Option<Self> {
        match ext {
            "md5" => Some(Self::Md5),
            "sha1" => Some(Self::Sha1),
            "sha256" => Some(Self::Sha256),
            "sha512" => Some(Self::Sha512),
            _ => None,
        }
    }

    /// Canonical file extension for this algorithm.
    #[must_use]
    pub const fn extension(self) -> &'static str {
        match self {
            Self::Md5 => "md5",
            Self::Sha1 => "sha1",
            Self::Sha256 => "sha256",
            Self::Sha512 => "sha512",
        }
    }

    /// Expected hex length in characters.
    #[must_use]
    pub const fn hex_len(self) -> usize {
        match self {
            Self::Md5 => 32,
            Self::Sha1 => 40,
            Self::Sha256 => 64,
            Self::Sha512 => 128,
        }
    }
}

/// Compute the checksum of `bytes` as a lowercase hex string.
///
/// MD5 is deliberately absent; callers that have opted in to
/// [`ChecksumAlgo::Md5`] should either store client-provided sidecars
/// verbatim (the FerroRepo default) or reject the request. Returning
/// `None` keeps compilation free of the `md5` crate dependency.
#[must_use]
pub fn compute_checksum(algo: ChecksumAlgo, bytes: &[u8]) -> Option<String> {
    match algo {
        ChecksumAlgo::Sha1 => {
            let mut h = Sha1::new();
            h.update(bytes);
            Some(hex::encode(h.finalize()))
        }
        ChecksumAlgo::Sha256 => {
            let mut h = Sha256::new();
            h.update(bytes);
            Some(hex::encode(h.finalize()))
        }
        ChecksumAlgo::Sha512 => {
            let mut h = Sha512::new();
            h.update(bytes);
            Some(hex::encode(h.finalize()))
        }
        ChecksumAlgo::Md5 => None,
    }
}

/// Parse a Maven checksum sidecar body.
///
/// Accepts the bare hex form (`"0a1b2c..."`), the `"hex *filename"`
/// form emitted by `md5sum`/`sha1sum`, and a trailing newline.
///
/// # Errors
///
/// Returns [`MavenError::ChecksumMismatch`] (named for a parser error,
/// not yet a hash mismatch) if the decoded hex length differs from the
/// algorithm's expected length or contains non-hex characters.
pub fn parse_sidecar(algo: ChecksumAlgo, body: &[u8]) -> Result<String, MavenError> {
    let text = std::str::from_utf8(body)
        .map_err(|_| MavenError::ChecksumMismatch("sidecar is not valid UTF-8".into()))?;
    // Grab the first whitespace-delimited token from the first line.
    let first_line = text.lines().next().unwrap_or("").trim();
    let token = first_line.split_whitespace().next().unwrap_or("");
    let hex = token.trim().to_ascii_lowercase();

    if hex.len() != algo.hex_len() {
        return Err(MavenError::ChecksumMismatch(format!(
            "expected {} hex characters for {:?}, got {}",
            algo.hex_len(),
            algo,
            hex.len()
        )));
    }
    if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(MavenError::ChecksumMismatch(
            "sidecar contains non-hex characters".into(),
        ));
    }
    Ok(hex)
}

#[cfg(test)]
mod tests {
    use super::{ChecksumAlgo, compute_checksum, parse_sidecar};

    #[test]
    fn sha1_of_abc_is_known_vector() {
        let hex = compute_checksum(ChecksumAlgo::Sha1, b"abc").expect("sha1");
        assert_eq!(hex, "a9993e364706816aba3e25717850c26c9cd0d89d");
    }

    #[test]
    fn sha256_of_abc_is_known_vector() {
        let hex = compute_checksum(ChecksumAlgo::Sha256, b"abc").expect("sha256");
        assert_eq!(
            hex,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn parse_sidecar_bare_form() {
        let hex = parse_sidecar(
            ChecksumAlgo::Sha1,
            b"a9993e364706816aba3e25717850c26c9cd0d89d\n",
        )
        .expect("parse");
        assert_eq!(hex, "a9993e364706816aba3e25717850c26c9cd0d89d");
    }

    #[test]
    fn parse_sidecar_with_filename_suffix() {
        let hex = parse_sidecar(
            ChecksumAlgo::Sha1,
            b"a9993e364706816aba3e25717850c26c9cd0d89d *foo-1.0.jar\n",
        )
        .expect("parse");
        assert_eq!(hex, "a9993e364706816aba3e25717850c26c9cd0d89d");
    }

    #[test]
    fn parse_sidecar_rejects_wrong_length() {
        let err = parse_sidecar(ChecksumAlgo::Sha256, b"deadbeef").expect_err("reject");
        assert!(err.to_string().contains("64 hex characters"));
    }

    #[test]
    fn md5_returns_none_from_compute() {
        // MD5 is accepted for sidecar parsing but we do not compute it
        // ourselves (no md5 crate in the dependency graph).
        assert!(compute_checksum(ChecksumAlgo::Md5, b"abc").is_none());
    }
}
