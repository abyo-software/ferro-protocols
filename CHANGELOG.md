<!-- SPDX-License-Identifier: Apache-2.0 -->
# Changelog

All notable changes to this workspace are documented here. This file is a
roll-up index; the authoritative, fine-grained history lives in the
per-crate changelogs (`crates/<name>/CHANGELOG.md`).

The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and each crate
adheres to [Semantic Versioning](https://semver.org/) **independently**.
Crates are versioned and released per-crate (the release workflow is
driven by a per-crate tag prefix), so there is no single workspace
version number. From each crate's `v1.0.0` onward its public API is a
strict semver contract.

## Current published versions

| Crate | Version | Status |
|---|---|---|
| `ferro-blob-store` | `1.0.0` | stable |
| `ferro-lumberjack` | `1.0.0` | stable |
| `ferro-airflow-dag-parser` | `1.0.1` | stable (latest; security patch over `1.0.0`) |
| `ferro-maven-layout` | `1.0.0` | stable |
| `ferro-cargo-registry-server` | `1.0.0` | stable |
| `ferro-oci-server` | `1.0.0` | stable |

The workspace `Cargo.toml` carries `[workspace.package].version = "1.0.0"`
as a non-inherited baseline default (every member crate pins its own
`version`); the table above is authoritative.

## [Unreleased]

No workspace-level changes pending. See the per-crate `[Unreleased]`
sections for in-flight work.

## 2026-06-16 — `ferro-airflow-dag-parser` `1.0.1` (security patch)

- **`ferro-airflow-dag-parser v1.0.1`** — closed a parser stack-overflow
  DoS (FP5): attacker-controlled Python DAG source could overflow the
  vendored recursive-descent parser and `SIGSEGV` the host process
  (`catch_unwind` cannot intercept a guard-page fault). Fix ports a
  three-layer recursion guard (bracket pre-scan, lexer recursion cap,
  dedicated 128 MiB parse/walk stack) plus an AST-walker depth cap.
  Additive, fully semver-compatible — no public-API change. Honest
  residual documented in `dd-pack/11-known-limitations.md`. See
  [`crates/ferro-airflow-dag-parser/CHANGELOG.md`](crates/ferro-airflow-dag-parser/CHANGELOG.md).

## 2026-06-08 — first semver-stable GA (`v1.0.0`) for all six crates

All six crates reached their first semver-stable release. Each shipped
clippy pedantic + nursery clean under `-D warnings` with
`unsafe_code = forbid`, `cargo audit` / `cargo deny` clean, hardened test
suites (≥95% mutation kill rate and ≥85% line coverage as the GA gate),
and a 6-round adversarial design-review (Codex DD) pass (0 P0/P1).

- **`ferro-blob-store v1.0.0`** — content-addressed `BlobStore` trait +
  in-memory and filesystem backends stabilized.
- **`ferro-lumberjack v1.0.0`** — Lumberjack/Beats v2 codec + client +
  server + TLS stabilized; closed an unbounded per-window memory
  accumulation DoS; landed upstream-wire conformance fixtures.
- **`ferro-airflow-dag-parser v1.0.0`** — panic-shielded static AST DAG
  extractor stabilized; vendored real Apache Airflow DAG fixtures.
- **`ferro-maven-layout v1.0.0`** — Maven Layout 2.0 + POM/metadata
  parsing + optional Axum router stabilized; explicit PUT body limit +
  delete TOCTOU fix.
- **`ferro-cargo-registry-server v1.0.0`** — Cargo Alternative Registry
  sparse-index server made runnable (binary, `/metrics`, K8s probes,
  durable filesystem index); verified end-to-end against the real
  `cargo` client.
- **`ferro-oci-server v1.0.0`** — OCI Distribution v1.1 server made
  runnable (binary, `/metrics`, K8s probes, durable metadata); passes
  the official `opencontainers/distribution-spec` v1.1 conformance suite
  (75 passed / 0 failed / 5 skipped).

See each crate's `CHANGELOG.md` for the per-crate fix detail.

## 2026-05-04 — beta promotion (`v0.1.0` / `v0.2.0`)

Five crates moved from the `v0.0.x` alpha track to the `v0.1.x` beta
track; `ferro-lumberjack` advanced to `v0.2.0`. The promotion signalled a
stronger API-stability commitment ahead of the `v1.0.0` GA.

## 2026-04-26 — initial public launch

Six crates published to crates.io in a single batch as initial alpha
versions, alongside the workspace scaffolding: Apache-2.0 licensing, CI,
DCO contribution policy, and security policy. The launch story is at
[Building Ferro](https://ferro.abyo.net/blog/building-ferro/).

[Unreleased]: https://github.com/abyo-software/ferro-protocols/commits/main
