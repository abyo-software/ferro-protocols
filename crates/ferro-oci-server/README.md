<!-- SPDX-License-Identifier: Apache-2.0 -->
# ferro-oci-server

[![License](https://img.shields.io/badge/License-Apache--2.0-blue.svg)](../../LICENSE)
[![crates.io](https://img.shields.io/crates/v/ferro-oci-server.svg)](https://crates.io/crates/ferro-oci-server)
[![docs.rs](https://docs.rs/ferro-oci-server/badge.svg)](https://docs.rs/ferro-oci-server)
[![CI](https://github.com/abyo-software/ferro-protocols/actions/workflows/ci.yml/badge.svg)](https://github.com/abyo-software/ferro-protocols/actions/workflows/ci.yml)
[![Rust 1.88+](https://img.shields.io/badge/rust-1.88%2B-orange.svg)](../../rust-toolchain.toml)
[![OCI Distribution Spec v1.1](https://img.shields.io/badge/OCI%20Distribution%20Spec-v1.1-blue.svg)](https://github.com/opencontainers/distribution-spec/blob/v1.1.0/spec.md)

**Embeddable** server-side primitives for the
[OCI Distribution Specification v1.1](https://github.com/opencontainers/distribution-spec/blob/v1.1.0/spec.md).
Manifest / Blob / Tag / Referrers handlers as an Axum router, a
chunked-upload state machine, the spec §6.2 error envelope, and a
metadata-plane trait that keeps the storage layer abstract — plus a
ready-to-run `ferro-oci-server` binary with Kubernetes health probes.

> ✅ **Passes the official OCI Distribution Spec v1.1 conformance suite:
> 75/75 specs (Push, Pull, Content Discovery, Content Management).**
> See [`tests/conformance/RESULTS.md`](tests/conformance/RESULTS.md)
> for the real run and
> [`tests/conformance/run_conformance.sh`](tests/conformance/run_conformance.sh)
> to reproduce.

> The Rust ecosystem has had [`oci-client`](https://crates.io/crates/oci-client)
> (formerly `oci-distribution`) and [`oci-spec`](https://crates.io/crates/oci-spec)
> for years. What's been missing is the **server**. The dominant
> open OCI registries —
> [Harbor](https://github.com/goharbor/harbor),
> [zot](https://github.com/project-zot/zot), and the
> [`distribution/distribution`](https://github.com/distribution/distribution)
> reference impl — are all Go. There has been no embeddable Rust
> answer for the OCI Distribution Spec server side. This crate is
> a starting point for one.

> 🟢 **Stable (`v1.0.0`).** The public API is committed under strict
> semver: breaking changes require a major bump. The server passes the
> official OCI Distribution Spec v1.1 conformance suite: **75/75 specs,
> 0 failures** (harness in `tests/conformance/`).

Part of the **Ferro ecosystem**. Extracted from FerroRepo, a private
Rust artifact repository.

## What this crate does

- **`router` / `handlers`** — Axum router for `/v2/**`:
  - `GET /v2/` — version check (200 OK)
  - `GET|HEAD|DELETE /v2/{name}/blobs/{digest}` — blob plane
  - `POST|PATCH|PUT /v2/{name}/blobs/uploads/{uuid?}` — chunked +
    monolithic upload state machine
  - `GET|HEAD|PUT|DELETE /v2/{name}/manifests/{reference}` —
    manifest plane (digest or tag references)
  - `GET /v2/{name}/tags/list` — tag listing with pagination
  - `GET /v2/_catalog` — repository listing with pagination
  - `GET /v2/{name}/referrers/{digest}` — referrer index per spec
    §6.7
- **`error`** — `OciError` plus `OciErrorCode` enum that renders
  the spec's error JSON envelope (`{ "errors": [...] }`)
- **`manifest`** — `ImageManifest`, `ImageIndex`, `Descriptor`
  serde types
- **`media_types`** — Media-type classification (Docker v2 vs OCI v1
  manifests, configs, layers)
- **`reference`** — image-name + tag + digest parsing with
  spec-compliant validation
- **`registry`** — `RegistryMeta` trait (manifest + tag + upload +
  referrer book-keeping). Reference impl `InMemoryRegistryMeta`
  using `parking_lot::RwLock` + `BTreeMap`.
- **`upload`** — `UploadState`, `ContentRange` parser

## What this crate does **not** do

- **External metadata database** — only `InMemoryRegistryMeta` ships.
  A SQLite / Postgres backend is on the roadmap; the trait is stable
  enough that you can implement your own today. Note that the
  filesystem deployment *does* durably persist metadata to a
  `metadata.json` snapshot under `FERRO_OCI_STORAGE_DIR` (see below),
  so manifests/tags/referrers survive a restart even without an
  external database.
- **Authentication** — handlers are open. Layer your auth in the
  Axum middleware stack above this router.
- **Sigstore / SLSA / TUF / cosign** — the OCI server stores and
  serves manifests but does not sign or verify. Those live in
  separate crates (not yet published).

## Run the server

A ready-to-run binary ships with the crate. It is configured entirely
through the environment:

| Variable                | Default        | Meaning                                          |
|-------------------------|----------------|--------------------------------------------------|
| `FERRO_OCI_LISTEN`      | `0.0.0.0:8080` | `host:port` to bind                              |
| `FERRO_OCI_STORAGE_DIR` | *(in-memory)*  | filesystem dir for blob bytes **and** metadata; unset → RAM (ephemeral) |
| `RUST_LOG`              | `info`         | `tracing-subscriber` env filter                  |

When `FERRO_OCI_STORAGE_DIR` is set the registry metadata plane
(manifests, tag aliases, referrer index) is durably mirrored to a
`metadata.json` snapshot in that directory (write-through on every
mutation, atomic temp-file + rename) and reloaded on boot, so blobs and
their manifests/tags survive a restart together. A missing or corrupt
snapshot is tolerated — the registry starts empty and logs a warning.
In-flight upload sessions are intentionally *not* persisted (a restart
aborts them). With `FERRO_OCI_STORAGE_DIR` unset, both blobs and
metadata are in-memory only and lost on exit.

```bash
FERRO_OCI_LISTEN=127.0.0.1:5000 \
FERRO_OCI_STORAGE_DIR=/var/lib/oci-registry \
  cargo run --bin ferro-oci-server -p ferro-oci-server

curl -s localhost:5000/healthz   # {"status":"ok"}
curl -s localhost:5000/v2/       # {}
docker push localhost:5000/myimage:latest
```

It exposes Kubernetes-style probe endpoints alongside the `/v2/**`
surface and shuts down gracefully on `SIGTERM`/`SIGINT`:

- `GET /live`    — liveness (`200 OK`, body `OK`)
- `GET /healthz` — health (`200 OK`, JSON `{"status":"ok"}`)
- `GET /ready`   — readiness (`200 OK`, body `OK`)

## Embed it in your own server

```rust,no_run
use std::sync::Arc;
use axum::Router;
use ferro_blob_store::FsBlobStore;
use ferro_oci_server::{router, probe_routes, AppState, InMemoryRegistryMeta};

# async fn run() -> Result<(), Box<dyn std::error::Error>> {
let store = Arc::new(FsBlobStore::new("/var/lib/oci-registry")?);
let meta = Arc::new(InMemoryRegistryMeta::new());
let state = AppState::new(store, meta);
// OCI `/v2/**` + Kubernetes health probes.
let app: Router = router(state).merge(probe_routes());
let listener = tokio::net::TcpListener::bind("0.0.0.0:5000").await?;
axum::serve(listener, app).await?;
# Ok(()) }
```

Then `docker push localhost:5000/myimage:latest` should work
against the running server.

## Conformance

The crate passes the official upstream
[`opencontainers/distribution-spec` conformance test suite](https://github.com/opencontainers/distribution-spec/tree/main/conformance)
end-to-end — **75/75 specs across all four workflow categories**
(Push, Pull, Content Discovery, Content Management). The real run and
its honest changelog (including the two server bugs the suite caught)
are recorded in [`tests/conformance/RESULTS.md`](tests/conformance/RESULTS.md);
[`tests/conformance/run_conformance.sh`](tests/conformance/run_conformance.sh)
boots the server, runs the suite (Go toolchain or prebuilt Docker
image), and writes a JUnit + HTML report, so you can reproduce it in
CI or on a workstation.

In addition, `tests/conformance_smoke.rs` exercises the full
in-process request walk for every endpoint pair (start upload →
chunked PATCH → finalize PUT → blob HEAD/GET → manifest PUT-by-tag →
manifest GET-by-digest → referrers GET → tag list → catalog → delete)
and the error variants in spec §6.2.

An external metadata database (SQLite / Postgres) and an
authentication trait remain on the roadmap. The filesystem deployment
already persists metadata durably via a `metadata.json` snapshot (see
"Run the server").

## Status

| Aspect | Status |
|---|---|
| API stability | **stable** (`v1.x`) — strict semver from `1.0.0` |
| Conformance | official OCI Distribution v1.1 suite: 75/75 specs, 0 failures |
| Manifest / blob / tag / catalog / referrers handlers | working |
| Chunked uploads | working |
| Runnable server binary + K8s probes (`/live`, `/healthz`, `/ready`) | working |
| OCI v1.1 conformance suite | **75/75 specs pass** (all 4 workflows) |
| Metadata durability | filesystem deployment persists to `metadata.json`; external DB backend (SQLite/Postgres) on roadmap |
| Authentication | scaffold — layer your own middleware |
| MSRV | rustc **1.88** |

## Used in production by

- **FerroRepo** (private) — Rust artifact repository.

## License

Apache-2.0. See [`LICENSE`](../../LICENSE).
