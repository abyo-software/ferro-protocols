// SPDX-License-Identifier: Apache-2.0
//! `ferro-oci-server`
//!
//! OCI Distribution Spec v1.1 (`opencontainers/distribution-spec`) and
//! Docker Registry HTTP API v2 (`docker/distribution`) for FerroRepo.
//!
//! Phase 1 scope (wired in this crate):
//!
//! - `GET /v2/` version check and auth challenge (spec §3.2);
//! - `GET /v2/_catalog` repository catalog with `n`/`last` pagination
//!   (spec §3.5);
//! - `GET /v2/{name}/tags/list` tag listing with pagination (spec §3.6);
//! - `GET|HEAD|DELETE /v2/{name}/blobs/{digest}` (spec §3.2 / §4.9);
//! - `POST|PATCH|PUT /v2/{name}/blobs/uploads/{uuid?}` — monolithic
//!   and chunked uploads (spec §4.3–§4.8);
//! - `GET|HEAD|PUT|DELETE /v2/{name}/manifests/{reference}` (spec
//!   §3.2 / §4.4 / §4.9);
//! - `GET /v2/{name}/referrers/{digest}` referrers API (spec §3.3).
//!
//! The Phase 1 exit gate is 100 % pass on the
//! `opencontainers/distribution-spec` conformance suite and interop
//! with `docker`, `podman`, `crane`, `skopeo`, and `nerdctl`.

pub mod error;
pub mod handlers;
pub mod manifest;
pub mod media_types;
pub mod reference;
pub mod registry;
pub mod router;
pub mod upload;

pub use error::{OciError, OciErrorBody, OciErrorCode, OciErrorInfo, OciResult};
pub use manifest::{Descriptor, ImageIndex, ImageManifest, empty_image_index};
pub use media_types::{ManifestKind, classify_manifest_media_type};
pub use reference::{MAX_NAME_LENGTH, MAX_TAG_LENGTH, Reference, validate_name};
pub use registry::{InMemoryRegistryMeta, ReferrerDescriptor, RegistryMeta};
pub use router::{AppState, router};
pub use upload::{ContentRange, UploadState};

/// Crate name, exposed for diagnostics and `/metrics` labelling.
pub const CRATE_NAME: &str = "ferro-oci-server";

#[cfg(test)]
mod tests {
    use super::CRATE_NAME;

    #[test]
    fn crate_name_is_stable() {
        assert_eq!(CRATE_NAME, "ferro-oci-server");
    }
}
