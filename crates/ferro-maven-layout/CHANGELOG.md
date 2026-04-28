<!-- SPDX-License-Identifier: Apache-2.0 -->
# Changelog — ferro-maven-layout

The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/). Currently
in the `v0.0.x` alpha series; breaking changes allowed between any
two releases until `v0.1.0`.

## [Unreleased]

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

[Unreleased]: https://github.com/abyo-software/ferro-protocols/compare/ferro-maven-layout-v0.0.1...HEAD
[0.0.1]: https://github.com/abyo-software/ferro-protocols/releases/tag/ferro-maven-layout-v0.0.1
