// SPDX-License-Identifier: Apache-2.0
//! Conformance tests against vendored upstream OCI Image Spec fixtures.
//!
//! These exercise the manifest / image-index parser surface against the
//! canonical examples published in the OCI Image Spec v1.1, the same
//! shapes the `opencontainers/distribution-spec/conformance` Go suite
//! seeds before each run.
//!
//! Source URLs and license attribution: see `tests/fixtures/README.md`.

use ferro_oci_server::{
    Descriptor, ImageIndex, ImageManifest, ManifestKind, classify_manifest_media_type,
};

const OCI_IMAGE_MANIFEST: &str = include_str!("fixtures/oci-image-manifest.json");
const OCI_IMAGE_INDEX: &str = include_str!("fixtures/oci-image-index.json");

#[test]
fn upstream_image_manifest_parses_into_typed_struct() {
    let parsed: ImageManifest =
        serde_json::from_str(OCI_IMAGE_MANIFEST).expect("upstream manifest deserialises");

    assert_eq!(parsed.schema_version, 2);
    assert_eq!(
        parsed.media_type.as_deref(),
        Some("application/vnd.oci.image.manifest.v1+json"),
    );
    assert_eq!(
        parsed.config.media_type,
        "application/vnd.oci.image.config.v1+json",
    );
    assert_eq!(parsed.config.size, 7023);
    assert_eq!(parsed.layers.len(), 3);
    assert!(
        parsed
            .layers
            .iter()
            .all(|l| l.media_type == "application/vnd.oci.image.layer.v1.tar+gzip"),
        "all canonical layers are gzip tar layers",
    );
    let subject = parsed.subject.as_ref().expect("subject present");
    assert_eq!(subject.size, 7682);
    let ann = parsed.annotations.as_ref().expect("annotations present");
    assert_eq!(
        ann.get("com.example.key1").map(String::as_str),
        Some("value1")
    );
}

#[test]
fn upstream_image_manifest_round_trips_byte_compatibly() {
    // Canonical-form round-trip: parse → re-serialise → re-parse and
    // assert equality. The conformance suite relies on this to validate
    // that intermediaries (mirrors, proxies) don't perturb the manifest
    // body before recomputing its digest.
    let parsed: ImageManifest = serde_json::from_str(OCI_IMAGE_MANIFEST).expect("parse 1");
    let reserialised = serde_json::to_string(&parsed).expect("serialise");
    let reparsed: ImageManifest = serde_json::from_str(&reserialised).expect("parse 2");
    assert_eq!(parsed, reparsed);
}

#[test]
fn upstream_image_index_parses_with_two_platforms() {
    let parsed: ImageIndex =
        serde_json::from_str(OCI_IMAGE_INDEX).expect("upstream index deserialises");

    assert_eq!(parsed.schema_version, 2);
    assert_eq!(
        parsed.media_type.as_deref(),
        Some("application/vnd.oci.image.index.v1+json"),
    );
    assert_eq!(parsed.manifests.len(), 2);

    // Per-arch presence — the index canonical example covers ppc64le
    // and amd64.
    let archs: Vec<String> = parsed
        .manifests
        .iter()
        .filter_map(|m: &Descriptor| {
            m.platform
                .as_ref()
                .and_then(|p| p.get("architecture"))
                .and_then(|a| a.as_str())
                .map(str::to_owned)
        })
        .collect();
    assert!(archs.contains(&"amd64".to_owned()));
    assert!(archs.contains(&"ppc64le".to_owned()));
}

#[test]
fn upstream_image_index_round_trips_byte_compatibly() {
    let parsed: ImageIndex = serde_json::from_str(OCI_IMAGE_INDEX).expect("parse 1");
    let reserialised = serde_json::to_string(&parsed).expect("serialise");
    let reparsed: ImageIndex = serde_json::from_str(&reserialised).expect("parse 2");
    assert_eq!(parsed, reparsed);
}

#[test]
fn upstream_manifest_media_type_is_classified_as_image_manifest() {
    let parsed: ImageManifest = serde_json::from_str(OCI_IMAGE_MANIFEST).expect("parse");
    let mt = parsed.media_type.expect("media type set");
    assert_eq!(
        classify_manifest_media_type(&mt),
        Some(ManifestKind::ImageManifest),
    );
}

#[test]
fn upstream_index_media_type_is_classified_as_image_index() {
    let parsed: ImageIndex = serde_json::from_str(OCI_IMAGE_INDEX).expect("parse");
    let mt = parsed.media_type.expect("media type set");
    assert_eq!(
        classify_manifest_media_type(&mt),
        Some(ManifestKind::ImageIndex),
    );
}
