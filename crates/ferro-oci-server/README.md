<!-- SPDX-License-Identifier: Apache-2.0 -->
# ferro-oci-server

[![License](https://img.shields.io/badge/License-Apache--2.0-blue.svg)](../../LICENSE)
[![Rust 1.88+](https://img.shields.io/badge/rust-1.88%2B-orange.svg)](../../rust-toolchain.toml)

**Server-side** primitives for the
[OCI Distribution Specification v1.1](https://github.com/opencontainers/distribution-spec).
Implements the Manifest / Blob / Tag / Referrers API surface as an
Axum router, plus a chunked-upload state machine, error envelope
that matches §6.2 of the spec, and a metadata-plane trait that
keeps the storage layer abstract.

> ⚠️ **Alpha (`v0.0.1`).** API will shift before `v0.1`. The
> roadmap target for `v0.1.0` is "passes the upstream
> `opencontainers/distribution-spec` conformance test suite".

Part of the **Ferro ecosystem**. Existing Rust crates implement the
OCI client (`oci-client`) and types (`oci-spec`); as far as we can
tell, this is the first crate on crates.io that publishes the
*server* half.

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
