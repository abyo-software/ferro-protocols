<!-- SPDX-License-Identifier: Apache-2.0 -->
# Roadmap

This document tracks the publication plan for the `ferro-protocols`
workspace. The full strategy is in `OSS_PUBLICATION_STRATEGY_v2.md`
in the parent project; this is the public-facing summary.

## Tier 1 — first wave

These five crates are the foundation. Each is extracted from a Ferro
ecosystem product where it has working test coverage today.

| Crate | Source product | Current version | Stability |
|---|---|---|---|
| [`ferro-blob-store`](../crates/ferro-blob-store/) | FerroRepo storage | `v0.0.3` | alpha — published 2026-04-26, foundation crate |
| [`ferro-lumberjack`](../crates/ferro-lumberjack/) | FerroBeat / FerroHeartbeat | `v0.1.0` | beta — published 2026-04-26, client + server, TLS both directions |
| [`ferro-airflow-dag-parser`](../crates/ferro-airflow-dag-parser/) | `ferro-air` | `v0.0.1` | alpha — published 2026-04-26, ruff backend, 75 tests |
| [`ferro-maven-layout`](../crates/ferro-maven-layout/) | FerroRepo Maven | `v0.0.1` | alpha — published 2026-04-26, 49 tests |
| [`ferro-cargo-registry-server`](../crates/ferro-cargo-registry-server/) | FerroRepo Cargo | `v0.0.1` | alpha — published 2026-04-26, sparse index, 38 tests |
| [`ferro-oci-server`](../crates/ferro-oci-server/) | FerroRepo OCI | `v0.0.1` | alpha — published 2026-04-26, 67 tests; conformance harness pending for `v0.1.0` |

## Tier 2 — secondary wave

Followups extracted as the corresponding Ferro product reaches the
relevant phase:

- `ferro-pep503-pep691` (FerroRepo / PyPI primitives)
- `ferro-go-module-proxy` (FerroRepo)
- `ferro-helm-chart-repo` (FerroRepo)
- `ferro-painless` (FerroStash / Elasticsearch Painless)
- `ferro-esql-parser` (FerroSearch)
- `ferro-eql-parser` (FerroSearch)
- `ferro-aql-parser` (FerroRepo / Artifactory)
- `ferro-logstash-dsl-parser` (Migration Suite)
- `ferro-keycloak-realm-import` (FerroAuth)

## Tier 4 — explicitly out of scope

To avoid duplicating well-maintained upstream crates, the workspace
will not publish:

- A Kafka wire-protocol crate — `kafka-protocol` (tychedelia) covers
  Kafka 4.1+ via codegen.
- A Cassandra CQL driver — `scylla-rust-driver` and `cdrs-tokio` are
  established.
- An MQTT client — `rumqttc` is the mainstream choice.
- A Redis RESP layer — `redis` is dominant.
- An AMQP 1.0 implementation — `fe2o3-amqp` covers it.
- An OCI distribution **client** crate — `oci-client` is sufficient
  (we publish a server-side complement, not a client duplicate).

When we have improvements to any of the above, we send a PR upstream
rather than fork.

## Versioning policy across the workspace

| Stage | Range | Breaking changes | Time in stage |
|---|---|---|---|
| Alpha | `v0.0.x` | Allowed at any release | typically 3-6 months from publication |
| Beta | `v0.1.x`–`v0.x.x` | Allowed at minor bumps; deprecation cycle starts | 6-12 months |
| Stable | `v1.x.x` | Strict semver | indefinite |

`ferro-lumberjack` enters at `v0.0.1` despite having working code in
the Ferro ecosystem because the public API surface (server-side, fluent
client builder) is still being shaped.

## What "ready to publish" means here

Before a crate is published to crates.io, it must:

1. Pass `cargo check`, `cargo clippy --all-targets -- -D warnings`,
   `cargo fmt --check`, `cargo test`, `cargo deny check`,
   `cargo audit` on a clean checkout.
2. Build on the workspace MSRV (currently rustc 1.88 — required by
   our use of `&&-let` chains and edition 2024) on Linux, macOS, and
   Windows.
3. Have at least one fuzz target if it parses untrusted input, with a
   nightly CI job exercising the corpus.
3. Have at least one criterion benchmark if it sits on a hot path.
4. Have a `README.md` that cites the relevant specification and
   declares the API stability stage.
5. Have a working `examples/` directory.
6. Reach 80%+ line coverage as measured by `cargo llvm-cov`, or
   document why a higher floor is unrealistic.

The `release.yml` workflow refuses to publish a crate that has not
passed all of the above.

## Open questions

- Whether to add a Cargo "alternative registry" mirror at our own
  domain so contributors behind restrictive networks can fetch builds
  without crates.io. Tracked separately.
- Whether to publish a workspace-level meta crate (`ferro-protocols`
  re-exports). Current plan: no — let consumers depend on the specific
  crate they need.
