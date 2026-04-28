<!-- SPDX-License-Identifier: Apache-2.0 -->
# Changelog — ferro-oci-server

The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/). Currently
in the `v0.0.x` alpha series; breaking changes allowed between any
two releases until `v0.1.0`.

## [Unreleased]

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

[Unreleased]: https://github.com/abyo-software/ferro-protocols/compare/ferro-oci-server-v0.0.1...HEAD
[0.0.1]: https://github.com/abyo-software/ferro-protocols/releases/tag/ferro-oci-server-v0.0.1
