<!-- SPDX-License-Identifier: Apache-2.0 -->
# Changelog

All notable changes to this workspace are documented here. Per-crate
changelogs live alongside each crate (`crates/<name>/CHANGELOG.md`).

The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and this
workspace adheres to [Semantic Versioning](https://semver.org/) on a
per-crate basis. The `v0.0.x` series of any crate explicitly allows
breaking API changes between releases; that contract becomes strict at
`v0.1.0`.

## [Unreleased]

### Added
- Workspace bootstrapped: licensing, CI scaffolding, contribution
  policy (DCO), security policy, and the first crates extracted
  from the Ferro ecosystem.
- `ferro-blob-store v0.0.3` — content-addressed `BlobStore` trait +
  in-memory + filesystem backends; foundation for the OCI / Maven /
  Cargo crates below.
- `ferro-lumberjack v0.1.0` — beta-grade client + server primitives
  with TLS in both directions. See
  [`crates/ferro-lumberjack/CHANGELOG.md`](crates/ferro-lumberjack/CHANGELOG.md).
- `ferro-airflow-dag-parser v0.0.1` — alpha static AST DAG
  extractor for Apache Airflow™ Python files. Ruff backend + seven
  dynamic-fallback markers. See
  [`crates/ferro-airflow-dag-parser/CHANGELOG.md`](crates/ferro-airflow-dag-parser/CHANGELOG.md).
- `ferro-maven-layout v0.0.1` — Maven Repository Layout 2.0
  primitives + Axum HTTP router. See
  [`crates/ferro-maven-layout/CHANGELOG.md`](crates/ferro-maven-layout/CHANGELOG.md).
- `ferro-cargo-registry-server v0.0.1` — Cargo Alternative
  Registry sparse-index server primitives. See
  [`crates/ferro-cargo-registry-server/CHANGELOG.md`](crates/ferro-cargo-registry-server/CHANGELOG.md).
- `ferro-oci-server v0.0.1` — OCI Distribution v1.1 server-side
  primitives. See
  [`crates/ferro-oci-server/CHANGELOG.md`](crates/ferro-oci-server/CHANGELOG.md).

[Unreleased]: https://github.com/abyo-software/ferro-protocols/commits/main
