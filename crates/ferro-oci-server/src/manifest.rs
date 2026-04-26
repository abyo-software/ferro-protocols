// SPDX-License-Identifier: Apache-2.0
//! OCI image manifest and image index types.
//!
//! Spec: OCI Image Spec v1.1 §5 "Media Types" and §6 "Image Manifest" /
//! §7 "Image Index".
//!
//! The structs here mirror the on-the-wire shape so they can be
//! round-tripped through `serde_json` byte-for-byte (serialize,
//! re-parse, re-serialize yields the same JSON), which the
//! conformance suite relies on.

use ferro_blob_store::Digest;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A content descriptor (OCI Image Spec §3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Descriptor {
    /// Media type of the referenced content.
    #[serde(rename = "mediaType")]
    pub media_type: String,
    /// Digest of the referenced content.
    pub digest: Digest,
    /// Size in bytes of the referenced content.
    pub size: u64,
    /// Optional URLs that can be used to retrieve the content.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub urls: Vec<String>,
    /// Optional annotations.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub annotations: Option<std::collections::BTreeMap<String, String>>,
    /// Optional artifact type.
    #[serde(
        rename = "artifactType",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub artifact_type: Option<String>,
    /// Optional platform (used in image indexes).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform: Option<Value>,
    /// Any additional fields preserved verbatim so a round-trip
    /// leaves unknown keys intact.
    #[serde(flatten, default)]
    pub extra: std::collections::BTreeMap<String, Value>,
}

/// OCI image manifest (spec §6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageManifest {
    /// Schema version; must be `2` for OCI v1.x manifests.
    #[serde(rename = "schemaVersion")]
    pub schema_version: u32,
    /// Media type of this manifest.
    #[serde(rename = "mediaType", skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,
    /// Optional artifact type (OCI 1.1 artifact manifests).
    #[serde(
        rename = "artifactType",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub artifact_type: Option<String>,
    /// Image config descriptor.
    pub config: Descriptor,
    /// Image layers (ordered base -> top).
    pub layers: Vec<Descriptor>,
    /// Optional subject descriptor (referrers API).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<Descriptor>,
    /// Optional annotations.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub annotations: Option<std::collections::BTreeMap<String, String>>,
    /// Forward-compatible extra keys.
    #[serde(flatten, default)]
    pub extra: std::collections::BTreeMap<String, Value>,
}

/// OCI image index / Docker manifest list (spec §7).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageIndex {
    /// Schema version; must be `2`.
    #[serde(rename = "schemaVersion")]
    pub schema_version: u32,
    /// Media type of this index.
    #[serde(rename = "mediaType", skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,
    /// Optional artifact type.
    #[serde(
        rename = "artifactType",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub artifact_type: Option<String>,
    /// Per-platform manifest descriptors.
    pub manifests: Vec<Descriptor>,
    /// Optional subject descriptor (referrers API).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<Descriptor>,
    /// Optional annotations.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub annotations: Option<std::collections::BTreeMap<String, String>>,
    /// Forward-compatible extra keys.
    #[serde(flatten, default)]
    pub extra: std::collections::BTreeMap<String, Value>,
}

/// Build an empty image index — the shape returned by the referrers API
/// when no referrers exist (spec §3.3).
#[must_use]
pub fn empty_image_index() -> ImageIndex {
    ImageIndex {
        schema_version: 2,
        media_type: Some(crate::media_types::OCI_IMAGE_INDEX.to_owned()),
        artifact_type: None,
        manifests: Vec::new(),
        subject: None,
        annotations: None,
        extra: std::collections::BTreeMap::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::{Descriptor, ImageIndex, ImageManifest, empty_image_index};
    use ferro_blob_store::Digest;

    fn sample_digest() -> Digest {
        Digest::sha256_of(b"sample")
    }

    #[test]
    fn image_manifest_round_trips_via_json() {
        let m = ImageManifest {
            schema_version: 2,
            media_type: Some("application/vnd.oci.image.manifest.v1+json".to_owned()),
            artifact_type: None,
            config: Descriptor {
                media_type: "application/vnd.oci.image.config.v1+json".to_owned(),
                digest: sample_digest(),
                size: 512,
                urls: Vec::new(),
                annotations: None,
                artifact_type: None,
                platform: None,
                extra: std::collections::BTreeMap::new(),
            },
            layers: vec![Descriptor {
                media_type: "application/vnd.oci.image.layer.v1.tar+gzip".to_owned(),
                digest: sample_digest(),
                size: 1024,
                urls: Vec::new(),
                annotations: None,
                artifact_type: None,
                platform: None,
                extra: std::collections::BTreeMap::new(),
            }],
            subject: None,
            annotations: None,
            extra: std::collections::BTreeMap::new(),
        };
        let json = serde_json::to_string(&m).expect("serialise");
        let back: ImageManifest = serde_json::from_str(&json).expect("deserialise");
        assert_eq!(back, m);
    }

    #[test]
    fn empty_image_index_has_no_manifests() {
        let idx = empty_image_index();
        assert_eq!(idx.schema_version, 2);
        assert!(idx.manifests.is_empty());
    }

    #[test]
    fn image_index_with_two_platforms_round_trips() {
        let idx = ImageIndex {
            schema_version: 2,
            media_type: Some("application/vnd.oci.image.index.v1+json".to_owned()),
            artifact_type: None,
            manifests: vec![
                Descriptor {
                    media_type: "application/vnd.oci.image.manifest.v1+json".to_owned(),
                    digest: sample_digest(),
                    size: 256,
                    urls: Vec::new(),
                    annotations: None,
                    artifact_type: None,
                    platform: Some(serde_json::json!({ "architecture": "amd64", "os": "linux" })),
                    extra: std::collections::BTreeMap::new(),
                },
                Descriptor {
                    media_type: "application/vnd.oci.image.manifest.v1+json".to_owned(),
                    digest: sample_digest(),
                    size: 256,
                    urls: Vec::new(),
                    annotations: None,
                    artifact_type: None,
                    platform: Some(serde_json::json!({ "architecture": "arm64", "os": "linux" })),
                    extra: std::collections::BTreeMap::new(),
                },
            ],
            subject: None,
            annotations: None,
            extra: std::collections::BTreeMap::new(),
        };
        let json = serde_json::to_string(&idx).expect("serialise");
        let back: ImageIndex = serde_json::from_str(&json).expect("deserialise");
        assert_eq!(back, idx);
    }
}
