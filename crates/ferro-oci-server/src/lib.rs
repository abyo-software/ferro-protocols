// SPDX-License-Identifier: Apache-2.0
//! `ferro-oci-server`
//!
//! OCI Distribution Spec v1.1 (`opencontainers/distribution-spec`) and
//! Docker Registry HTTP API v2 (`docker/distribution`) for `FerroRepo`.
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
//!
//! # Quick start
//!
//! Build an [`AppState`] from a blob store and a metadata plane, hand
//! it to [`router()`], optionally merge in the Kubernetes health probes
//! from [`probe_routes`], and serve it with `axum`:
//!
//! ```no_run
//! use std::sync::Arc;
//!
//! use ferro_blob_store::InMemoryBlobStore;
//! use ferro_oci_server::{AppState, InMemoryRegistryMeta, probe_routes, router};
//!
//! # async fn run() -> Result<(), Box<dyn std::error::Error>> {
//! let state = AppState::new(
//!     Arc::new(InMemoryBlobStore::new()),
//!     Arc::new(InMemoryRegistryMeta::new()),
//! );
//! let app = router(state).merge(probe_routes());
//! let listener = tokio::net::TcpListener::bind("0.0.0.0:8080").await?;
//! axum::serve(listener, app).await?;
//! # Ok(())
//! # }
//! ```
//!
//! After that, `docker push localhost:8080/myimage:latest` (or
//! `podman` / `crane` / `skopeo`) works against the running server.
//!
//! # Integration story
//!
//! - **Storage** — blob bytes live behind
//!   [`ferro_blob_store::BlobStore`] (use the bundled
//!   [`ferro_blob_store::FsBlobStore`] for a filesystem registry or
//!   [`ferro_blob_store::InMemoryBlobStore`] for tests). Metadata
//!   (manifests, tags, upload sessions, referrers) lives behind the
//!   [`RegistryMeta`] trait; [`InMemoryRegistryMeta`] ships as the
//!   single-node reference impl, and you can supply a
//!   SQLite/Postgres-backed impl of your own.
//! - **Auth** — handlers are open by design. Layer authentication and
//!   authorization as `tower` middleware *above* the [`router()`].
//! - **Deployment** — a runnable `ferro-oci-server` binary ships with
//!   this crate (see `src/bin/ferro-oci-server.rs`); it reads
//!   `FERRO_OCI_LISTEN` and `FERRO_OCI_STORAGE_DIR` from the
//!   environment, exposes the `/live`, `/healthz`, and `/ready` probes,
//!   and shuts down gracefully on `SIGTERM`/`SIGINT`.

pub mod error;
pub mod handlers;
pub mod manifest;
pub mod media_types;
pub mod metrics;
pub mod reference;
pub mod registry;
pub mod router;
pub mod serve;
pub mod upload;

pub use error::{OciError, OciErrorBody, OciErrorCode, OciErrorInfo, OciResult};
pub use manifest::{Descriptor, ImageIndex, ImageManifest, empty_image_index};
pub use media_types::{ManifestKind, classify_manifest_media_type};
pub use metrics::{Metrics, MetricsState, instrument, metrics_routes};
pub use reference::{MAX_NAME_LENGTH, MAX_TAG_LENGTH, Reference, validate_name};
pub use registry::{
    DEFAULT_MAX_UPLOAD_SESSIONS, DEFAULT_UPLOAD_SESSION_TTL, InMemoryRegistryMeta,
    METADATA_FILE_NAME, ReferrerDescriptor, RegistryMeta, SessionLimits, UploadAdmission,
};
pub use router::{AppState, probe_routes, router};
pub use serve::{Config, build_app, build_app_persisted, init_tracing, serve};
pub use router::MAX_BODY_BYTES;
pub use upload::{ContentRange, MAX_UPLOAD_SESSION_BYTES, UploadState};

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
