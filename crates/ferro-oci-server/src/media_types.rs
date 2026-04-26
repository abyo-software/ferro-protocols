// SPDX-License-Identifier: Apache-2.0
//! OCI / Docker media type constants.
//!
//! Spec: OCI Distribution Spec v1.1 §2 "Definitions" / OCI Image Spec
//! §5 "Media Types".
//!
//! The helpers in this module are used by the manifest handler to
//! choose the right `Content-Type` on response and to validate the
//! `Content-Type` on PUT.

/// Media type for an OCI image manifest (spec §5.2).
pub const OCI_IMAGE_MANIFEST: &str = "application/vnd.oci.image.manifest.v1+json";

/// Media type for an OCI image index (spec §5.3).
pub const OCI_IMAGE_INDEX: &str = "application/vnd.oci.image.index.v1+json";

/// Media type for an OCI image configuration object.
pub const OCI_IMAGE_CONFIG: &str = "application/vnd.oci.image.config.v1+json";

/// Media type for an OCI layer (uncompressed tar).
pub const OCI_IMAGE_LAYER_TAR: &str = "application/vnd.oci.image.layer.v1.tar";

/// Media type for an OCI layer (gzip tar).
pub const OCI_IMAGE_LAYER_TAR_GZIP: &str = "application/vnd.oci.image.layer.v1.tar+gzip";

/// Media type for an OCI layer (zstd tar).
pub const OCI_IMAGE_LAYER_TAR_ZSTD: &str = "application/vnd.oci.image.layer.v1.tar+zstd";

/// Media type for a generic OCI empty-payload descriptor.
pub const OCI_EMPTY: &str = "application/vnd.oci.empty.v1+json";

/// Media type for a Docker v2 Schema 2 manifest.
pub const DOCKER_MANIFEST_V2: &str = "application/vnd.docker.distribution.manifest.v2+json";

/// Media type for a Docker v2 Schema 2 manifest list.
pub const DOCKER_MANIFEST_LIST_V2: &str =
    "application/vnd.docker.distribution.manifest.list.v2+json";

/// Media type for a Docker v2 image config.
pub const DOCKER_IMAGE_CONFIG_V1: &str = "application/vnd.docker.container.image.v1+json";

/// Media type for a gzipped Docker image layer.
pub const DOCKER_LAYER_TAR_GZIP: &str = "application/vnd.docker.image.rootfs.diff.tar.gzip";

/// Classify a manifest media type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManifestKind {
    /// A single-platform OCI or Docker manifest.
    ImageManifest,
    /// A multi-platform OCI image index or Docker manifest list.
    ImageIndex,
    /// An OCI artifact manifest whose `artifactType` is set.
    Artifact,
}

/// Classify a `Content-Type` string reaching the manifest PUT handler.
///
/// Returns `None` for media types that are not valid manifests (the
/// handler then emits `MANIFEST_INVALID`).
#[must_use]
pub fn classify_manifest_media_type(ct: &str) -> Option<ManifestKind> {
    // Strip optional parameters (`; charset=utf-8`, etc.).
    let bare = ct.split(';').next().unwrap_or(ct).trim();
    match bare {
        OCI_IMAGE_MANIFEST | DOCKER_MANIFEST_V2 => Some(ManifestKind::ImageManifest),
        OCI_IMAGE_INDEX | DOCKER_MANIFEST_LIST_V2 => Some(ManifestKind::ImageIndex),
        other if other.starts_with("application/vnd.") && other.ends_with("+json") => {
            Some(ManifestKind::Artifact)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{ManifestKind, classify_manifest_media_type};

    #[test]
    fn oci_image_manifest_is_classified_as_image() {
        assert_eq!(
            classify_manifest_media_type("application/vnd.oci.image.manifest.v1+json"),
            Some(ManifestKind::ImageManifest),
        );
    }

    #[test]
    fn oci_image_index_is_classified_as_index() {
        assert_eq!(
            classify_manifest_media_type("application/vnd.oci.image.index.v1+json"),
            Some(ManifestKind::ImageIndex),
        );
    }

    #[test]
    fn arbitrary_vnd_json_is_artifact() {
        assert_eq!(
            classify_manifest_media_type("application/vnd.cncf.helm.config.v1+json"),
            Some(ManifestKind::Artifact),
        );
    }

    #[test]
    fn plain_json_is_rejected() {
        assert_eq!(classify_manifest_media_type("application/json"), None);
    }

    #[test]
    fn charset_parameter_is_stripped() {
        assert_eq!(
            classify_manifest_media_type(
                "application/vnd.oci.image.manifest.v1+json; charset=utf-8"
            ),
            Some(ManifestKind::ImageManifest),
        );
    }
}
