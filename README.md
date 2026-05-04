<!-- SPDX-License-Identifier: Apache-2.0 -->
# ferro-protocols

[![License](https://img.shields.io/badge/License-Apache--2.0-blue.svg)](LICENSE)
[![DCO](https://img.shields.io/badge/DCO-required-green.svg)](CONTRIBUTING.md#developer-certificate-of-origin)
[![Rust 1.88+](https://img.shields.io/badge/rust-1.88%2B-orange.svg)](rust-toolchain.toml)

<!-- Star history badge — add to this README only after the workspace
     reaches ~50 stars. An empty graph is worse than no graph; the
     social-proof loop only kicks in once there is something to show. -->

**The protocol layer Rust was missing.** Six small crates that
implement the wire formats and server-side primitives behind the
JVM and Python data infrastructure stack — extracted from
production use in the Ferro ecosystem.

```bash
cargo add ferro-blob-store              # content-addressed BlobStore + InMem + Fs backends
cargo add ferro-lumberjack              # Logstash Lumberjack v2 (Beats) — codec + client + server + TLS
cargo add ferro-airflow-dag-parser      # Apache Airflow DAG static AST extractor (no CPython)
cargo add ferro-maven-layout            # Maven Repository Layout 2.0 + Axum router
cargo add ferro-cargo-registry-server   # Cargo Alternative Registry sparse-index server primitives
cargo add ferro-oci-server              # OCI Distribution v1.1 server primitives
```

## What's new

### 2026-04-26 — Initial public launch

Six crates published to crates.io in a single batch:

| Crate | Version | Highlight |
|---|---|---|
| [`ferro-blob-store`](https://crates.io/crates/ferro-blob-store) | `v0.0.3` | foundation: 5-method async `BlobStore` trait + in-memory + filesystem backends |
| [`ferro-lumberjack`](https://crates.io/crates/ferro-lumberjack) | `v0.2.0` | Logstash Lumberjack v2 — frame codec + async client + async server + TLS |
| [`ferro-airflow-dag-parser`](https://crates.io/crates/ferro-airflow-dag-parser) | `v0.0.1` | static AST extraction of Apache Airflow™ DAG files (no CPython) |
| [`ferro-maven-layout`](https://crates.io/crates/ferro-maven-layout) | `v0.0.1` | Maven Repository Layout 2.0 + Axum router |
| [`ferro-cargo-registry-server`](https://crates.io/crates/ferro-cargo-registry-server) | `v0.0.1` | embeddable Cargo Alternative Registry sparse-index server primitives |
| [`ferro-oci-server`](https://crates.io/crates/ferro-oci-server) | `v0.0.1` | embeddable OCI Distribution v1.1 server primitives |

The full launch story is at [Building Ferro](https://ferro.abyo.net/blog/building-ferro/).

## Why these six together

Three of the crates (`ferro-oci-server`, `ferro-maven-layout`,
`ferro-cargo-registry-server`) sit on the same storage abstraction:
the [`BlobStore`](https://docs.rs/ferro-blob-store) trait. Pick a
storage backend once, then mount any combination of registries on
top of it. That dependency shape is what justifies the workspace:

```text
ferro-blob-store  ←──┬── ferro-oci-server         (OCI Distribution v1.1)
                     ├── ferro-maven-layout       (Maven Layout 2.0 + HTTP)
                     └── ferro-cargo-registry-server  (Cargo Alt Registry)

ferro-lumberjack             (no shared deps — standalone codec + client + server)
ferro-airflow-dag-parser     (no shared deps — standalone static parser)
```

Each crate in this workspace started its life as part of a Ferro
product (FerroSearch, FerroStream, FerroBeat, FerroAir, FerroAuth,
FerroRepo, …) and was extracted into a narrowly-scoped, standalone
release.

## Why this exists

The Rust ecosystem has many high-quality protocol *clients* (Kafka, OCI
distribution, Cassandra CQL, MQTT, …) but is missing primitives in
several load-bearing areas:

- **Server-side OCI Distribution** — Rust has clients (`oci-client`,
  `oci-spec`) but no embeddable server primitives. The dominant open
  registries (Harbor, zot, distribution/distribution) are all Go.
- **Cargo Alternative Registry server primitives** — RFC 2141 was
  accepted in 2018, but the only widely-known full server
  ([`alexandrie`](https://github.com/Hirevo/alexandrie)) is a
  standalone application, not a library you embed.
- **Maven Repository Layout 2.0** — no Rust implementation at all.
- **Logstash Lumberjack v2** — no Rust implementation at all.
- **Static-only Apache Airflow™ DAG parsing** — completely absent in
  any language outside Airflow itself.
- **Painless / ES|QL / EQL / AQL** parsers and various PyPI / Helm /
  Go-module registry primitives — partial or absent.

These gaps showed up while building the Ferro products. Rather than
keep the implementations private, we publish them here on the same
terms as the rest of the Rust ecosystem (Apache-2.0).

## Status

> ⚠️ **This workspace is in early publication.** Most crates are alpha
> (`v0.0.x`). API stability is documented per crate. See each crate's
> `README.md` for its current status and roadmap.

| Crate | Version | Extracted from | Status |
|---|---|---|---|
| [`ferro-blob-store`](crates/ferro-blob-store/README.md) | `v0.0.3` | FerroRepo storage | alpha — content-addressed `BlobStore` trait + in-memory + filesystem backends; foundation for OCI / Maven / Cargo crates below |
| [`ferro-lumberjack`](crates/ferro-lumberjack/README.md) | `v0.2.0` | `ferro-beat` / `ferro-heartbeat` | stable — Logstash Lumberjack v2 codec + client + server + TLS (semver from `0.2.0`) |
| [`ferro-airflow-dag-parser`](crates/ferro-airflow-dag-parser/README.md) | `v0.0.1` | `ferro-air` | alpha — static AST DAG extraction (ruff backend, 7 dynamic-fallback markers) |
| [`ferro-maven-layout`](crates/ferro-maven-layout/README.md) | `v0.0.1` | FerroRepo Maven | alpha — Maven Repository Layout 2.0 + Axum router |
| [`ferro-cargo-registry-server`](crates/ferro-cargo-registry-server/README.md) | `v0.0.1` | FerroRepo Cargo | alpha — Cargo Alternative Registry sparse-index server |
| [`ferro-oci-server`](crates/ferro-oci-server/README.md) | `v0.0.1` | FerroRepo OCI | alpha — OCI Distribution v1.1 server primitives |

See [`docs/roadmap.md`](docs/roadmap.md) for Tier-2 follow-ups
(`ferro-pep503-pep691`, `ferro-go-module-proxy`, `ferro-helm-chart-repo`,
`ferro-painless`, `ferro-esql-parser`, `ferro-eql-parser`,
`ferro-aql-parser`, `ferro-logstash-dsl-parser`, `ferro-keycloak-realm-import`).

## Workspace layout

```
ferro-protocols/
├── Cargo.toml             — workspace manifest
├── deny.toml              — license / advisory / source restrictions
├── rust-toolchain.toml    — pinned toolchain (1.91.1)
├── rustfmt.toml
├── crates/
│   ├── ferro-blob-store/              — async BlobStore trait + InMem + Fs backends
│   ├── ferro-lumberjack/              — Logstash Lumberjack v2 codec + client + server + TLS
│   ├── ferro-airflow-dag-parser/      — Apache Airflow DAG static AST extractor
│   ├── ferro-maven-layout/            — Maven Repository Layout 2.0 + Axum router
│   ├── ferro-cargo-registry-server/   — Cargo Alternative Registry sparse-index server
│   └── ferro-oci-server/              — OCI Distribution v1.1 server primitives
└── .github/workflows/
    ├── ci.yml             — check / clippy / fmt / test / coverage / audit / deny / docs
    ├── dco.yml            — DCO sign-off check on every PR
    ├── fuzz-nightly.yml   — nightly fuzzing for parsers
    ├── coverage.yml       — uploads cobertura artefact
    └── release.yml        — tag-prefix-driven crates.io publish
```

## Building locally

```bash
# pinned toolchain auto-installs via rust-toolchain.toml
cargo check --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
cargo test --workspace
cargo deny check
```

## Contributing

We accept contributions under the
[Developer Certificate of Origin (DCO)](CONTRIBUTING.md#developer-certificate-of-origin) —
add `Signed-off-by: Your Name <email>` to every commit (`git commit -s`).
There is no CLA. See [`CONTRIBUTING.md`](CONTRIBUTING.md).

Issue triage policy, response targets, and what each crate does **not**
support are documented per crate.

## Security

If you believe you have found a security issue, please **do not** open a
public issue. Instead follow the process in [`SECURITY.md`](SECURITY.md).

## License

Apache License, Version 2.0. See [`LICENSE`](LICENSE) and [`NOTICE`](NOTICE).

Trademarks remain the property of their respective owners; the use of any
third-party protocol or product name in this repository is purely
descriptive of compatibility.
