// SPDX-License-Identifier: Apache-2.0
//! Boot a Cargo registry over a filesystem blob store and serve it.
//!
//! This mirrors what the bundled `ferro-cargo-registry-server` binary
//! does, in the smallest standalone form, so you can see how to embed
//! [`router`] in your own service. It implements the Cargo Alternative
//! Registry Protocol (sparse-index variant, RFC 2789).
//!
//! Run with:
//!
//! ```bash
//! cargo run --example serve_registry -p ferro-cargo-registry-server
//! ```
//!
//! Then, in another shell, point cargo at it via `~/.cargo/config.toml`:
//!
//! ```toml
//! [registries.ferro]
//! index = "sparse+http://127.0.0.1:8088/"
//! ```
//!
//! and run, for example, `cargo publish --registry ferro`.

use std::sync::Arc;

use ferro_blob_store::FsBlobStore;
use ferro_cargo_registry_server::{CargoState, router};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // A throwaway data directory under the system temp dir keeps the
    // example self-contained; a real deployment points this at durable
    // storage (or uses the `ferro-cargo-registry-server` binary, which
    // reads `FERRO_CARGO_REGISTRY_DATA`).
    let data_dir = std::env::temp_dir().join("ferro-cargo-registry-example");
    std::fs::create_dir_all(&data_dir)?;

    let api_host = "http://127.0.0.1:8088";
    let store = Arc::new(FsBlobStore::new(&data_dir)?);
    let state = CargoState::new(store, api_host);
    let app = router(state);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:8088").await?;
    println!("serving Cargo registry on {api_host}");
    println!("  data dir: {}", data_dir.display());
    println!("  config:   {api_host}/config.json");
    println!("press Ctrl-C to stop");

    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}
