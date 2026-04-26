// SPDX-License-Identifier: Apache-2.0
#![doc = include_str!("../README.md")]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![deny(missing_docs)]

mod blob_store;
mod digest;
mod error;
mod memory;

#[cfg(feature = "fs")]
#[cfg_attr(docsrs, doc(cfg(feature = "fs")))]
mod fs;

pub use blob_store::BlobStore;
pub use digest::{Digest, DigestAlgo, DigestParseError};
pub use error::BlobStoreError;
pub use memory::InMemoryBlobStore;

#[cfg(feature = "fs")]
#[cfg_attr(docsrs, doc(cfg(feature = "fs")))]
pub use fs::FsBlobStore;

/// Convenience [`Result`] alias.
///
/// [`Result`]: core::result::Result
pub type Result<T> = core::result::Result<T, BlobStoreError>;

/// Crate name, exposed for diagnostics and `/metrics` labelling.
pub const CRATE_NAME: &str = "ferro-blob-store";
