<!-- SPDX-License-Identifier: Apache-2.0 -->
# ferro-blob-store

[![crates.io](https://img.shields.io/crates/v/ferro-blob-store.svg)](https://crates.io/crates/ferro-blob-store)
[![docs.rs](https://img.shields.io/docsrs/ferro-blob-store)](https://docs.rs/ferro-blob-store)
[![CI](https://github.com/abyo-software/ferro-protocols/actions/workflows/ci.yml/badge.svg)](https://github.com/abyo-software/ferro-protocols/actions/workflows/ci.yml)
[![License](https://img.shields.io/badge/License-Apache--2.0-blue.svg)](../../LICENSE)
[![Rust 1.88+](https://img.shields.io/badge/rust-1.88%2B-orange.svg)](../../rust-toolchain.toml)

Foundation crate for content-addressed blob storage in the Ferro
ecosystem. A deliberately tiny `async fn` trait (5 methods) plus
two reference backends (in-memory + filesystem) plus a
`Digest` newtype with SHA-256 / SHA-512 support.

> 🟢 **Stable (`v1.0.0`).** The public API is committed under strict
> semver: breaking changes require a major bump. The trait surface is
> minimal on purpose so streaming variants can be added additively in a
> future minor.

Part of the **Ferro ecosystem**. Used as the storage abstraction
under [`ferro-oci-server`](https://crates.io/crates/ferro-oci-server),
[`ferro-maven-layout`](https://crates.io/crates/ferro-maven-layout),
and [`ferro-cargo-registry-server`](https://crates.io/crates/ferro-cargo-registry-server).

## What this crate does

- **`Digest`** — an `<algo>:<hex>` content identifier (SHA-256 or
  SHA-512). Validates hex length and character set on construction.
  Computes from bytes via `Digest::sha256_of(&[u8])`.
- **`BlobStore`** — five-method async trait: `put`, `get`,
  `contains`, `delete`, `list`. Writers verify the SHA-256 of the
  input matches the supplied digest. Implementations are expected to
  be `Send + Sync`.
- **`InMemoryBlobStore`** — `Arc<RwLock<HashMap<Digest, Bytes>>>`
  reference implementation. Useful for tests and ephemeral
  caches.
- **`FsBlobStore`** (default feature `fs`) — local-filesystem backend
  that lays out blobs at `<root>/<algo>/<2-char-prefix>/<rest-of-hex>`.
  Atomic writes via temp-file + rename.

## What this crate does **not** do

- **Streaming** (`put_stream` / `get_stream`). Planned as an additive
  minor on the `v1.x` track.
- **Cloud backends** (S3, GCS, Azure). Use the `object_store` crate
  family and write a 50-line adapter; the trait is small enough.
- **Tiered storage** (Hot/Warm/Cold). The Ferro internal repo has a
  router for this; it is not in the public crate.
- **Replication / dedupe / compression**. Layer it under your own
  `BlobStore` impl that wraps an inner one.

## Quick start

```rust
use ferro_blob_store::{BlobStore, Digest, InMemoryBlobStore};
use bytes::Bytes;

# async fn run() -> Result<(), Box<dyn std::error::Error>> {
let store = InMemoryBlobStore::new();
let body = Bytes::from_static(b"hello world");
let digest = Digest::sha256_of(&body);

store.put(&digest, body.clone()).await?;
assert!(store.contains(&digest).await?);
assert_eq!(store.get(&digest).await?, body);
assert_eq!(store.list().await?.len(), 1);
# Ok(()) }
```

Filesystem variant:

```rust,no_run
use ferro_blob_store::{BlobStore, Digest, FsBlobStore};
use bytes::Bytes;

# async fn run() -> Result<(), Box<dyn std::error::Error>> {
let store = FsBlobStore::new("/var/lib/my-registry/blobs")?;
let body = Bytes::from_static(b"layer bytes");
let digest = Digest::sha256_of(&body);
store.put(&digest, body).await?;
# Ok(()) }
```

## Status

| Aspect | Status |
|---|---|
| API stability | **stable** (`v1.x`) — strict semver from `1.0.0` |
| Backends | `InMemoryBlobStore` ✅ / `FsBlobStore` ✅ (default feature) |
| Streaming I/O | not yet — additive in a future minor |
| MSRV | rustc **1.88** |
| Async runtime | Tokio (for `FsBlobStore`); the trait itself is runtime-agnostic |

## Used in production by

- [**ferro-oci-server**](https://crates.io/crates/ferro-oci-server) — OCI
  Distribution v1.1 server primitives.
- [**ferro-maven-layout**](https://crates.io/crates/ferro-maven-layout) —
  Maven Repository Layout 2.0 + HTTP handlers.
- [**ferro-cargo-registry-server**](https://crates.io/crates/ferro-cargo-registry-server) —
  Cargo Alternative Registry sparse-index server.

## License

Apache-2.0. See [`LICENSE`](../../LICENSE).
