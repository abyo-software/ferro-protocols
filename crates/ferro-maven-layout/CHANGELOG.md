<!-- SPDX-License-Identifier: Apache-2.0 -->
# Changelog — ferro-maven-layout

The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/). From
`v1.0.0` onward this crate follows strict
[Semantic Versioning](https://semver.org/): breaking changes to the
public API require a major bump.

## [Unreleased]

## [1.0.0] - 2026-06-08

First semver-stable release; the public API is committed under semver.
This crate provides Maven Repository Layout 2.0 path classification plus
POM and `maven-metadata.xml` parsing (both panic-shielded against
`quick-xml`) and an optional Axum router (`http` feature). No public-API
breakage versus the `v0.x` series — the bump is a stabilization signal
backed by the DD-hardening fixes below, mutation/coverage hardening, and
a 6-round adversarial design-review pass.

### Security
- **Explicit `PUT` request body limit on the HTTP router.** Uploads are
  now bounded by an explicit body limit rather than relying on the
  framework default, closing an unbounded-upload memory exhaustion path.
- **Closed a delete check-delete TOCTOU race.** The delete handler's
  existence check and removal were racing, allowing a window where a
  concurrent request could be mis-served; the check and delete are now
  performed without the gap.
- Test suite hardened to a ≥95% mutation kill rate and ≥85% line
  coverage; workspace clippy pedantic + nursery clean under `-D warnings`
  with `unsafe_code = forbid`; `cargo audit` / `cargo deny` clean; passed
  a 6-round adversarial Codex design-review (GA gate, 0 P0/P1).

### Changed
- API stabilized at `1.0.0` under strict semver. The `coordinate`,
  `layout`, `metadata`, `pom`, `snapshot`, `checksum`, and (under the
  default `http` feature) `handlers` / `router` surfaces, plus
  `MavenError`, are now committed.

### Added
- `tests/fixtures/` — vendored real Maven Central artefacts for
  `org.apache.commons:commons-lang3:3.14.0`: the live
  `commons-lang3-3.14.0.pom` GAV + parent block excerpt, and the
  artifact-index `maven-metadata.xml` covering the 3.0 → 3.14.0 release
  history (Apache-2.0 upstream, license header preserved).
- `tests/conformance.rs` — 4 conformance tests that parse the upstream
  POM into typed `Pom` / `PomParent` structs (asserting the apache
  `commons-parent:69` parent-pointer is recovered) and the metadata
  XML into `MavenMetadata` (asserting `<release>=3.14.0`,
  `<latest>=3.14.0`, full versions list, and `<lastUpdated>` parse +
  round-trip). Closes the v0.1.0 "vendor real-protocol fixtures" gate.

### Documentation
- Added crates.io, docs.rs, and CI status badges to the README, which is
  also the docs.rs landing page (`#![doc = include_str!("../README.md")]`).
- README API stability statement upgraded from "beta" to "stable
  (`v1.x`)".

## [0.1.0] — 2026-05-04

First beta release. Promotes the crate from the `v0.0.x` alpha track
to the `v0.1.x` beta track to signal a higher level of API stability
commitment.

### Added
- Beta track. `0.1.x` semver: minor bumps may add additive items;
  removals or signature changes will be flagged in the CHANGELOG and
  released as a separate `0.2.0`.
- `examples/parse_layout.rs` — codec-only walkthrough covering
  layout-path classification, GAV coordinates, checksum sidecar
  parsing, and SNAPSHOT timestamp composition.

### Changed
- `handlers` and `router` modules are now feature-gated on `http`
  (the default). With `--no-default-features`, the crate compiles
  to the pure-data parsing surface only — no `axum` / `tokio`
  pull-in. This is an additive de-coupling; nobody on the default
  feature set sees a change.
- `MavenError::status()` and the `IntoResponse` impl are similarly
  gated on `http`. The `MavenError` enum itself stays available
  unconditionally as a value type.
- Bumped `ferro-blob-store` dependency from `0.0` to `0.1`. Public
  surface unchanged.

### Notes
- POM parser remains "layout-validation grade" — full Maven
  inheritance / variable interpolation is `v0.2.0` scope.

## [0.0.1] — initial alpha

Initial extraction from FerroRepo's Maven protocol crate.

### Added
- `coordinate` — GAV parser with structured errors
- `layout` — `LayoutPath` typed path classification (artifact /
  metadata / sidecar)
- `metadata` — `maven-metadata.xml` types + `quick-xml` serializer
- `pom` — minimal POM parser (layout-validation grade)
- `snapshot` — SNAPSHOT timestamp + buildNumber helpers
- `checksum` — SHA-1, SHA-256 helpers; MD5 gated under `legacy-md5`
- `handlers` / `router` (default feature `http`) — Axum router for
  `GET / HEAD / PUT / DELETE` against a [`ferro_blob_store::BlobStore`]
- `MavenError` with `IntoResponse` for Axum integration

[Unreleased]: https://github.com/abyo-software/ferro-protocols/compare/ferro-maven-layout-v1.0.0...HEAD
[1.0.0]: https://github.com/abyo-software/ferro-protocols/compare/ferro-maven-layout-v0.1.0...ferro-maven-layout-v1.0.0
[0.1.0]: https://github.com/abyo-software/ferro-protocols/releases/tag/ferro-maven-layout-v0.1.0
[0.0.1]: https://github.com/abyo-software/ferro-protocols/releases/tag/ferro-maven-layout-v0.0.1
