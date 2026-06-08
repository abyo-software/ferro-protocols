<!-- SPDX-License-Identifier: Apache-2.0 -->
# Changelog — ferro-cargo-registry-server

The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/). From
`v1.0.0` onward this crate follows strict
[Semantic Versioning](https://semver.org/): breaking changes to the
public API require a major bump.

## [Unreleased]

## [1.0.0] - 2026-06-08

First semver-stable release; the public API is committed under semver.
This release turns the Cargo Alternative Registry Protocol (RFC 2789)
primitives into a runnable server: a `ferro-cargo-registry-server`
binary, Prometheus `/metrics`, Kubernetes probes (`/live` `/healthz`
`/ready`), and **durable filesystem index persistence** that survives a
restart. It adds an explicit publish body limit, version immutability
(`409` on republish), canonical (case- and hyphen-folded) crate-name
keying, and closes a publish-rollback orphan-blob TOCTOU. Verified
end-to-end against the real `cargo` client (publish / fetch / yank /
owners), and backed by mutation/coverage hardening and a 6-round
adversarial design-review pass (GA gate, 0 P0/P1).

### Stabilization
- API stabilized at `1.0.0` under strict semver. The library surface
  (`router()` / `instrument()`, `CargoState`, the sparse-index
  `IndexEntry` / `render_lines` / `parse_lines` types, and `CargoError`)
  is now committed. Test suite hardened to a ≥95% mutation kill rate and
  ≥85% line coverage; workspace clippy pedantic + nursery clean under
  `-D warnings` with `unsafe_code = forbid`; `cargo audit` /
  `cargo deny` clean.
- Crate names are keyed canonically (case- and hyphen-folded) so
  `Foo_Bar` and `foo-bar` resolve to the same crate, matching crates.io
  collision semantics.

### Added
- **Durable index persistence (DD R2-6).** The filesystem-backed binary
  now mirrors the in-memory crate index (versions, `cksum`s, `yanked`
  flags, owners) to an `index-state.json` snapshot in the data directory.
  The snapshot is written through on every publish / yank / unyank /
  owner change and loaded on boot, so a restart re-serves every
  previously published crate — not just the tarballs the blob store
  already kept. A missing or corrupt snapshot starts the index empty
  (logged) and never blocks boot. The version → tarball digest map is
  rebuilt from each entry's `cksum`, so it is not duplicated in the
  snapshot. `CargoState::with_persistence` enables it; `CargoState::new`
  stays ephemeral for in-process / unit-test use.

### Fixed
- **Published versions are immutable (DD R2-5).** Re-publishing an
  existing `(name, version)` is now rejected with `409 Conflict`
  (`DuplicateVersion`); the original tarball and index `cksum` are left
  untouched. Only yank / unyank may mutate an existing index line.
- **Validate before writing the tarball (DD R2-8).** The publish handler
  now runs the name-collision and duplicate-version checks *before*
  storing the `.crate` blob, so a rejected publish no longer leaves an
  orphan blob on disk.
- **No wildcard origin in `config.json` (DD R2-9).** When the listen host
  is a wildcard (`0.0.0.0` / `::`) or port `0` and no explicit
  `FERRO_CARGO_REGISTRY_API` is set, the binary now refuses to boot
  rather than advertising an unfetchable `http://0.0.0.0:8081` origin.
- **`/metrics` scrapes are self-instrumented (DD R2-3).** The `/metrics`
  route is merged before the tracking middleware is layered, so a scrape
  is counted under `handler="metrics"` (the docs already claimed every
  request is recorded).

### Added (earlier)
- **Prometheus `/metrics` endpoint + request instrumentation.** New
  `metrics` module exposes a `GET /metrics` route (Prometheus text
  exposition format) and a tower/axum middleware that records, by
  `method` + matched-route `handler` + `status`, a request counter
  (`ferrocargo_http_requests_total`), a latency histogram
  (`ferrocargo_http_request_duration_seconds`), an in-flight gauge
  (`ferrocargo_in_flight`), a `ferrocargo_build_info` gauge, and index
  gauges `ferrocargo_crates_total` (distinct crate names) +
  `ferrocargo_crate_versions` (distinct published versions);
  `ferrocargo_storage_bytes` is registered but reads 0 until a
  size-reporting backend is wired. Labels use the matched route pattern,
  never raw crate names/versions, to keep cardinality bounded. Wired into
  both `instrument()` (library) and the serve binary; the chart's
  `ServiceMonitor` is now enabled by default.
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
- Added `prometheus` (0.14, Apache-2.0, `default-features = false`) for
  the `/metrics` endpoint.

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

[Unreleased]: https://github.com/abyo-software/ferro-protocols/compare/ferro-cargo-registry-server-v1.0.0...HEAD
[1.0.0]: https://github.com/abyo-software/ferro-protocols/compare/ferro-cargo-registry-server-v0.1.0...ferro-cargo-registry-server-v1.0.0
[0.1.0]: https://github.com/abyo-software/ferro-protocols/releases/tag/ferro-cargo-registry-server-v0.1.0
[0.0.1]: https://github.com/abyo-software/ferro-protocols/releases/tag/ferro-cargo-registry-server-v0.0.1
