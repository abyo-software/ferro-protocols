// SPDX-License-Identifier: Apache-2.0
//! Runnable Cargo registry server.
//!
//! Boots the [`ferro_cargo_registry_server::router`] over a
//! filesystem-backed [`ferro_blob_store::FsBlobStore`] and serves the
//! Cargo Alternative Registry Protocol (sparse-index variant). See
//! [RFC 2789] / the [Cargo registry reference] for the wire protocol.
//!
//! [RFC 2789]: https://rust-lang.github.io/rfcs/2789-sparse-index.html
//! [Cargo registry reference]: https://doc.rust-lang.org/cargo/reference/registries.html
//!
//! All boot logic lives in [`ferro_cargo_registry_server::serve`]; this
//! binary is a thin shim that reads the environment and serves.
//!
//! ## Configuration (environment)
//!
//! | Variable | Default | Purpose |
//! |---|---|---|
//! | `FERRO_CARGO_REGISTRY_DATA`   | `./registry-data` | Blob-store data directory |
//! | `FERRO_CARGO_REGISTRY_LISTEN` | `0.0.0.0:8081`    | Listen socket address |
//! | `FERRO_CARGO_REGISTRY_API`    | `http://<listen>` | API host advertised in `/config.json` |
//!
//! ## Probe routes
//!
//! In addition to the protocol routes, the binary mounts
//! Kubernetes-style probes:
//!
//! - `GET /live`    — liveness, returns `200 OK`.
//! - `GET /ready`   — readiness, returns `200 OK`.
//! - `GET /healthz` — health, returns `200 OK` with `{"status":"ok"}`.

use std::process::ExitCode;

use ferro_cargo_registry_server::{Config, init_tracing, serve};

#[tokio::main]
async fn main() -> ExitCode {
    init_tracing();
    let config = Config::from_env();
    match serve(&config).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("ferro-cargo-registry-server: fatal: {err}");
            ExitCode::FAILURE
        }
    }
}
