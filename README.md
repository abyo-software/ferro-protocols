<!-- SPDX-License-Identifier: Apache-2.0 -->
# ferro-protocols

[![CI](https://github.com/abyo-software/ferro-protocols/actions/workflows/ci.yml/badge.svg)](https://github.com/abyo-software/ferro-protocols/actions/workflows/ci.yml)
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

### 2026-06-16 — `ferro-airflow-dag-parser` `v1.0.1` (security patch)

A semver-compatible security patch (no public-API change). Parsing
attacker-controlled Python DAG source could overflow the vendored
recursive-descent parser and abort the host process with a `SIGSEGV`
(a guard-page fault that `catch_unwind` cannot intercept). `v1.0.1`
ports a three-layer recursion guard — an iterative bracket pre-scan, a
single real-tokenizer pass that bounds combined expression recursion,
and execution of the parse **and** AST walk on a dedicated 128 MiB stack
— plus a depth cap on the recursive AST walkers. The realistic
parser-recursion shapes are fully closed; one honest residual (a
multi-MB single left-leaning trailer chain bounded by the 128 MiB stack)
is documented in [`dd-pack/11-known-limitations.md`](dd-pack/11-known-limitations.md).
The other five crates remain at `v1.0.0`.

### 2026-06-08 — All six crates reach `v1.0.0` (stable, semver-committed)

The workspace shipped its first semver-stable GA. Every crate is now
under a strict semver contract, clippy pedantic + nursery clean under
`-D warnings` with `unsafe_code = forbid`, `cargo audit` / `cargo deny`
clean, ≥95% mutation kill rate, ≥85% line coverage, and passed a 6-round
adversarial design-review (0 P0/P1).

| Crate | Version | v1.0.0 highlight |
|---|---|---|
| [`ferro-blob-store`](https://crates.io/crates/ferro-blob-store) | `v1.0.0` stable | API stabilized; mutation/DD-hardened foundation blob store |
| [`ferro-lumberjack`](https://crates.io/crates/ferro-lumberjack) | `v1.0.0` stable | API stabilized; configurable per-window memory cap closes an unbounded-accumulation DoS |
| [`ferro-airflow-dag-parser`](https://crates.io/crates/ferro-airflow-dag-parser) | `v1.0.0` stable (now `v1.0.1`) | API stabilized; panic-shielded static AST extractor (recursion-DoS hardening landed in `v1.0.1`, see above) |
| [`ferro-maven-layout`](https://crates.io/crates/ferro-maven-layout) | `v1.0.0` stable | API stabilized; explicit PUT body limit + delete TOCTOU fix |
| [`ferro-cargo-registry-server`](https://crates.io/crates/ferro-cargo-registry-server) | `v1.0.0` stable | runnable binary + `/metrics` + K8s probes + durable filesystem index; real-`cargo` verified |
| [`ferro-oci-server`](https://crates.io/crates/ferro-oci-server) | `v1.0.0` stable | runnable binary + `/metrics` + K8s probes + durable metadata; **official OCI conformance suite 75/75** |

### 2026-05-04 — All crates promoted to `v0.1.0` beta

Five of the six crates moved from the `v0.0.x` alpha track to the
`v0.1.x` beta track, signalling a higher level of API stability
commitment (`additive-only between minors`). `ferro-lumberjack`
remains at `v0.2.0` stable.

| Crate | Version | Highlight |
|---|---|---|
| [`ferro-blob-store`](https://crates.io/crates/ferro-blob-store) | `v0.1.0` beta | foundation: 5-method async `BlobStore` trait + in-memory + filesystem backends |
| [`ferro-lumberjack`](https://crates.io/crates/ferro-lumberjack) | `v0.2.0` stable | Logstash Lumberjack v2 — frame codec + async client + async server + TLS |
| [`ferro-airflow-dag-parser`](https://crates.io/crates/ferro-airflow-dag-parser) | `v0.1.0` beta | static AST extraction of Apache Airflow™ DAG files (no `CPython`) |
| [`ferro-maven-layout`](https://crates.io/crates/ferro-maven-layout) | `v0.1.0` beta | Maven Repository Layout 2.0 + Axum router |
| [`ferro-cargo-registry-server`](https://crates.io/crates/ferro-cargo-registry-server) | `v0.1.0` beta | embeddable Cargo Alternative Registry sparse-index server primitives |
| [`ferro-oci-server`](https://crates.io/crates/ferro-oci-server) | `v0.1.0` beta | embeddable OCI Distribution v1.1 server primitives |

### 2026-04-26 — Initial public launch

Six crates published to crates.io in a single batch (initial alpha
versions). The full launch story is at
[Building Ferro](https://ferro.abyo.net/blog/building-ferro/).

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

> 🟢 **All six crates are stable at `v1.0.0`+ (semver-committed).**
> Five are at `v1.0.0`; `ferro-airflow-dag-parser` is at `v1.0.1` (a
> security patch). Each crate is versioned and released independently —
> there is no single workspace version number. From each crate's
> `v1.0.0` onward its public API is a strict semver contract: breaking
> changes require a major bump; minor releases are additive (and may
> `#[deprecate]`, but not remove). See each crate's `README.md` for its
> current status and roadmap.

The six crates split into **four libraries** (data types, codecs, and
traits you embed) and **two server-primitive crates** (HTTP request
handlers you mount behind your own Axum/Tower stack):

**Libraries**

| Crate | docs.rs | Version | Extracted from | Status |
|---|---|---|---|---|
| [`ferro-blob-store`](crates/ferro-blob-store/README.md) [![crates.io](https://img.shields.io/crates/v/ferro-blob-store.svg)](https://crates.io/crates/ferro-blob-store) | [![docs.rs](https://img.shields.io/docsrs/ferro-blob-store)](https://docs.rs/ferro-blob-store) | `v1.0.0` | FerroRepo storage | stable — content-addressed `BlobStore` trait + in-memory + filesystem backends; foundation for OCI / Maven / Cargo crates below |
| [`ferro-lumberjack`](crates/ferro-lumberjack/README.md) [![crates.io](https://img.shields.io/crates/v/ferro-lumberjack.svg)](https://crates.io/crates/ferro-lumberjack) | [![docs.rs](https://img.shields.io/docsrs/ferro-lumberjack)](https://docs.rs/ferro-lumberjack) | `v1.0.0` | `ferro-beat` / `ferro-heartbeat` | stable — Logstash Lumberjack v2 codec + client + server + TLS |
| [`ferro-airflow-dag-parser`](crates/ferro-airflow-dag-parser/README.md) [![crates.io](https://img.shields.io/crates/v/ferro-airflow-dag-parser.svg)](https://crates.io/crates/ferro-airflow-dag-parser) | [![docs.rs](https://img.shields.io/docsrs/ferro-airflow-dag-parser)](https://docs.rs/ferro-airflow-dag-parser) | `v1.0.1` | `ferro-air` | stable — static AST DAG extraction (ruff backend, 7 dynamic-fallback markers); recursion-DoS hardened |
| [`ferro-maven-layout`](crates/ferro-maven-layout/README.md) [![crates.io](https://img.shields.io/crates/v/ferro-maven-layout.svg)](https://crates.io/crates/ferro-maven-layout) | [![docs.rs](https://img.shields.io/docsrs/ferro-maven-layout)](https://docs.rs/ferro-maven-layout) | `v1.0.0` | FerroRepo Maven | stable — Maven Repository Layout 2.0 + Axum router (`http` feature) |

**Server primitives**

| Crate | docs.rs | Version | Extracted from | Status |
|---|---|---|---|---|
| [`ferro-cargo-registry-server`](crates/ferro-cargo-registry-server/README.md) [![crates.io](https://img.shields.io/crates/v/ferro-cargo-registry-server.svg)](https://crates.io/crates/ferro-cargo-registry-server) | [![docs.rs](https://img.shields.io/docsrs/ferro-cargo-registry-server)](https://docs.rs/ferro-cargo-registry-server) | `v1.0.0` | FerroRepo Cargo | stable — Cargo Alternative Registry sparse-index server + binary + `/metrics` + K8s probes + durable index (real-`cargo` verified) |
| [`ferro-oci-server`](crates/ferro-oci-server/README.md) [![crates.io](https://img.shields.io/crates/v/ferro-oci-server.svg)](https://crates.io/crates/ferro-oci-server) | [![docs.rs](https://img.shields.io/docsrs/ferro-oci-server)](https://docs.rs/ferro-oci-server) | `v1.0.0` | FerroRepo OCI | stable — OCI Distribution v1.1 server + binary + durable metadata; official conformance suite 75/75 |

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

As of 2026-06-18, `cargo test --workspace` runs **735 tests** (unit +
integration + doctests) with 0 failures. The MSRV is Rust **1.88**
(declared via `rust-version`); the pinned development toolchain in
`rust-toolchain.toml` is **1.91.1**. The OCI server additionally has an
out-of-band conformance harness (`crates/ferro-oci-server/tests/conformance/`)
that drives the official `opencontainers/distribution-spec` v1.1 suite
against the running binary — latest recorded run **75 passed / 0 failed /
5 skipped** (see [`RESULTS.md`](crates/ferro-oci-server/tests/conformance/RESULTS.md)).

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
