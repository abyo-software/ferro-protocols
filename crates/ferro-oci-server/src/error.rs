// SPDX-License-Identifier: Apache-2.0
//! OCI error response shape and mapping to HTTP status codes.
//!
//! Spec: OCI Distribution Spec v1.1 §3.1 "Error codes".
//!
//! Every 4xx/5xx response returned by the handlers in this crate must be
//! `application/json` with a body of the form:
//!
//! ```json
//! { "errors": [ { "code": "...", "message": "...", "detail": { ... } } ] }
//! ```
//!
//! The set of valid `code` values is fixed by the specification — the
//! conformance suite greps response bodies for these exact strings, so
//! the enum here must never drift from §3.1.

use axum::Json;
use axum::response::{IntoResponse, Response};
use http::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Registry error codes defined by OCI Distribution Spec v1.1 §3.1.
///
/// The `Display` impl emits the uppercase-with-underscores string that
/// appears on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OciErrorCode {
    /// Blob unknown to registry. Spec §3.1.
    BlobUnknown,
    /// Blob upload invalid. Spec §3.1.
    BlobUploadInvalid,
    /// Blob upload unknown to registry. Spec §3.1.
    BlobUploadUnknown,
    /// Provided digest did not match uploaded content.
    DigestInvalid,
    /// Blob unknown to registry (during manifest PUT).
    ManifestBlobUnknown,
    /// Manifest invalid.
    ManifestInvalid,
    /// Manifest unknown to registry.
    ManifestUnknown,
    /// Invalid repository name.
    NameInvalid,
    /// Repository name not known to registry.
    NameUnknown,
    /// Provided length did not match content length.
    SizeInvalid,
    /// Authentication required.
    Unauthorized,
    /// Requested access to the resource is denied.
    Denied,
    /// The operation is unsupported.
    Unsupported,
    /// The client has been rate-limited.
    TooManyRequests,
}

impl OciErrorCode {
    /// Wire string used in the JSON error body.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::BlobUnknown => "BLOB_UNKNOWN",
            Self::BlobUploadInvalid => "BLOB_UPLOAD_INVALID",
            Self::BlobUploadUnknown => "BLOB_UPLOAD_UNKNOWN",
            Self::DigestInvalid => "DIGEST_INVALID",
            Self::ManifestBlobUnknown => "MANIFEST_BLOB_UNKNOWN",
            Self::ManifestInvalid => "MANIFEST_INVALID",
            Self::ManifestUnknown => "MANIFEST_UNKNOWN",
            Self::NameInvalid => "NAME_INVALID",
            Self::NameUnknown => "NAME_UNKNOWN",
            Self::SizeInvalid => "SIZE_INVALID",
            Self::Unauthorized => "UNAUTHORIZED",
            Self::Denied => "DENIED",
            Self::Unsupported => "UNSUPPORTED",
            Self::TooManyRequests => "TOOMANYREQUESTS",
        }
    }

    /// HTTP status code recommended by the spec for this code.
    #[must_use]
    pub const fn status(self) -> StatusCode {
        match self {
            Self::BlobUnknown
            | Self::BlobUploadUnknown
            | Self::ManifestBlobUnknown
            | Self::ManifestUnknown
            | Self::NameUnknown => StatusCode::NOT_FOUND,
            Self::BlobUploadInvalid
            | Self::DigestInvalid
            | Self::ManifestInvalid
            | Self::NameInvalid
            | Self::SizeInvalid => StatusCode::BAD_REQUEST,
            Self::Unauthorized => StatusCode::UNAUTHORIZED,
            Self::Denied => StatusCode::FORBIDDEN,
            Self::Unsupported => StatusCode::METHOD_NOT_ALLOWED,
            Self::TooManyRequests => StatusCode::TOO_MANY_REQUESTS,
        }
    }
}

impl std::fmt::Display for OciErrorCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One entry in the error response array.
///
/// Spec §3.1 requires `code` and `message`; `detail` is optional and may
/// be any JSON value.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OciErrorInfo {
    /// Error code string (uppercase, underscore-separated).
    pub code: String,
    /// Human-readable message.
    pub message: String,
    /// Optional machine-readable detail payload.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<Value>,
}

/// Top-level JSON response body for an error.
///
/// Spec §3.1: `{"errors": [...]}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OciErrorBody {
    /// Errors, non-empty.
    pub errors: Vec<OciErrorInfo>,
}

