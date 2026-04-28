<!-- SPDX-License-Identifier: Apache-2.0 -->
# Building Ferro: rewriting the JVM and Python data stack in Rust as a solo developer

> Status: **draft** — pending publication on a personal blog plus
> Hacker News (Show HN), `r/rust`, `r/dataengineering`, `dev.to`,
> Zenn (Japanese version), and Mastodon. This file is the canonical
> source-of-truth for the post; tweak here, then mirror.

---

I've spent the last several months writing Rust. The output is what
I'm going to call "Ferro": a set of Rust-native rewrites of the
JVM and Python data infrastructure most teams take for granted —
search, streaming, orchestration, identity, package registries,
log shipping, monitoring. There are seven product repositories now,
on the order of 800,000 lines of own-code Rust, around 18,000 tests.
I am a single developer.

This post is about *why* and *how*. It is also the public-launch
post for [`ferro-protocols`][repo] — the first public-facing piece
of that work. `ferro-protocols` is a workspace of small, narrowly
scoped crates carved out of the larger ecosystem and published
under Apache-2.0, so the parts of the work that have value
independent of the products themselves can ship to crates.io and
be used by anyone.

[repo]: https://github.com/abyo-software/ferro-protocols

## Why rewrite all of it

Most of the JVM data stack — Elasticsearch, Kafka, Logstash,
Metricbeat, Filebeat — was designed in an era where a JVM-resident
service with a 4 GB heap and a 60-second GC pause window was the
acceptable cost of doing business. The Python data stack — Airflow,
the SDK ecosystem around it — was designed when "spawn a CPython
worker per task" was a reasonable scheduling primitive.

It is no longer the right answer for most installations. Cold-start
matters. Memory matters. P99 latency matters. Container density
matters. The Datadog-shaped vendors are pricing on agent footprint
because that's where the squeeze is. A 30 MB resident-set, single-binary
Rust replacement of Filebeat ships on every container in a fleet
without making the bill irrational; a 200 MB JVM does not.

Rust has been ready for this rewrite for a couple of years now. The
async story is solid (Tokio is mature, rustls is mature, openraft
exists, sqlx works). The ecosystem has stabilised on permissive
licensing. `cargo` makes producing a static binary trivial. What's
been missing is *people doing the work*.

I started doing the work.

## What's in the ecosystem today

Concretely, by individual product:

| Product | Compat target | Status as of 2026-04-25 |
|---|---|---|
| **FerroSearch** | Elasticsearch 9.3 / OpenSearch 2.19 + 3.6 / OpenSearch Dashboards 3.6 | YAML REST 100% on ES, 100% on OS 2.19, 100% on OS 3.6, 100% on OSD 3.6 api_integration, plus 92% on Kibana 9.3 in Docker mode. Single Rust engine. |
| **FerroStream** | Kafka 4.2 wire | Brokers, KRaft, Tiered Storage, Share Groups (KIP-932), Streams API surface, OMB benchmarking harness. Performance has caught up with Redpanda on consume in the synthetic suites; the more-complex multi-topic OMB workloads are still being root-caused (more on that below). |
| **FerroFlow** | TPC-H + Kafka SQL connectors | TPC-H 22/22, ~275 compat tests, six real connectors (sink/source/TLS/SASL/S3/GCS/partition/LSM). |
| **FerroAuth** | Keycloak 26 IdP wire | Runnable IdP with WebAuthn L3, mTLS, LDAP sync, HA stores. Passkey-issuance live smoke pass. |
| **FerroRepo** | OCI Distribution v1.1 + Cargo + Maven + PyPI + Helm + Go modules + APT/YUM, with Sigstore + SLSA + TUF | v0.1.0 GA tag local, 1391 tests pass, OCI conformance suite green. |
| **FerroAir** | Apache Airflow 3 | Phase 0 done — PyO3 + airflow.sdk integration proven, AIP-72 stub, 100% parser parity on a 64-DAG sample. |
| **FerroBeat / FerroHeartbeat / FerroMetric / FerroStash** | Filebeat / Heartbeat / Metricbeat / Logstash | All running. FerroMetric reaches a Linux 9.5 MB RSS at 50K events/s (compared with ~31 MB for FerroBeat and a multi-hundred-MB JVM baseline for the originals). |

