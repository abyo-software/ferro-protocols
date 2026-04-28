<!-- SPDX-License-Identifier: Apache-2.0 -->
# ferro-oci-server

[![License](https://img.shields.io/badge/License-Apache--2.0-blue.svg)](../../LICENSE)
[![crates.io](https://img.shields.io/crates/v/ferro-oci-server.svg)](https://crates.io/crates/ferro-oci-server)
[![docs.rs](https://docs.rs/ferro-oci-server/badge.svg)](https://docs.rs/ferro-oci-server)
[![Rust 1.88+](https://img.shields.io/badge/rust-1.88%2B-orange.svg)](../../rust-toolchain.toml)

**Embeddable** server-side primitives for the
[OCI Distribution Specification v1.1](https://github.com/opencontainers/distribution-spec).
Manifest / Blob / Tag / Referrers handlers as an Axum router, a
chunked-upload state machine, the spec §6.2 error envelope, and a
metadata-plane trait that keeps the storage layer abstract.

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

> ⚠️ **Alpha (`v0.0.1`).** API will shift before `v0.1`. See
> [Roadmap to v0.1.0](#roadmap-to-v010) below for the explicit gate.

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

- **Persistent metadata** — only `InMemoryRegistryMeta` ships. A
  SQLite / Postgres backend is on the roadmap; the trait is stable
  enough that you can implement your own today.
- **Authentication** — handlers are open. Layer your auth in the
  Axum middleware stack above this router.
- **Sigstore / SLSA / TUF / cosign** — the OCI server stores and
  serves manifests but does not sign or verify. Those live in
  separate crates (not yet published).
- **Conformance suite vendoring** — `tests/conformance_smoke.rs`
  exercises the request paths end-to-end, but the upstream
  `opencontainers/distribution-spec` conformance harness is not
  yet wired in. Doing so is the gate for `v0.1.0`.

## Quick start

```rust,no_run
use std::sync::Arc;
use axum::Router;
use ferro_blob_store::FsBlobStore;
use ferro_oci_server::{router, AppState, registry::InMemoryRegistryMeta};

# async fn run() -> Result<(), Box<dyn std::error::Error>> {
let store = Arc::new(FsBlobStore::new("/var/lib/oci-registry")?);
let meta = Arc::new(InMemoryRegistryMeta::default());
let state = AppState::new(store, meta);
let app: Router = router(state);
let listener = tokio::net::TcpListener::bind("0.0.0.0:5000").await?;
axum::serve(listener, app).await?;
# Ok(()) }
```

Then `docker push localhost:5000/myimage:latest` should work
against the running server.

## Roadmap to v0.1.0

The `v0.0.x` → `v0.1.0` promotion gate is one explicit milestone:

> The crate must pass the upstream
> [`opencontainers/distribution-spec` conformance test suite](https://github.com/opencontainers/distribution-spec/tree/main/conformance)
> end-to-end.

Today's coverage is **smoke-test grade**: `tests/conformance_smoke.rs`
exercises the full request walk for every endpoint pair (start
upload → chunked PATCH → finalize PUT → blob HEAD/GET → manifest
PUT-by-tag → manifest GET-by-digest → referrers GET → tag list →
catalog → delete) and the error path for the variants in spec §6.2.
That gives confidence that the wire shape is right, but it is not
the same as passing the official conformance harness.

Vendoring the upstream Go-based conformance suite into the test
matrix is tracked in
[issue #1](https://github.com/abyo-software/ferro-protocols/issues/1).
Persistent metadata backends (SQLite / Postgres) and an
authentication trait are also gated against `v0.1.0`.

## Status

| Aspect | Status |
|---|---|
| API stability | **alpha** (`v0.0.x`) |
| Manifest / blob / tag / catalog / referrers handlers | working |
| Chunked uploads | working |
| Conformance suite | smoke tests only — formal harness pending |
| Persistent metadata backend | in-memory only |
| Authentication | scaffold — layer your own middleware |
| MSRV | rustc **1.88** |

## Used in production by

- **FerroRepo** (private) — Rust artifact repository.

## License

Apache-2.0. See [`LICENSE`](../../LICENSE).
