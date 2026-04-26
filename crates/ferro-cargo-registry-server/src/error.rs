// SPDX-License-Identifier: Apache-2.0
//! Cargo registry protocol errors.
//!
//! Cargo clients consume JSON on error; the registry API §"Errors"
//! defines the envelope as `{ "errors": [{ "detail": "..." }] }`.
//! Reference: <https://doc.rust-lang.org/cargo/reference/registry-web-api.html#errors>.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use ferro_blob_store::BlobStoreError;
use serde_json::json;

/// Errors raised by the Cargo registry protocol.
#[derive(Debug, thiserror::Error)]
pub enum CargoError {
    /// A crate name failed validation (registry §"Crate name
    /// restrictions").
    #[error("invalid crate name: {0}")]
    InvalidName(String),

    /// A version string is not legal semver.
    #[error("invalid semver: {0}")]
    InvalidVersion(String),

    /// The publish request body is malformed (LE-length / JSON /
    /// tarball mismatch).
    #[error("invalid publish payload: {0}")]
    InvalidPublish(String),

    /// The declared `cksum` did not match the tarball SHA-256.
    #[error("checksum mismatch: declared {declared}, computed {computed}")]
    ChecksumMismatch {
        /// Client-declared SHA-256 hex.
        declared: String,
        /// Server-computed SHA-256 hex.
        computed: String,
    },

    /// The requested resource does not exist.
    #[error("not found: {0}")]
    NotFound(String),

    /// Feature not yet implemented in this phase (Git index).
    #[error("not implemented: {0}")]
    NotImplemented(String),

    /// Underlying blob-store error (I/O, digest mismatch, missing blob).
    #[error(transparent)]
    Storage(#[from] BlobStoreError),
}

impl CargoError {
    /// HTTP status code for this error.
    #[must_use]
    pub fn status(&self) -> StatusCode {
        match self {
            Self::InvalidName(_)
            | Self::InvalidVersion(_)
            | Self::InvalidPublish(_)
            | Self::ChecksumMismatch { .. } => StatusCode::BAD_REQUEST,
            Self::NotFound(_) => StatusCode::NOT_FOUND,
            Self::NotImplemented(_) => StatusCode::NOT_IMPLEMENTED,
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
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

impl IntoResponse for CargoError {
    fn into_response(self) -> Response {
        let status = self.status();
        let body = json!({
            "errors": [{ "detail": self.to_string() }]
        });
        (status, axum::Json(body)).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::CargoError;
    use axum::http::StatusCode;

    #[test]
    fn invalid_name_is_400() {
        assert_eq!(
            CargoError::InvalidName(String::new()).status(),
            StatusCode::BAD_REQUEST
        );
    }

    #[test]
    fn not_found_is_404() {
        assert_eq!(
            CargoError::NotFound("x".into()).status(),
            StatusCode::NOT_FOUND
        );
    }

    #[test]
    fn not_implemented_is_501() {
        assert_eq!(
            CargoError::NotImplemented("git".into()).status(),
            StatusCode::NOT_IMPLEMENTED
        );
    }

    #[test]
    fn checksum_mismatch_is_400() {
        let e = CargoError::ChecksumMismatch {
            declared: "a".into(),
            computed: "b".into(),
        };
        assert_eq!(e.status(), StatusCode::BAD_REQUEST);
    }
}
