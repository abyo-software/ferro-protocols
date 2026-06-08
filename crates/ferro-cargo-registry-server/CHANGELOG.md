<!-- SPDX-License-Identifier: Apache-2.0 -->
# Changelog — ferro-cargo-registry-server

The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/). The crate
is on the `v0.1.x` beta track; additive changes only between minor
releases. Breaking changes will be released as a separate `v0.2.0`.

## [Unreleased]

### Added
- **Runnable server binary** `ferro-cargo-registry-server`
  (`src/bin/ferro-cargo-registry-server.rs`): boots the Axum `router()`
  over a filesystem `FsBlobStore`, configurable via
  `FERRO_CARGO_REGISTRY_DATA` / `FERRO_CARGO_REGISTRY_LISTEN` /
  `FERRO_CARGO_REGISTRY_API`. Adds Kubernetes-style probes `/live`,
  `/ready`, `/healthz` (`{"status":"ok"}`) and graceful `SIGTERM` /
  Ctrl-C shutdown via `axum::serve(...).with_graceful_shutdown(...)`.
- **Real-`cargo` end-to-end test** `tests/cargo_e2e.rs`: spins the
  binary on an ephemeral port and drives the actual `cargo` client
  through publish → sparse-index fetch → tarball download → yank →
  unyank, asserting the index lines, plus an owners `GET`. Skips
  cleanly (does not fail) when `cargo`/`curl` are unavailable. See
  `tests/e2e-results.md` for the honest verified-path matrix
  (publish/fetch/yank/unyank via real cargo; owners via HTTP-level).
- `examples/serve_registry.rs` — minimal embeddable-server example
  mirroring the binary in standalone form.
- Root-relative sparse-index routes (`/{prefix}/{name}` and
  `/{p0}/{p1}/{name}`) so a stock `cargo` configured with
  `index = "sparse+http://host/"` resolves index line files without an
  `/index/` URL prefix (the legacy `/index/{*path}` route is retained).
- `tests/fixtures/` — vendored real crates.io sparse-index lines for
  `serde` (1.0.0 + 1.0.219) and `anyhow` (1.0.0 + 1.0.99), plus the
  live `https://index.crates.io/config.json` shape.
- `tests/conformance.rs` — 5 conformance tests that parse the upstream
  index lines, validate dev-dep / `package`-rename / `features2`
  preservation, and round-trip the lines through `render_lines` →
  `parse_lines` to assert canonical-shape stability. Closes the v0.1.0
  "vendor real-protocol fixtures" gate.

### Changed
- `IndexConfig::new` now renders the `config.json` `dl` download
  template as an **absolute URL** rooted at the configured API host
  (was a server-relative path), so a stock `cargo fetch` downloads
  tarballs without a host-resolution error. Callers passing a real
  origin (e.g. `http://127.0.0.1:8081`) need no change; an empty host
  degrades to the prior relative path.
- Crate now inherits the workspace `clippy` pedantic + nursery lint
  set (previously locally `allow`-ed); the source was brought clean
  under `-D warnings`.

### Dependencies
- Added `tracing-subscriber` (binary logging) and enabled the
  `rt-multi-thread` / `macros` / `net` / `signal` tokio features needed
  by the binary.

## [0.1.0] — 2026-05-04

First beta release. Promotes the crate from the `v0.0.x` alpha track
to the `v0.1.x` beta track to signal a higher level of API stability
commitment.

### Added
- Beta track. `0.1.x` semver: minor bumps may add additive items;
  removals or signature changes will be flagged in the CHANGELOG and
  released as a separate `0.2.0`.
- `examples/sparse_index_round_trip.rs` — end-to-end demonstration
  exercising `IndexEntry`, `render_lines`, `parse_lines`, and
  `index_path`.

### Changed
- Bumped `ferro-blob-store` dependency from `0.0` to `0.1`. The crate
  consumes the blob store through trait objects only (`Arc<dyn
  BlobStore>`), so callers see no API change.

### Notes
- Sparse-index protocol coverage still matches the spec; planned
  `v0.2` work is the auth-pluggable middleware integration and a
  pluggable index-storage backend.

## [0.0.1] — initial alpha

Initial extraction from FerroRepo's Cargo protocol crate.

### Added
- `config` — `/config.json` response shape (`IndexConfig`)
- `index` — sparse-index `IndexEntry` / `IndexDep` plus parse / render
  helpers
- `name` — canonical crate-name validation per spec
- `publish` — length-prefixed publish-request body parser
- `version` — semver validation
- `owners` — owners API request/response types
- `yank` — yank/unyank response
- `handlers` / `router` — Axum router for `/config.json`,
  `/index/{*path}`, `/api/v1/crates/**`
- `CargoError` with `IntoResponse` for Axum integration; renders
  the spec's `{ "errors": [{ "detail": "..." }] }` envelope

### Notes
- Sparse index only. Git index returns 501 (`NotImplemented`).
- Auth is open in this crate — layer your own middleware.

[Unreleased]: https://github.com/abyo-software/ferro-protocols/compare/ferro-cargo-registry-server-v0.1.0...HEAD
[0.1.0]: https://github.com/abyo-software/ferro-protocols/releases/tag/ferro-cargo-registry-server-v0.1.0
[0.0.1]: https://github.com/abyo-software/ferro-protocols/releases/tag/ferro-cargo-registry-server-v0.0.1
