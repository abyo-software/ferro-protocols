<!-- SPDX-License-Identifier: Apache-2.0 -->
# ferro-maven-layout

[![License](https://img.shields.io/badge/License-Apache--2.0-blue.svg)](../../LICENSE)
[![Rust 1.88+](https://img.shields.io/badge/rust-1.88%2B-orange.svg)](../../rust-toolchain.toml)

Apache Maven Repository Layout 2.0 in Rust — GAV (`groupId:artifactId:version`)
parsing, `maven-metadata.xml`, minimal POM parser, SNAPSHOT timestamp +
buildNumber, SHA-1 / SHA-256 / MD5 checksum helpers, and an Axum
HTTP router that mounts the layout at any URL prefix.

> 🟢 **Beta (`v0.1.0`).** Public API is stable for the `v0.1.x`
> series; additive changes only between minors.

Part of the **Ferro ecosystem**. Extracted from FerroRepo, a private
Rust artifact repository.

## What this crate does

- **`coordinate`** — parses `groupId:artifactId:version[:classifier]`
  forms, validates the safe-charset rule, surfaces structured errors.
- **`layout`** — turns a request path into a typed `LayoutPath`
  (artifact / metadata / sidecar) without doing I/O.
- **`metadata`** — `maven-metadata.xml` types and serialiser, built on
  `quick-xml`.
- **`pom`** — minimal POM reader (groupId, artifactId, version, parent
  GAV); enough for path / coordinate validation. Not a full Maven
  effective-model resolver.
- **`snapshot`** — SNAPSHOT timestamp generation
  (`yyyyMMdd.HHmmss-buildNumber`).
- **`checksum`** — SHA-1, SHA-256, and (gated under `legacy-md5`) MD5
  helpers + sidecar parser.
- **`router` / `handlers`** (default feature `http`) — Axum router
  for `GET / HEAD / PUT / DELETE`. Mounts a `BlobStore` from
  [`ferro-blob-store`](https://crates.io/crates/ferro-blob-store) and
  serves Maven traffic under any prefix.

## What this crate does **not** do

- **Effective POM resolution**: no parent-chain merging, no profile
  activation, no property substitution. The bundled POM parser is
  layout-validation grade only.
- **Repository proxying / mirrors**: no upstream fetch, no caching.
- **Authentication**: handlers are open; layer your auth in the
  Axum middleware stack above this router.
- **Search / index**: no full-text or coordinate-search API; this is
  a layout primitive, not a full Nexus / Artifactory replacement.

## Quick start

```rust,no_run
use std::sync::Arc;
use axum::Router;
use ferro_blob_store::FsBlobStore;
use ferro_maven_layout::{router, MavenState};

# async fn run() -> Result<(), Box<dyn std::error::Error>> {
let store = Arc::new(FsBlobStore::new("/var/lib/maven-repo")?);
let state = MavenState::new(store);
let app: Router = router(state);
// app.layer(...) // your auth / tracing / rate-limit middleware
let listener = tokio::net::TcpListener::bind("0.0.0.0:8081").await?;
axum::serve(listener, app).await?;
# Ok(()) }
```

Pure-data layout parsing (no `http` feature needed):

```rust
use ferro_maven_layout::{parse_layout_path, PathClass};

let path = parse_layout_path("repo/com/example/lib/1.0.0/lib-1.0.0.jar").unwrap();
assert!(matches!(path.class, PathClass::Artifact { .. }));
```

## Status

| Aspect | Status |
|---|---|
| API stability | **beta** (`v0.1.x`) — additive-only between minors |
| Feature: `http` (default) | Axum router + handlers |
| Feature: `legacy-md5` | MD5 sidecar acceptance for Maven 2 clients |
| MSRV | rustc **1.88** |
| Async runtime | Tokio (only when `http` is enabled) |
| Test fixtures | All inline strings — no vendored Maven artefacts |

## Used in production by

- **FerroRepo** (private) — Rust artifact repository, Maven Central
  + private repository support.

## Trademarks

Apache Maven™ is a trademark of the Apache Software Foundation.
This crate implements the Maven repository layout; it is not
endorsed by, or affiliated with, the ASF.

## License

Apache-2.0. See [`LICENSE`](../../LICENSE).