/// The error type returned by every handler in this crate.
///
/// It carries both the spec-defined [`OciErrorCode`] (which determines
/// the JSON `code` and HTTP status) and a free-form message. An optional
/// status override covers cases like `405 Method Not Allowed` for a
/// manifest DELETE-by-tag, which spec §3.1 doesn't have a dedicated
/// code for.
#[derive(Debug, Clone, thiserror::Error)]
#[error("{code}: {message}")]
pub struct OciError {
    /// Spec-defined error code.
    pub code: OciErrorCode,
    /// Human-readable message.
    pub message: String,
    /// Machine-readable detail, forwarded into the response body.
    pub detail: Option<Value>,
    /// Optional status override (e.g. 405 for DELETE-by-tag).
    pub status_override: Option<StatusCode>,
}

impl OciError {
    /// Build a new error from a code and message.
    pub fn new(code: OciErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            detail: None,
            status_override: None,
        }
    }

    /// Attach a JSON `detail` payload to the error.
    #[must_use]
    pub fn with_detail(mut self, detail: Value) -> Self {
        self.detail = Some(detail);
        self
    }

    /// Override the HTTP status independent of the error code's default.
    #[must_use]
    pub fn with_status(mut self, status: StatusCode) -> Self {
        self.status_override = Some(status);
        self
    }

    /// Final HTTP status to return.
    #[must_use]
    pub fn status(&self) -> StatusCode {
        self.status_override.unwrap_or_else(|| self.code.status())
    }

    /// Build the JSON body.
    #[must_use]
    pub fn body(&self) -> OciErrorBody {
        OciErrorBody {
            errors: vec![OciErrorInfo {
                code: self.code.to_string(),
                message: self.message.clone(),
                detail: self.detail.clone(),
            }],
        }
    }
}

impl IntoResponse for OciError {
    fn into_response(self) -> Response {
        let status = self.status();
        let body = self.body();
        (status, Json(body)).into_response()
    }
}

/// Convenience alias for handler results.
pub type OciResult<T> = Result<T, OciError>;

/// Map a [`ferro_blob_store::FerroRepoError`] onto an [`OciError`].
///
/// Called at the edge of every handler so protocol-crate code can use
/// the workspace-wide `Result` type while still surfacing spec-shaped
/// responses.
impl From<ferro_blob_store::FerroRepoError> for OciError {
    fn from(err: ferro_blob_store::FerroRepoError) -> Self {
        use ferro_blob_store::FerroRepoError as F;
        match err {
            F::BlobNotFound(_) => Self::new(OciErrorCode::BlobUnknown, err.to_string()),
            F::Digest(msg) => Self::new(OciErrorCode::DigestInvalid, msg),
            F::InvalidRequest(msg) => Self::new(OciErrorCode::ManifestInvalid, msg),
            F::Auth(msg) => Self::new(OciErrorCode::Unauthorized, msg),
            F::Unsupported(msg) => Self::new(OciErrorCode::Unsupported, msg),
            other => Self::new(OciErrorCode::Unsupported, other.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{OciError, OciErrorCode};
    use http::StatusCode;

    #[test]
    fn code_wire_strings_match_spec() {
        assert_eq!(OciErrorCode::BlobUnknown.as_str(), "BLOB_UNKNOWN");
        assert_eq!(
            OciErrorCode::BlobUploadInvalid.as_str(),
            "BLOB_UPLOAD_INVALID"
        );
        assert_eq!(
            OciErrorCode::ManifestBlobUnknown.as_str(),
            "MANIFEST_BLOB_UNKNOWN"
        );
        assert_eq!(OciErrorCode::NameInvalid.as_str(), "NAME_INVALID");
        assert_eq!(OciErrorCode::TooManyRequests.as_str(), "TOOMANYREQUESTS");
    }

    #[test]
    fn default_statuses_align_with_spec() {
        assert_eq!(OciErrorCode::BlobUnknown.status(), StatusCode::NOT_FOUND);
        assert_eq!(
            OciErrorCode::DigestInvalid.status(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            OciErrorCode::Unauthorized.status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(OciErrorCode::Denied.status(), StatusCode::FORBIDDEN);
    }

    #[test]
    fn body_contains_single_error_entry() {
        let err = OciError::new(OciErrorCode::NameInvalid, "bad name");
        let body = err.body();
        assert_eq!(body.errors.len(), 1);
        assert_eq!(body.errors[0].code, "NAME_INVALID");
        assert_eq!(body.errors[0].message, "bad name");
    }

    #[test]
    fn status_override_wins_over_code_default() {
        let err = OciError::new(OciErrorCode::Unsupported, "no delete by tag")
            .with_status(StatusCode::METHOD_NOT_ALLOWED);
        assert_eq!(err.status(), StatusCode::METHOD_NOT_ALLOWED);
    }
}