This is "the largest single-developer rewrite of a major data
infrastructure category I can find," but I want to be careful not
to over-claim. Plenty of those numbers come from synthetic test
suites; benchmarks against production-shaped workloads are an
ongoing, partially-finished exercise (the OMB hang I mentioned
above is a real, current bug — a non-daemon
`ScheduledThreadPoolExecutor` in the OMB Java client side that
prevents JVM exit, which is fixed in the harness but the run on
real EC2 with the fix has not happened yet).

## The shape of the work

Each Ferro product is a Rust workspace of dozens of crates. Most of
them are private monorepos. They share a few common rules:

- `forbid(unsafe_code)` workspace-wide. Exceptions need a `SAFETY:`
  comment and live under feature flags (`py-embed` in FerroAir is the
  one current case, for the PyO3 boundary).
- `cargo clippy --all-targets -- -D warnings` with pedantic
  elevated to `warn`. No exceptions.
- `cargo deny check` clean. License allowlist (Apache-2.0, MIT, BSD
  family, ISC, MPL-2.0, etc.). `openssl`, `openssl-sys`, `native-tls`
  blocked — we use rustls everywhere.
- 80%+ line coverage target. Some products are well above
  (FerroBeat 87.6%, FerroMetric 86.4%); the cap is set by what's
  measurable in a realistic CI tier.
- Zero `unwrap` on user-controlled paths. Zero `TODO` /
  `FIXME` / `HACK` in source.

These rules are non-negotiable in any of the seven repos. The
result is that when I extract a module into a public crate (more
on that in a moment), I do not have to spend time tightening it up
to OSS quality — it already meets that bar.

## Diligence: I run myself through synthetic DD rounds

The work is not just code. Every Ferro product has a `due-diligence/`
directory with round-by-round findings logs from synthetic
investor-style audits. I run these through Codex CLI (with GPT-5
class models) and Anthropic Claude in parallel — same brief, two
adversaries — and merge the findings. They go through identification
("R1 surfaced 54 actionable findings"), then remediation rounds
("R2 closed 38 of those, surfaced 16 new ones, …") until the
remediation backlog stabilises.

For perspective: FerroMetric reached **R14 Clean Pass** with 0
actionable findings remaining. FerroAuth is at R14 with 4 medium /
3 low / 1 info open. The findings logs are commit-pinned, so
"this round happened" is auditable, and the fixes are commit-pinned,
so "this finding was closed by *this commit*" is auditable.

This is the part I want potential acquirers and partners to look
at — not a list of features. The features are easy to claim. What's
hard is showing the work *behind* them.

## The first public release: ferro-protocols

The publicly-published part of all of this starts with
[`ferro-protocols`][repo] — a mono-repo workspace whose individual
crates are extracted from the products above. The first two crates
in the repo are:

### `ferro-lumberjack` (v0.1.0)

The Logstash Lumberjack v2 wire protocol. Filebeat and Heartbeat
agents speak it. Logstash receives it. Until now, no Rust crate
implemented either side: extending `rust-elasticsearch` or
`rumqttc` with Lumberjack support was the only option, and they
do not.

`ferro-lumberjack` is the **frame codec, async client, and async
server** for Lumberjack v2, with TLS in both directions via
rustls. The frame codec is pure-data (no I/O, usable from any
runtime); the client + server are Tokio-only. There are 66 tests
including 6 real-socket end-to-end client↔server tests with
self-signed TLS.

It is at `v0.1.0` because the implementation is extracted from
production use in FerroBeat and FerroHeartbeat — not new code.
The server side is fresh, but exercised by real client↔server
e2e tests.

### `ferro-airflow-dag-parser` (v0.0.1)

A static AST-based extractor for Apache Airflow™ Python DAG
files. Recovers `dag_id`, `task_ids`, dependencies, schedule, and
seven categories of dynamic-fallback markers — without running
the source.

