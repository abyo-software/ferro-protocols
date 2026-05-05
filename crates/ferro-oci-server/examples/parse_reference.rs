// SPDX-License-Identifier: Apache-2.0
//! Demonstrates the codec-only types in `ferro-oci-server`: repository
//! name validation, reference (tag / digest) parsing, manifest media-type
//! classification, and a minimal `ImageManifest` round-trip.
//!
//! No HTTP server, no Tokio, no `axum::Router` — just the spec-mapped
//! types you can use anywhere (a CLI tool, a vanilla `hyper` server,
//! a `tower` service, etc.).
//!
//! Run with:
//!
//! ```bash
//! cargo run --example parse_reference -p ferro-oci-server
//! ```

use ferro_oci_server::{
    Descriptor, ImageManifest, Reference, classify_manifest_media_type, validate_name,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // ----- Repository name validation --------------------------------------

    for name in [
        "alpine",
        "library/alpine",
        "my-org/my_team__service-v2",
        "INVALID", // starts with uppercase
        "ends-with-dash-",
        "double//slash",
    ] {
        match validate_name(name) {
            Ok(()) => println!("name OK:    {name:?}"),
            Err(e) => println!("name BAD:   {name:?} -> {e}"),
        }
    }

    // ----- Reference parsing -----------------------------------------------

    let tag: Reference = "v1.2.3".parse()?;
    let dig: Reference = format!("sha256:{}", "a".repeat(64)).parse()?;
    println!("tag: {tag} (is_tag={})", tag.is_tag());
    println!("dig: {dig} (is_digest={})", dig.is_digest());

    // ----- Media-type classification ---------------------------------------

    let kinds = [
        "application/vnd.oci.image.manifest.v1+json",
        "application/vnd.oci.image.index.v1+json",
        "application/vnd.docker.distribution.manifest.v2+json",
        "application/vnd.docker.distribution.manifest.list.v2+json",
        "text/plain",
    ];
    for kind in kinds {
        match classify_manifest_media_type(kind) {
            Some(c) => println!("media-type {kind} -> {c:?}"),
            None => println!("media-type {kind} -> rejected (415 Unsupported Media Type)"),
        }
    }

    // ----- Manifest round-trip ---------------------------------------------

    let blob_digest: ferro_blob_store::Digest = format!("sha256:{}", "b".repeat(64)).parse()?;
    let cfg_digest: ferro_blob_store::Digest = format!("sha256:{}", "c".repeat(64)).parse()?;

    let manifest = ImageManifest {
        schema_version: 2,
        media_type: Some("application/vnd.oci.image.manifest.v1+json".into()),
        config: Descriptor {
            media_type: "application/vnd.oci.image.config.v1+json".into(),
            digest: cfg_digest,
            size: 512,
            urls: vec![],
            annotations: None,
            artifact_type: None,
            platform: None,
            extra: Default::default(),
        },
        layers: vec![Descriptor {
            media_type: "application/vnd.oci.image.layer.v1.tar+gzip".into(),
            digest: blob_digest,
            size: 1024,
            urls: vec![],
            annotations: None,
            artifact_type: None,
            platform: None,
            extra: Default::default(),
        }],
        subject: None,
        artifact_type: None,
        annotations: None,
        extra: Default::default(),
    };

    let json = serde_json::to_string_pretty(&manifest)?;
    println!("manifest JSON ({} bytes):\n{json}", json.len());

    let parsed: ImageManifest = serde_json::from_str(&json)?;
    assert_eq!(parsed, manifest);
    println!("manifest round-trip OK");

    Ok(())
}
