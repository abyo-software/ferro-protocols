<!-- SPDX-License-Identifier: Apache-2.0 -->
# Changelog — ferro-cargo-registry-server

The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/). Currently
in the `v0.0.x` alpha series; breaking changes allowed between any
two releases until `v0.1.0`.

## [Unreleased]

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

[Unreleased]: https://github.com/abyo-software/ferro-protocols/compare/ferro-cargo-registry-server-v0.0.1...HEAD
[0.0.1]: https://github.com/abyo-software/ferro-protocols/releases/tag/ferro-cargo-registry-server-v0.0.1
