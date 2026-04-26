// SPDX-License-Identifier: Apache-2.0
//! Error types.

use thiserror::Error;

use crate::digest::DigestParseError;

/// Errors emitted by [`crate::BlobStore`] implementations.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum BlobStoreError {
    /// I/O error while reading from or writing to the backing store.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    /// The bytes supplied to [`crate::BlobStore::put`] did not match
    /// the supplied [`crate::Digest`].
    #[error("digest mismatch on put: caller said {expected}, computed {computed}")]
    DigestMismatch {
        /// Digest the caller asserted.
        expected: String,
        /// Digest the implementation computed from the supplied bytes.
        computed: String,
    },

    /// No blob exists for the supplied [`crate::Digest`].
    #[error("blob not found: {0}")]
    NotFound(String),

    /// A wire `<algo>:<hex>` string failed to parse.
    #[error("invalid digest: {0}")]
    InvalidDigest(#[from] DigestParseError),
}
