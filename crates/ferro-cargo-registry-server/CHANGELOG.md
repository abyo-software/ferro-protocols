<!-- SPDX-License-Identifier: Apache-2.0 -->
# Changelog — ferro-cargo-registry-server

The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/). The crate
is on the `v0.1.x` beta track; additive changes only between minor
releases. Breaking changes will be released as a separate `v0.2.0`.

## [Unreleased]

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
