<!-- SPDX-License-Identifier: Apache-2.0 -->
# Changelog — ferro-airflow-dag-parser

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
From `v1.0.0` onward this crate follows strict
[Semantic Versioning](https://semver.org/): breaking changes to the
public API require a major bump.

## [Unreleased]

## [1.0.1] - 2026-06-16

Security (recursion-DoS hardening). No public-API change — additive, fully
semver-compatible.

### Security
- **Closed a parser stack-overflow DoS (FP5).** Parsing attacker-controlled
  Python could overflow the vendored `littrs-ruff-python-parser` 0.6.2
  recursive-descent parser, aborting the host process with a `SIGSEGV`
  (`catch_unwind` cannot intercept a guard-page fault). The previous
  pre-screen capped only bracket nesting (32) and *consecutive* single-operator
  runs (64), so the non-bracket recursion vectors — `not`/`await` keyword
  chains, `~`/`-`/`+` runs, right-associative `a**b**c`, `a if b else …`
  conditional / `lambda:` chains, deeply nested compound statements,
  *mixed* prefix-operator chains (fuzz Finding 2, `crash-0665b68…`),
  `yield`/`yield from` chains, and `async async … def` error-recovery chains —
  slipped through and overflowed the parser. The last three were surfaced by
  the adversarial design-review (Codex DD) convergence pass and closed by
  counting `Yield`/`From`/`Async` in the lexer recursion metric.

  The fix ports FerroAir's complete three-layer recursion guard
  (`ferroair-dag-parser`, FA1) into `panic_safe.rs`: (1) an iterative bracket
  pre-scan (cap 256), (2) a single real-tokenizer pass that bounds combined
  expression recursion (`brackets + operator-run + per-line right-recursion +
  indent`, cap 1024) and rejects PEP-750 t-strings (which the parser panics
  on), and (3) execution of the parse **and** AST walk on a dedicated 128 MiB
  stack so the numeric cap — not the caller's ~2 MiB stack — is the binding
  limit. The recursive AST walkers (`collect_shift_edges`, `stringify_expr`,
  `resolve_to_task_id`, …) additionally truncate past a 1024 depth so a deep
  left-leaning `>>` / attribute / call chain that survives the parser cannot
  overflow the walk either.

  This is not a claim of bulletproof input handling — see
  `dd-pack/11-known-limitations.md` for the honest residual (a single
  left-leaning chain of hundreds of thousands of trailers in a multi-MB file
  can still overflow on recursive AST construction/drop, bounded by the
  128 MiB stack). The realistic FP5 parser-recursion shapes (each well under
  4 KiB) are fully closed, with regression tests under `tests/stack_safety.rs`
  and an adversarial design-review (Codex DD) convergence pass.

## [1.0.0] - 2026-06-08

First semver-stable release; the public API is committed under semver.
This crate statically extracts Apache Airflow DAG structure from Python
source via an AST (ruff backend) with no CPython evaluation, and is
panic-shielded against the parser. No public-API breakage versus the
`v0.x` series — the bump is a stabilization signal backed by
mutation/coverage hardening and a 6-round adversarial design-review pass.

### Changed
- API stabilized at `1.0.0` under strict semver. The `extract_static_dag`
  / `extract_all_static_dags` surface, `ExtractedDag`, the validated
  `DagId` / `TaskId` newtypes, the `#[non_exhaustive]` error enums, and
  the `dynamic_markers` detectors are now committed.

### Security
- Parser invocation remains panic-shielded. Test suite hardened to a
  ≥95% mutation kill rate and ≥85% line coverage; workspace clippy
  pedantic + nursery clean under `-D warnings` with
  `unsafe_code = forbid`; `cargo audit` / `cargo deny` clean; passed a
  6-round adversarial Codex design-review (GA gate, 0 P0/P1).

### Added
- `tests/fixtures/example_bash_operator.py` and `tests/fixtures/tutorial.py`
  — vendored verbatim from `apache/airflow/airflow/example_dags/`
  with their Apache-2.0 license headers preserved. Together they
  exercise the canonical `with DAG(...) as dag` shape, multi-key
  `default_args` literal, fan-out / fan-in `[a, b, c] >> d` list-shift
  edges, and `EmptyOperator` import paths.
- `tests/conformance.rs` — 6 conformance tests asserting `dag_id`
  recovery, full `task_id` set recovery (7 tasks for
  `example_bash_operator`, 3 for `tutorial`), `default_args` flag,
  agreement between `extract_static_dag` and `extract_all_static_dags`,
  and presence of the canonical fan-out edge `runme_0 →
  run_after_loop`. Closes the "vendor real Airflow DAGs" remark from
  the 0.0.1 → 0.1.0 promotion notes.

### Documentation
- Added a CI status badge to the README and normalised the docs.rs badge
  to the `img.shields.io/docsrs` form.
- README API stability statement upgraded from "beta" to "stable
  (`v1.x`)".

## [0.1.0] — 2026-05-04

First beta release. Promotes the crate from the `v0.0.x` alpha track
to the `v0.1.x` beta track to signal a higher level of API stability
commitment.

### Added
- Beta track. `0.1.x` semver: minor bumps may add additive items;
  removals or signature changes will be flagged in the CHANGELOG and
  released as a separate `0.2.0`.
- `examples/extract_dag.rs` — static fast-path extraction plus
  dynamic-marker detection on a small `with DAG(...)` Python source.

### Changed
- No public-API breaking changes from `0.0.1`. `IdentifierError`
  and `ParseError` carry `#[non_exhaustive]` so future variants are
  minor-bump-safe.

### Removed (pre-publish hardening)
- `parser-rustpython` feature and `rustpython_impl` module dropped
  before initial release. The transitive dependency closure of
  `rustpython-parser 0.4` pulls LGPL-3.0-only crates
  (`malachite-*`) and unmaintained Unicode crates
  (`unic-ucd-version` family), neither of which are compatible
  with the workspace's Apache-2.0-clean license profile.
  `parser-ruff` is the only backend.
- `parser_shootout` example and cross-backend `parity.rs`
  integration tests removed for the same reason.

## [0.0.1] — initial alpha

Initial extraction from FerroAir into a standalone crate.

### Added
- Static AST-based DAG extraction with two backends:
  - `parser-ruff` (default) — `littrs-ruff-python-parser` 0.6.2
    (Astral-mirrored ruff parser).
  - `parser-rustpython` — `rustpython-parser` 0.4.
- `ExtractedDag` aggregate (`dag_id`, `task_ids`, `schedule`,
  `default_args` flag, `deps_edges`, `source_span`).
- Validated `DagId` / `TaskId` newtypes — Airflow rule:
  non-empty, ≤ 250 characters, `[a-zA-Z0-9_\-\.]`.
- `ParseError` with `Parse`, `InvalidIdentifier`, `Internal`, `Io`, and
  `NoBackend` variants.
- `dynamic_markers` module — seven detectors for patterns that need
  runtime evaluation (`PathStemDagId`, `ChainSplat`, `FStringTaskId`,
  `DynamicScheduleExpr`, `UnsupportedTaskFlow`, `ImportTimeBranching`,
  `ForLoopTaskGeneration`).
- `ParseCache` — process-local mtime+size fingerprint + content-hash
  cache keyed on canonicalised path. `dashmap`-backed.
- Cross-backend parity tests when both features are enabled.
- `parser_shootout` example for empirical backend comparison.
- Fuzz target (`extract_static_dag`) covering arbitrary Python
  bytes input — checked nightly in CI.

### Notes
- All test inputs are inline source strings; no vendored Apache
  Airflow™ DAG files are shipped with the crate.
- Identifier validation uses `chars().count()` (not byte length) to
  match the upstream Python implementation.

[Unreleased]: https://github.com/abyo-software/ferro-protocols/compare/ferro-airflow-dag-parser-v1.0.0...HEAD
[1.0.0]: https://github.com/abyo-software/ferro-protocols/compare/ferro-airflow-dag-parser-v0.1.0...ferro-airflow-dag-parser-v1.0.0
[0.1.0]: https://github.com/abyo-software/ferro-protocols/releases/tag/ferro-airflow-dag-parser-v0.1.0
[0.0.1]: https://github.com/abyo-software/ferro-protocols/releases/tag/ferro-airflow-dag-parser-v0.0.1
