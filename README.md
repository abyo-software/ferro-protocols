<!-- SPDX-License-Identifier: Apache-2.0 -->
# ferro-protocols

[![License](https://img.shields.io/badge/License-Apache--2.0-blue.svg)](LICENSE)
[![DCO](https://img.shields.io/badge/DCO-required-green.svg)](CONTRIBUTING.md#developer-certificate-of-origin)
[![Rust 1.80+](https://img.shields.io/badge/rust-1.88%2B-orange.svg)](rust-toolchain.toml)

Production-extracted Rust crates that implement the protocols, wire formats,
and compatibility layers underlying the **Ferro ecosystem** of Rust-native
rewrites of JVM and Python data infrastructure.

Each crate in this workspace started its life as part of a Ferro product
(FerroSearch, FerroStream, FerroBeat, FerroAir, FerroAuth, FerroRepo, …)
and was extracted into a standalone, narrowly-scoped crate so that other
projects can depend on the same battle-tested implementation.

## Why this exists

The Rust ecosystem has many high-quality protocol *clients* (Kafka, OCI
distribution, Cassandra CQL, MQTT, …) but is missing primitives in several
load-bearing areas:

- **Server-side OCI Distribution** — Rust has clients (`oci-client`,
  `oci-spec`) but no public server crate.
- **Cargo Alternative Registry server** — the protocol is documented but
  not surfaced as a reusable library.
- **Maven Repository Layout 2.0** — no Rust implementation at all.
- **Logstash Lumberjack v2** — no Rust implementation at all.
- **Static-only Apache Airflow™ DAG parsing** — completely absent in any
  language outside Airflow itself.
- **Painless / ES|QL / EQL / AQL** parsers and various PyPI / Helm /
  Go-module registry primitives — partial or absent.

These gaps showed up while building the Ferro products. Rather than keep
the implementations private, we publish them here on the same terms as
the rest of the Rust ecosystem (Apache-2.0).

## Status

> ⚠️ **This workspace is in early publication.** Most crates are alpha
> (`v0.0.x`). API stability is documented per crate. See each crate's
> `README.md` for its current status and roadmap.

| Crate | Version | Extracted from | Status |
|---|---|---|---|
| [`ferro-lumberjack`](crates/ferro-lumberjack/README.md) | `v0.1.0` | `ferro-beat` / `ferro-heartbeat` | beta — client + server, TLS both directions |
| [`ferro-airflow-dag-parser`](crates/ferro-airflow-dag-parser/README.md) | `v0.0.1` | `ferro-air` | alpha — static AST DAG extraction with two backends |

Additional Tier-1 crates planned for early publication: `ferro-airflow-dag-parser`,
`ferro-cargo-registry-server`, `ferro-maven-layout`, `ferro-oci-server`. See
[`docs/roadmap.md`](docs/roadmap.md) for the full schedule and Tier-2 crates.

## Workspace layout

```
ferro-protocols/
├── Cargo.toml             — workspace manifest
├── deny.toml              — license / advisory / source restrictions
├── rust-toolchain.toml    — pinned toolchain (1.91.1)
├── rustfmt.toml
├── crates/
│   └── ferro-lumberjack/  — Logstash Lumberjack v2 protocol primitives
│       ├── src/
│       ├── tests/
│       ├── benches/
│       ├── examples/
│       └── fuzz/
└── .github/workflows/
    ├── ci.yml             — check / clippy / fmt / test / coverage / audit / deny
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
