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
    /// Manifest JSON (name / vers / deps / features / rust_version / ...).
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
}
