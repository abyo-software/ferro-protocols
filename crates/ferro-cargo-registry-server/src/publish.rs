// SPDX-License-Identifier: Apache-2.0
//! Publish request body parser.
//!
//! Reference:
//! `doc.rust-lang.org/cargo/reference/registry-web-api.html#publish`.
//!
//! The body layout is:
//!
//! ```text
//! ┌──────────────────────┐
//! │ 4-byte LE metadata_len│
//! ├──────────────────────┤
//! │ metadata JSON bytes   │
//! ├──────────────────────┤
//! │ 4-byte LE crate_len   │
//! ├──────────────────────┤
//! │ .crate tarball bytes  │
//! └──────────────────────┘
//! ```

use bytes::Bytes;
use serde_json::Value;

use crate::error::CargoError;

/// Decoded publish request.
#[derive(Debug, Clone)]
pub struct PublishRequest {
    /// Manifest JSON (name / vers / deps / features / `rust_version` / ...).
    pub manifest: Value,
    /// Raw `.crate` tarball bytes.
    pub tarball: Bytes,
}

/// Parse a publish body.
///
/// # Errors
/// Returns [`CargoError::InvalidPublish`] on any length / JSON failure.
pub fn parse(body: &[u8]) -> Result<PublishRequest, CargoError> {
    let (metadata_len, rest) = read_u32_le(body)?;
    let metadata_len = usize::try_from(metadata_len)
        .map_err(|_| CargoError::InvalidPublish("metadata_len overflow".into()))?;
    if rest.len() < metadata_len {
        return Err(CargoError::InvalidPublish("metadata body truncated".into()));
    }
    let (metadata_bytes, rest) = rest.split_at(metadata_len);
    let manifest: Value = serde_json::from_slice(metadata_bytes)
        .map_err(|e| CargoError::InvalidPublish(format!("metadata JSON parse: {e}")))?;

    let (crate_len, rest) = read_u32_le(rest)?;
    let crate_len = usize::try_from(crate_len)
        .map_err(|_| CargoError::InvalidPublish("crate_len overflow".into()))?;
    if rest.len() < crate_len {
        return Err(CargoError::InvalidPublish("crate body truncated".into()));
    }
    let tarball = rest[..crate_len].to_vec();
    Ok(PublishRequest {
        manifest,
        tarball: Bytes::from(tarball),
    })
}

fn read_u32_le(body: &[u8]) -> Result<(u32, &[u8]), CargoError> {
    if body.len() < 4 {
        return Err(CargoError::InvalidPublish(
            "body too short for 4-byte length prefix".into(),
        ));
    }
    let (len_bytes, rest) = body.split_at(4);
    let mut arr = [0u8; 4];
    arr.copy_from_slice(len_bytes);
    Ok((u32::from_le_bytes(arr), rest))
}

/// Build a synthetic publish body for tests or round-trip tooling.
#[must_use]
pub fn encode(manifest: &Value, tarball: &[u8]) -> Vec<u8> {
    let metadata_bytes = serde_json::to_vec(manifest).unwrap_or_default();
    let mlen: u32 = u32::try_from(metadata_bytes.len()).unwrap_or(u32::MAX);
    let clen: u32 = u32::try_from(tarball.len()).unwrap_or(u32::MAX);
    let mut out = Vec::with_capacity(metadata_bytes.len() + tarball.len() + 8);
    out.extend_from_slice(&mlen.to_le_bytes());
    out.extend_from_slice(&metadata_bytes);
    out.extend_from_slice(&clen.to_le_bytes());
    out.extend_from_slice(tarball);
    out
}

#[cfg(test)]
mod tests {
    use super::{encode, parse};
    use crate::error::CargoError;
    use serde_json::json;

    #[test]
    fn round_trip_encode_decode() {
        let manifest = json!({"name": "foo", "vers": "1.0.0"});
        let tarball = b"synthetic-tar-bytes";
        let body = encode(&manifest, tarball);
        let p = parse(&body).unwrap();
        assert_eq!(p.manifest, manifest);
        assert_eq!(p.tarball.as_ref(), tarball);
    }

    #[test]
    fn short_body_is_rejected() {
        assert!(parse(&[0, 0]).is_err());
    }

