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

    /// The publish name collides with an already-published crate under
    /// cargo's uniqueness rules (case-insensitive, `-`/`_`-insensitive).
    #[error("crate name conflict: `{requested}` collides with existing `{existing}`")]
    NameConflict {
        /// The name the client attempted to publish.
        requested: String,
        /// The existing crate it collides with.
        existing: String,
    },

    /// A publish targets a `(name, version)` that already exists. Cargo
    /// registry versions are immutable, so a re-publish is rejected; only
    /// yank / unyank may mutate an existing index line.
    #[error("crate version already exists: {name} {version}")]
    DuplicateVersion {
        /// The crate name being re-published.
        name: String,
        /// The version being re-published.
        version: String,
    },

    /// Feature not yet implemented in this phase (Git index).
    #[error("not implemented: {0}")]
    NotImplemented(String),

    /// A mutation succeeded in memory but could not be made durable, so it
    /// was rolled back rather than acknowledged. The client must retry;
    /// the change was **not** stored (DD R3-2).
    #[error("could not persist change durably: {0}")]
    Persistence(String),

    /// Underlying blob-store error (I/O, digest mismatch, missing blob).
    #[error(transparent)]
    Storage(#[from] BlobStoreError),
}

impl CargoError {
    /// HTTP status code for this error.
    #[must_use]
    pub const fn status(&self) -> StatusCode {
        match self {
            Self::InvalidName(_)
            | Self::InvalidVersion(_)
            | Self::InvalidPublish(_)
            | Self::ChecksumMismatch { .. } => StatusCode::BAD_REQUEST,
            Self::NameConflict { .. } | Self::DuplicateVersion { .. } => StatusCode::CONFLICT,
            Self::NotFound(_) => StatusCode::NOT_FOUND,
            Self::NotImplemented(_) => StatusCode::NOT_IMPLEMENTED,
            Self::Persistence(_) => StatusCode::INTERNAL_SERVER_ERROR,
            Self::Storage(err) => storage_status(err),
        }
    }
}

const fn storage_status(err: &BlobStoreError) -> StatusCode {
    match err {
        BlobStoreError::NotFound(_) => StatusCode::NOT_FOUND,
        BlobStoreError::DigestMismatch { .. } | BlobStoreError::InvalidDigest(_) => {
            StatusCode::BAD_REQUEST
        }
        // I/O and any other variant map to 500; kept explicit so a new
        // blob-store variant forces a conscious classification here.
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
    fn name_conflict_is_409() {
        let e = CargoError::NameConflict {
            requested: "foo_bar".into(),
            existing: "foo-bar".into(),
        };
        assert_eq!(e.status(), StatusCode::CONFLICT);
    }

    #[test]
    fn duplicate_version_is_409() {
        let e = CargoError::DuplicateVersion {
            name: "foo".into(),
            version: "1.0.0".into(),
        };
        assert_eq!(e.status(), StatusCode::CONFLICT);
    }

    #[test]
    fn checksum_mismatch_is_400() {
        let e = CargoError::ChecksumMismatch {
            declared: "a".into(),
            computed: "b".into(),
        };
        assert_eq!(e.status(), StatusCode::BAD_REQUEST);
    }

    /// A persistence failure (mutation rolled back) is surfaced as `500`,
    /// not a success — the client must know the change was not stored.
    #[test]
    fn persistence_is_500() {
        assert_eq!(
            CargoError::Persistence("disk full".into()).status(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }

    /// `storage_status` maps a blob-store `NotFound` to `404`. Deleting
    /// that match arm would fold it into the catch-all `500`, so the
    /// explicit assertion pins the mapping.
    #[test]
    fn storage_not_found_is_404() {
        use ferro_blob_store::BlobStoreError;
        let e = CargoError::Storage(BlobStoreError::NotFound("digest".into()));
        assert_eq!(e.status(), StatusCode::NOT_FOUND);
    }

    /// `storage_status` maps a blob-store digest mismatch to `400`.
    /// Deleting that match arm would fold it into the catch-all `500`.
    #[test]
    fn storage_digest_mismatch_is_400() {
        use ferro_blob_store::BlobStoreError;
        let e = CargoError::Storage(BlobStoreError::DigestMismatch {
            expected: "aa".into(),
            computed: "bb".into(),
        });
        assert_eq!(e.status(), StatusCode::BAD_REQUEST);
    }

    /// `storage_status` maps an invalid-digest parse error to `400` (the
    /// same arm as `DigestMismatch`). Deleting the arm regresses it to
    /// `500`.
    #[test]
    fn storage_invalid_digest_is_400() {
        use ferro_blob_store::{Digest, DigestAlgo};
        // An odd-length / non-hex string is not a valid SHA-256 digest.
        let parse_err = Digest::new(DigestAlgo::Sha256, "zz").unwrap_err();
        let e = CargoError::Storage(parse_err.into());
        assert_eq!(e.status(), StatusCode::BAD_REQUEST);
    }

    /// An I/O blob-store error falls through to the explicit `500` arm
    /// (the "new variant forces a conscious classification" guard). This
    /// distinguishes the catch-all from the 404 / 400 arms.
    #[test]
    fn storage_io_error_is_500() {
        use ferro_blob_store::BlobStoreError;
        let io = std::io::Error::other("disk on fire");
        let e = CargoError::Storage(BlobStoreError::Io(io));
        assert_eq!(e.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }
}
