// SPDX-License-Identifier: Apache-2.0
//! `ferro-oci-server` — a runnable OCI Distribution Spec v1.1 registry.
//!
//! Boots the [`ferro_oci_server::router`] over a filesystem-backed blob
//! store and the in-memory metadata plane, merges in the Kubernetes
//! health probes (`/live`, `/healthz`, `/ready`), and serves it with
//! graceful `SIGTERM`/`SIGINT` shutdown.
//!
//! All boot logic lives in [`ferro_oci_server::serve`]; this binary is a
//! thin shim that reads the environment and serves.
//!
//! # Configuration
//!
//! All configuration is environment-driven (see
//! [`ferro_oci_server::Config`]):
//!
//! | Variable                  | Default          | Meaning                                   |
//! |---------------------------|------------------|-------------------------------------------|
//! | `FERRO_OCI_LISTEN`        | `0.0.0.0:8080`   | host:port the HTTP server binds to        |
//! | `FERRO_OCI_STORAGE_DIR`   | *(in-memory)*    | filesystem dir for blob bytes; unset → RAM|
//! | `RUST_LOG`                | `info`           | `tracing-subscriber` env filter           |
//!
//! # Example
//!
//! ```bash
//! FERRO_OCI_LISTEN=127.0.0.1:5000 \
//! FERRO_OCI_STORAGE_DIR=/var/lib/oci-registry \
//!   cargo run --bin ferro-oci-server -p ferro-oci-server
//!
//! curl -s localhost:5000/healthz        # {"status":"ok"}
//! curl -s localhost:5000/v2/            # {}
//! docker push localhost:5000/alpine:latest
//! ```

use ferro_oci_server::{Config, init_tracing, serve};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    init_tracing();
    let config = Config::from_env();
    serve(&config).await
}
