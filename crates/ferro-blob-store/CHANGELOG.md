<!-- SPDX-License-Identifier: Apache-2.0 -->
# Changelog — ferro-blob-store

The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/). This crate
is currently in the `v0.0.x` alpha series; breaking changes are
allowed between any two releases until `v0.1.0`.

## [Unreleased]

## [0.0.3] — 2026-04-26

### Changed
- Doc-only: split `SharedBlobStore` description into a leading
  one-sentence summary + a separate detail paragraph, satisfying
  the workspace `clippy::too_long_first_doc_paragraph` lint.

## [0.0.2] — 2026-04-26

### Added
- `serde` feature: `Digest` gains `Serialize` / `Deserialize` via its
  `<algo>:<hex>` wire string. Needed by protocol crates that put
  digests into JSON manifests (OCI etc.).
- `SharedBlobStore` type alias (`Arc<dyn BlobStore>`) for ergonomic
  cross-task handle passing.

### Changed
- No breaking changes. `serde` is opt-in; `SharedBlobStore` is
  additive.

## [0.0.1] — initial alpha

Initial extraction from the FerroRepo storage layer.

### Added
- `Digest` type with SHA-256 / SHA-512, parsed from `<algo>:<hex>`
  wire form, computed from `&[u8]` via `Digest::sha256_of`.
- `BlobStore` async trait — five methods: `put` / `get` / `contains`
  / `delete` / `list`. Writers verify SHA-256 matches the supplied
  digest.
- `InMemoryBlobStore` — reference backend, `Arc<RwLock<HashMap>>`.
- `FsBlobStore` (default feature `fs`) — atomic-rename filesystem
  backend, layout `<root>/<algo>/<2-char-prefix>/<rest-of-hex>`.
- `BlobStoreError` enum (Io, Digest, NotFound, InvalidDigest).

[Unreleased]: https://github.com/abyo-software/ferro-protocols/compare/ferro-blob-store-v0.0.1...HEAD
[0.0.1]: https://github.com/abyo-software/ferro-protocols/releases/tag/ferro-blob-store-v0.0.1
