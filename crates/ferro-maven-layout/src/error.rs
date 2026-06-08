// SPDX-License-Identifier: Apache-2.0
//! Maven-specific error type.
//!
//! Maps onto HTTP status codes at the REST boundary. Wraps
//! [`ferro_blob_store::BlobStoreError`] transparently so storage and
//! digest failures surface without a second translation step.
//!
//! The HTTP / `IntoResponse` integration is gated on the `http`
//! feature; without it, the [`MavenError`] enum is still usable as a
//! pure value type.

#[cfg(feature = "http")]
use axum::http::StatusCode;
#[cfg(feature = "http")]
use axum::response::{IntoResponse, Response};
use ferro_blob_store::BlobStoreError;

/// Errors raised by the Maven protocol crate.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum MavenError {
    /// The request path did not match the Maven 2/3 layout.
    #[error("invalid Maven layout path: {0}")]
    InvalidPath(String),

    /// The request path's GAV did not match the POM contents on PUT.
    #[error("POM coordinate mismatch: {0}")]
    CoordinateMismatch(String),

    /// The POM body failed to parse as XML.
    #[error("invalid POM: {0}")]
    InvalidPom(String),

    /// A maven-metadata.xml body failed to parse.
    #[error("invalid maven-metadata.xml: {0}")]
    InvalidMetadata(String),

    /// The requested artifact or metadata document does not exist.
    #[error("not found: {0}")]
    NotFound(String),

    /// A checksum sidecar did not agree with the hash of the underlying
    /// artifact.
    #[error("checksum mismatch: {0}")]
    ChecksumMismatch(String),

    /// An underlying blob-store error (I/O, digest mismatch, missing blob).
    #[error(transparent)]
    Storage(#[from] BlobStoreError),
}

#[cfg(feature = "http")]
impl MavenError {
    /// HTTP status code for this error category.
    #[must_use]
    pub const fn status(&self) -> StatusCode {
        match self {
            Self::InvalidPath(_)
            | Self::InvalidPom(_)
            | Self::InvalidMetadata(_)
            | Self::CoordinateMismatch(_)
            | Self::ChecksumMismatch(_) => StatusCode::BAD_REQUEST,
            Self::NotFound(_) => StatusCode::NOT_FOUND,
            Self::Storage(err) => storage_status(err),
        }
    }
}

#[cfg(feature = "http")]
const fn storage_status(err: &BlobStoreError) -> StatusCode {
    match err {
        BlobStoreError::NotFound(_) => StatusCode::NOT_FOUND,
        BlobStoreError::DigestMismatch { .. } | BlobStoreError::InvalidDigest(_) => {
            StatusCode::BAD_REQUEST
        }
        // `BlobStoreError::Io(_)`, plus future `#[non_exhaustive]` variants.
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

#[cfg(feature = "http")]
impl IntoResponse for MavenError {
    fn into_response(self) -> Response {
        let status = self.status();
        let body = self.to_string();
        (status, body).into_response()
    }
}

#[cfg(all(test, feature = "http"))]
mod tests {
    use super::MavenError;
    use axum::http::StatusCode;
    use ferro_blob_store::{BlobStoreError, Digest};

    #[test]
    fn invalid_path_maps_to_400() {
        let err = MavenError::InvalidPath("no artifactId segment".into());
        assert_eq!(err.status(), StatusCode::BAD_REQUEST);
    }

    #[test]
    fn not_found_maps_to_404() {
        let err = MavenError::NotFound("foo.jar".into());
        assert_eq!(err.status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn coordinate_and_pom_and_metadata_and_checksum_map_to_400() {
        // Cover the remaining `BAD_REQUEST` arms of `MavenError::status`.
        for err in [
            MavenError::CoordinateMismatch("gav".into()),
            MavenError::InvalidPom("bad pom".into()),
            MavenError::InvalidMetadata("bad metadata".into()),
            MavenError::ChecksumMismatch("nope".into()),
        ] {
            assert_eq!(err.status(), StatusCode::BAD_REQUEST);
        }
    }

    #[test]
    fn storage_not_found_maps_to_404() {
        // Exercises the `BlobStoreError::NotFound` arm of `storage_status`.
        let err = MavenError::Storage(BlobStoreError::NotFound("sha256:deadbeef".into()));
        assert_eq!(err.status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn storage_digest_mismatch_maps_to_400() {
        // Exercises the `DigestMismatch` arm of `storage_status`.
        let err = MavenError::Storage(BlobStoreError::DigestMismatch {
            expected: "sha256:00".into(),
            computed: "sha256:11".into(),
        });
        assert_eq!(err.status(), StatusCode::BAD_REQUEST);
    }

    #[test]
    fn storage_invalid_digest_maps_to_400() {
        // Exercises the `InvalidDigest` arm of `storage_status`. A wire
        // string without an `<algo>:<hex>` separator fails to parse.
        let parse_err = "not-a-digest"
            .parse::<Digest>()
            .expect_err("must reject malformed digest");
        let err = MavenError::Storage(BlobStoreError::InvalidDigest(parse_err));
        assert_eq!(err.status(), StatusCode::BAD_REQUEST);
    }

    #[test]
    fn storage_io_maps_to_500() {
        // The catch-all `_` arm of `storage_status` (e.g. an I/O error)
        // must surface as `INTERNAL_SERVER_ERROR`, distinguishing it from
        // the explicit 404 / 400 arms.
        let io = std::io::Error::other("disk gone");
        let err = MavenError::Storage(BlobStoreError::Io(io));
        assert_eq!(err.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }
}
