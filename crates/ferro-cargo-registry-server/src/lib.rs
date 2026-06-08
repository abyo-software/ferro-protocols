// SPDX-License-Identifier: Apache-2.0
//! `ferro-cargo-registry-server`
//!
//! Server-side primitives for the **Cargo Alternative Registry
//! Protocol** (sparse-index variant, [RFC 2789]) for `FerroRepo` —
//! sparse-index + publish / yank / owners / download endpoints. The
//! Git-index protocol is wired with a 501 stub; Cargo defaults to
//! sparse since 1.68 (`CARGO_REGISTRIES_*_PROTOCOL=sparse`), so the
//! stub is a no-op for the client flows we target.
//!
//! ## Library quick start
//!
//! Mount the [`router()`] over any [`ferro_blob_store::BlobStore`] inside
//! a service you already run:
//!
//! ```rust,no_run
//! use std::sync::Arc;
//! use ferro_blob_store::FsBlobStore;
//! use ferro_cargo_registry_server::{router, CargoState};
//!
//! # async fn run() -> Result<(), Box<dyn std::error::Error>> {
//! let store = Arc::new(FsBlobStore::new("/var/lib/cargo-registry")?);
//! let state = CargoState::new(store, "http://127.0.0.1:8081");
//! let app = router(state);
//! let listener = tokio::net::TcpListener::bind("0.0.0.0:8081").await?;
//! axum::serve(listener, app).await?;
//! # Ok(()) }
//! ```
//!
//! ## Runnable binary
//!
//! The crate also ships a `ferro-cargo-registry-server` binary that
//! boots the router over a filesystem blob store and adds `/live`,
//! `/ready`, `/healthz` probes:
//!
//! ```bash
//! FERRO_CARGO_REGISTRY_LISTEN=0.0.0.0:8081 \
//! FERRO_CARGO_REGISTRY_DATA=./registry-data \
//!   cargo run --bin ferro-cargo-registry-server
//! ```
//!
//! Point cargo at it with
//! `index = "sparse+http://127.0.0.1:8081/"` in `~/.cargo/config.toml`.
//! See `tests/e2e-results.md` for the real-`cargo` round-trip results.
//!
//! ## Spec references
//!
//! - Sparse registry index ([RFC 2789]) —
//!   <https://rust-lang.github.io/rfcs/2789-sparse-index.html>
//! - Alternative registries ([RFC 2141]) —
//!   <https://rust-lang.github.io/rfcs/2141-alternative-registries.html>
//! - Registry reference —
//!   <https://doc.rust-lang.org/cargo/reference/registries.html>
//! - Registry Web API —
//!   <https://doc.rust-lang.org/cargo/reference/registry-web-api.html>
//! - Index format —
//!   <https://doc.rust-lang.org/cargo/reference/registries.html#index-format>
//! - Publish pre-image layout (binary) —
//!   registry-web-api.html#publish
//!
//! [RFC 2789]: https://rust-lang.github.io/rfcs/2789-sparse-index.html
//! [RFC 2141]: https://rust-lang.github.io/rfcs/2141-alternative-registries.html

#![deny(missing_docs)]

pub mod config;
pub mod error;
pub mod handlers;
pub mod index;
pub mod metrics;
pub mod name;
pub mod owners;
pub mod persist;
pub mod publish;
pub mod router;
pub mod serve;
pub mod version;
pub mod yank;

pub use config::IndexConfig;
pub use error::CargoError;
pub use index::{
    IndexDep, IndexEntry, PublishDep, PublishManifest, entry_from_manifest, parse_lines,
    render_lines,
};
pub use metrics::{Metrics, MetricsState, instrument, metrics_routes};
pub use name::{
    MAX_NAME_LEN, canonical_name, index_path, is_valid_name, names_collide, validate_name,
};
pub use owners::{Owner, OwnersMutationResponse, OwnersRequest, OwnersResponse};
pub use publish::{PublishRequest, encode as encode_publish_body, parse as parse_publish_body};
pub use router::{CargoState, CrateRecord, MAX_PUBLISH_BODY_BYTES, router};
pub use serve::{Config, build_app, build_state, init_tracing, probe_routes, serve};
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