    #[test]
    fn truncated_metadata_is_rejected() {
        // metadata_len = 100 but no metadata bytes follow.
        let mut body = Vec::new();
        body.extend_from_slice(&100u32.to_le_bytes());
        body.extend_from_slice(b"{}");
        assert!(parse(&body).is_err());
    }

    #[test]
    fn truncated_tarball_is_rejected() {
        let manifest = b"{}";
        let mlen: u32 = u32::try_from(manifest.len()).unwrap();
        let mut body = Vec::new();
        body.extend_from_slice(&mlen.to_le_bytes());
        body.extend_from_slice(manifest);
        // crate_len = 10, but we only provide 3 bytes.
        body.extend_from_slice(&10u32.to_le_bytes());
        body.extend_from_slice(b"abc");
        assert!(parse(&body).is_err());
    }

    #[test]
    fn invalid_json_is_rejected() {
        let bad = b"xx";
        let mlen: u32 = u32::try_from(bad.len()).unwrap();
        let mut body = Vec::new();
        body.extend_from_slice(&mlen.to_le_bytes());
        body.extend_from_slice(bad);
        body.extend_from_slice(&0u32.to_le_bytes());
        assert!(parse(&body).is_err());
    }

    /// Boundary for the metadata-length check (`rest.len() < metadata_len`):
    /// a body whose remaining bytes EXACTLY equal `metadata_len` must NOT
    /// be treated as a truncated metadata body — the metadata slice is read
    /// in full and parsing then fails on the *missing `crate_len` prefix*.
    /// The `< → <=` mutant would instead reject at the metadata check,
    /// changing the error detail from "body too short for 4-byte length
    /// prefix" to "metadata body truncated".
    #[test]
    fn exact_fit_metadata_consumes_whole_rest_then_fails_at_crate_len() {
        let meta = b"{}";
        let mut body = Vec::new();
        body.extend_from_slice(&u32::try_from(meta.len()).unwrap().to_le_bytes());
        body.extend_from_slice(meta); // rest.len() == metadata_len exactly.
        let err = parse(&body).expect_err("missing crate_len prefix");
        let CargoError::InvalidPublish(detail) = err else {
            panic!("expected InvalidPublish, got {err:?}");
        };
        assert!(
            detail.contains("4-byte length prefix"),
            "metadata must be read in full, failing only at the absent \
             crate_len prefix; got: {detail}"
        );
    }

    /// A body whose metadata bytes are genuinely short of `metadata_len`
    /// IS rejected at the metadata check — the lower side of the boundary.
    #[test]
    fn under_fit_metadata_is_truncated() {
        let mut body = Vec::new();
        body.extend_from_slice(&10u32.to_le_bytes()); // metadata_len = 10
        body.extend_from_slice(b"{}"); // only 2 bytes follow
        let err = parse(&body).expect_err("truncated metadata");
        let CargoError::InvalidPublish(detail) = err else {
            panic!("expected InvalidPublish, got {err:?}");
        };
        assert!(detail.contains("metadata body truncated"), "got: {detail}");
    }

    /// Boundary for the 4-byte length-prefix check (`body.len() < 4`): a
    /// body of EXACTLY 4 bytes is a complete length prefix and must be
    /// read (here a `metadata_len` of 0). The `< → <=` mutant would reject a
    /// 4-byte prefix as "too short".
    #[test]
    fn exact_four_byte_prefix_is_read_not_rejected() {
        // metadata = "{}" (len 2), then the crate_len prefix is the LAST
        // 4 bytes of the body — `read_u32_le` is handed exactly a 4-byte
        // slice for crate_len = 0. The `< → <=` mutant (`body.len() < 4`
        // → `<= 4`) would reject this exact 4-byte prefix as "too short".
        let mut body = Vec::new();
        body.extend_from_slice(&2u32.to_le_bytes()); // metadata_len = 2
        body.extend_from_slice(b"{}");
        body.extend_from_slice(&0u32.to_le_bytes()); // crate_len = 0 (last 4 bytes)
        let p = parse(&body).expect("4-byte crate_len prefix must be read");
        assert!(p.tarball.is_empty());
        // A body shorter than 4 bytes is still rejected (lower side).
        assert!(parse(&[0, 0, 0]).is_err());
    }
}
