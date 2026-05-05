<!-- SPDX-License-Identifier: Apache-2.0 -->
# Changelog — ferro-oci-server

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
- `examples/parse_reference.rs` — codec-only walkthrough covering
  repository name validation, reference (tag / digest) parsing,
  manifest media-type classification, and an `ImageManifest`
  round-trip.

### Changed
- Bumped `ferro-blob-store` dependency from `0.0.3` to `0.1`. The
  `Digest` type's serde wire form is unchanged (`<algo>:<hex>`).

### Notes
- The `v0.1.0` "formal conformance harness vendoring" gate (see
  `v0.0.1` notes) lands as a separate `0.1.x` minor.

## [0.0.1] — initial alpha

Initial extraction from FerroRepo's OCI protocol crate.

### Added
- `error` — `OciError` + `OciErrorCode` rendering the spec's
  `{ "errors": [...] }` envelope
- `manifest` — `ImageManifest`, `ImageIndex`, `Descriptor` serde types
- `media_types` — Docker v2 / OCI v1 media-type classification
- `reference` — image-name + tag + digest parsing with spec-compliant
  validation (max length, no `..`, no `//`, lowercase rule)
- `registry` — `RegistryMeta` trait (manifest + tag + upload +
  referrer plane); `InMemoryRegistryMeta` reference impl
- `upload` — `UploadState` + `ContentRange` parsing
- `router` / `handlers` — Axum router for `/v2/**` covering base
  version check, catalog, tag listing, blob CRUD, blob upload
  state machine, manifest CRUD, referrers
- 67 tests pass (42 unit + 3 + 22 integration smoke). Smoke tests
  exercise full request walks; formal upstream conformance harness
  vendoring is the `v0.1.0` gate.

### Notes
- Persistent metadata backend (SQLite / Postgres) deferred to
  `v0.0.x` follow-ups; the trait is stable enough that you can
  implement your own today.

[Unreleased]: https://github.com/abyo-software/ferro-protocols/compare/ferro-oci-server-v0.1.0...HEAD
[0.1.0]: https://github.com/abyo-software/ferro-protocols/releases/tag/ferro-oci-server-v0.1.0
[0.0.1]: https://github.com/abyo-software/ferro-protocols/releases/tag/ferro-oci-server-v0.0.1