This is a primitive that, as far as I can tell, **does not exist
elsewhere in any language**. Apache Airflow's reference scheduler
imports every `dags/*.py` through CPython on every poll cycle,
which is fine for small fleets and a heavy tax on big ones.
FerroAir uses this crate as a fast-path that handles the static
fraction of DAGs in microseconds, and only routes to CPython the
ones whose structure depends on runtime state. The crate is in
alpha (`v0.0.x`) because the public API surface is still
shaping; the implementation has 75 tests and runs under both a
ruff backend and a rustpython backend with cross-backend parity
checks.

## The roadmap (yes, it includes "all of the above")

`ferro-protocols` is a mono-repo on purpose. As each product
matures past the "hardening behind closed doors" stage, the parts
that have value as standalone crates will surface here. Already
planned for the next several weeks, in rough order:

- `ferro-cargo-registry-server` — Cargo Alternative Registry server-side
  (sparse + git index; first public Rust crate to do this).
- `ferro-maven-layout` — Maven Repository Layout 2.0 (no Rust
  implementation exists yet).
- `ferro-oci-server` — OCI Distribution Specification v1.1
  server-side primitives (clients exist; servers do not).
- `ferro-painless` — Elasticsearch Painless lexer/parser/JIT
  (extracted from FerroStash).
- `ferro-esql-parser` — ES|QL parser (the Elasticsearch query
  language; absent from the Rust ecosystem).
- `ferro-aql-parser` — Artifactory AQL.
- `ferro-pep503-pep691` — PyPI primitives.
- `ferro-go-module-proxy` — Go module proxy server-side.
- `ferro-helm-chart-repo` — Helm 3 chart repository primitives.
- `ferro-keycloak-realm-import` — Keycloak realm JSON 26.x.
- `ferro-logstash-dsl-parser` — Logstash `.conf` DSL.

All Apache-2.0. All extracted from production use (or to be
production-tested in their parent product first). All published
under a Developer Certificate of Origin contributor flow — no CLA
— so contributing is friction-free.

## What I am not claiming

A tactical list of things that are still open:

1. **Track A M&A is not closed.** FerroStream and adjacent products
   are intended for acquisition; that is the Track A line. It is
   not yet at the closing stage.
2. **OMB benchmarks against real cloud are not yet public.**
   Synthetic benches show parity-or-better; the real EC2 13-hour
   workload run with the harness fixes is queued, not done.
3. **FerroAir Phase 1 is not done.** Phase 0 is a 4-pillar PoC
   with 100% parser parity over a 64-DAG sample. The full Airflow
   3.x compat surface is Phase 1 work.
4. **Marketplace listings (FerroAuth Pro, FerroRepo Pro, FerroAir
   Pro) are pending company formation.** They are intentionally
   gated on the Track A close — premature commercial launch under
   personal-name billing is the wrong move.
5. **I am not going to claim this is all "production-ready" for
   your environment.** It is production-tested in mine. Yours
   may be different.

The diligence rounds, the test counts, and the per-product
README files are the load-bearing artefacts. Look at those, not
at marketing copy.

## How to actually use it

```toml
[dependencies]
ferro-lumberjack = "0.1"
ferro-airflow-dag-parser = "0.0"
```

The `ferro-protocols` repo has the canonical READMEs, contribution
flow (DCO), security policy, and the workflow files used by CI.
Issues + PRs welcome on the repo. Discussions are a better channel
for design questions.

## Why I'm publishing now

A single-developer ecosystem rewrite of this scale is unusual
enough that the natural skeptical question is **"how is this real?"**
The answer is to put the parts that *can* be published into the
open, on terms (Apache-2.0, DCO, on crates.io with semver) that
make them easy to inspect, depend on, and contribute to. Acquirers
will look at the repo. Hiring managers will look at the repo. Other
Rust-data-infrastructure people will look at the repo. The way to
make the work falsifiable is to ship.

If you are working on Airflow internals and need a static DAG
parser, or you operate Logstash and need a Beats sender or
receiver in Rust, you can install both crates today.

The rest of the ecosystem will surface here as it matures.

— Y.U.

---

*Discussion: HN comments / `r/rust` / `r/dataengineering` / dev.to
/ Zenn — see the post header for canonical links.*
