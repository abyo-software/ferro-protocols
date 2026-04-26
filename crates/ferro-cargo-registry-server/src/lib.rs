// SPDX-License-Identifier: Apache-2.0
//! `ferro-cargo-registry-server`
//!
//! Cargo registry protocol for FerroRepo — sparse-index + publish /
//! yank / owners / download endpoints. The Git-index protocol is
//! wired with a 501 stub in Phase 1; Cargo itself defaults to sparse
//! since 1.68 (`CARGO_REGISTRIES_*_PROTOCOL=sparse`), so the stub is a
//! no-op for the client flows we target.
//!
//! ## Spec references
//!
//! - Registry reference —
//!   <https://doc.rust-lang.org/cargo/reference/registries.html>
//! - Registry Web API —
//!   <https://doc.rust-lang.org/cargo/reference/registry-web-api.html>
//! - Index format —
//!   <https://doc.rust-lang.org/cargo/reference/registries.html#index-format>
//! - Publish pre-image layout (binary) —
//!   registry-web-api.html#publish

#![deny(missing_docs)]

pub mod config;
pub mod error;
pub mod handlers;
pub mod index;
pub mod name;
pub mod owners;
pub mod publish;
pub mod router;
pub mod version;
pub mod yank;

pub use config::IndexConfig;
pub use error::CargoError;
pub use index::{IndexDep, IndexEntry, entry_from_manifest, parse_lines, render_lines};
pub use name::{MAX_NAME_LEN, index_path, is_valid_name, validate_name};
pub use owners::{Owner, OwnersMutationResponse, OwnersRequest, OwnersResponse};
pub use publish::{PublishRequest, encode as encode_publish_body, parse as parse_publish_body};
pub use router::{CargoState, CrateRecord, router};
pub use version::is_valid_semver;
pub use yank::YankResponse;

/// Crate name, exposed for diagnostics and `/metrics` labelling.
pub const CRATE_NAME: &str = "ferro-cargo-registry-server";

#[cfg(test)]
mod tests {
    use super::CRATE_NAME;

    #[test]
    fn crate_name_is_stable() {
        assert_eq!(CRATE_NAME, "ferro-cargo-registry-server");
    }
}
