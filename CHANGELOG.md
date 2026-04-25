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
  policy (DCO), security policy, and the first crate
  (`ferro-lumberjack`) extracted from the Ferro ecosystem.
- `ferro-lumberjack v0.1.0` — beta-grade client + server primitives
  with TLS in both directions. See
  [`crates/ferro-lumberjack/CHANGELOG.md`](crates/ferro-lumberjack/CHANGELOG.md).
- `ferro-airflow-dag-parser v0.0.1` — alpha static AST DAG
  extractor for Apache Airflow™ Python files. Two parser backends
  (ruff default, rustpython parity). See
  [`crates/ferro-airflow-dag-parser/CHANGELOG.md`](crates/ferro-airflow-dag-parser/CHANGELOG.md).

[Unreleased]: https://github.com/youichi-uda/ferro-protocols/commits/main
