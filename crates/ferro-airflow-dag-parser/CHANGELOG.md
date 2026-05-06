<!-- SPDX-License-Identifier: Apache-2.0 -->
# Changelog — ferro-airflow-dag-parser

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
This crate is on the `v0.1.x` beta track; additive changes only
between minor releases. Breaking changes will be released as a
separate `v0.2.0`.

## [Unreleased]

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

[Unreleased]: https://github.com/abyo-software/ferro-protocols/compare/ferro-airflow-dag-parser-v0.1.0...HEAD
[0.1.0]: https://github.com/abyo-software/ferro-protocols/releases/tag/ferro-airflow-dag-parser-v0.1.0
[0.0.1]: https://github.com/abyo-software/ferro-protocols/releases/tag/ferro-airflow-dag-parser-v0.0.1
