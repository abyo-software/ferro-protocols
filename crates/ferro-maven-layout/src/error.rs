// SPDX-License-Identifier: Apache-2.0
//! Maven-specific error type.
//!
//! Maps onto HTTP status codes at the REST boundary. Wraps
//! [`ferro_blob_store::BlobStoreError`] transparently so storage and
//! digest failures surface without a second translation step.

use axum::http::StatusCode;
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

impl MavenError {
    /// HTTP status code for this error category.
    #[must_use]
    pub fn status(&self) -> StatusCode {
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

fn storage_status(err: &BlobStoreError) -> StatusCode {
    match err {
        BlobStoreError::NotFound(_) => StatusCode::NOT_FOUND,
        BlobStoreError::DigestMismatch { .. } | BlobStoreError::InvalidDigest(_) => {
            StatusCode::BAD_REQUEST
        }
        BlobStoreError::Io(_) => StatusCode::INTERNAL_SERVER_ERROR,
        // BlobStoreError is `#[non_exhaustive]`; future variants land here.
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

impl IntoResponse for MavenError {
    fn into_response(self) -> Response {
        let status = self.status();
        let body = self.to_string();
        (status, body).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::MavenError;
    use axum::http::StatusCode;

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
}
