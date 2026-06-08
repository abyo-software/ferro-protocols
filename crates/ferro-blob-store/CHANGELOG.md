<!-- SPDX-License-Identifier: Apache-2.0 -->
# Changelog — ferro-blob-store

The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/). From
`v1.0.0` onward this crate follows strict
[Semantic Versioning](https://semver.org/): breaking changes to the
public API require a major bump.

## [Unreleased]

## [1.0.0] - 2026-06-08

First semver-stable release; the public API is committed under semver.
This is the foundation content-addressed blob store for the workspace
(in-memory and filesystem backends). No public-API breakage versus the
`v0.x` series — the bump is a deliberate stabilization signal backed by
mutation/coverage hardening and a 6-round adversarial design-review pass.

### Changed
- API stabilized at `1.0.0`. The `BlobStore` trait, `Digest`,
  `InMemoryBlobStore`, and `FsBlobStore` surfaces are now under a strict
  semver contract; the surface is intentionally minimal so streaming
  variants can be added additively in a future minor.

### Security
- Test suite hardened to a ≥95% mutation kill rate and ≥85% line
  coverage; workspace clippy pedantic + nursery clean under `-D warnings`
  with `unsafe_code = forbid`; `cargo audit` / `cargo deny` clean. Passed
  a 6-round adversarial Codex design-review (GA gate, 0 P0/P1).

### Documentation
- Added crates.io, docs.rs, and CI status badges to the README, which is
  also the docs.rs landing page (`#![doc = include_str!("../README.md")]`).
- README API stability statement upgraded from "beta" to "stable
  (`v1.x`)".

## [0.1.0] — 2026-05-04

First beta release. The crate has been stable since `v0.0.1`; this
bump promotes it from the `v0.0.x` alpha track to the `v0.1.x` beta
track to signal a higher level of API stability commitment.

### Added
- Beta track. `0.1.x` semver: minor bumps may add additive items;
  removals or signature changes will be flagged in the CHANGELOG and
  released as a separate `0.2.0`.
- `examples/in_memory_round_trip.rs` — end-to-end demonstration
  exercising every public method of [`BlobStore`].

### Changed
- README: API stability statement upgraded from "alpha" to "beta".
- No code changes from `0.0.3`. The public surface is unchanged.

### Notes
- Streaming (`put_stream` / `get_stream`) and a paginated `list`
  variant remain the named `0.2.0` deliverable.

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

[Unreleased]: https://github.com/abyo-software/ferro-protocols/compare/ferro-blob-store-v1.0.0...HEAD
[1.0.0]: https://github.com/abyo-software/ferro-protocols/compare/ferro-blob-store-v0.1.0...ferro-blob-store-v1.0.0
[0.1.0]: https://github.com/abyo-software/ferro-protocols/releases/tag/ferro-blob-store-v0.1.0
[0.0.3]: https://github.com/abyo-software/ferro-protocols/releases/tag/ferro-blob-store-v0.0.3
[0.0.2]: https://github.com/abyo-software/ferro-protocols/releases/tag/ferro-blob-store-v0.0.2
[0.0.1]: https://github.com/abyo-software/ferro-protocols/releases/tag/ferro-blob-store-v0.0.1
