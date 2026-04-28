<!-- SPDX-License-Identifier: Apache-2.0 -->
# ferro-airflow-dag-parser

[![License](https://img.shields.io/badge/License-Apache--2.0-blue.svg)](../../LICENSE)
[![crates.io](https://img.shields.io/crates/v/ferro-airflow-dag-parser.svg)](https://crates.io/crates/ferro-airflow-dag-parser)
[![docs.rs](https://docs.rs/ferro-airflow-dag-parser/badge.svg)](https://docs.rs/ferro-airflow-dag-parser)
[![Rust 1.88+](https://img.shields.io/badge/rust-1.88%2B-orange.svg)](../../rust-toolchain.toml)

Static, AST-based extractor for **Apache Airflow™** Python DAG files.
Recovers `dag_id`, `task_ids`, task dependencies, schedule, and a
catalogue of "this can't be resolved statically" markers — without
running the source.

> Apache Airflow's own performance docs tell users to "minimize
> top-level code" because every poll cycle of the DAG processor
> imports every `dags/*.py` file through CPython, and import-time
> work blocks the scheduler. The advice is a workaround for a
> structural problem: the reference scheduler has no way to read a
> DAG file's *structure* without evaluating the *file*.
> `ferro-airflow-dag-parser` answers the question that workaround
> leaves hanging: *what if the structural read didn't need
> CPython at all?*

> ⚠️ **Alpha (`v0.0.1`).** API will change between `0.0.x` releases.
> The implementation is pulled from production use in the Ferro
> ecosystem; it is the static fast-path that orchestrators use to
> skip `CPython` evaluation when a DAG file's structure can be
> determined by looking at the Python AST alone.

Part of the **Ferro ecosystem**. Extracted from production use in
[FerroAir](https://github.com/abyo-software/ferro-air) (an Airflow-3-compatible orchestrator written in Rust).

## What this saves you

```python
# Reference Airflow path: spawn / reuse CPython, import the file,
# walk DagBag, pickle the DAG into the scheduler's metadata DB.
# Cost: ~50–200 ms per file at startup, repeated every poll cycle,
# multiplied by however many DAG files you have.
```

```text
// ferro-airflow-dag-parser path: parse to AST, walk it once.
// No Python interpreter, no DagBag, no pickle.
let dag = ferro_airflow_dag_parser::extract_static_dag(src)?;
//   ↑ microseconds per file, on the static fraction.
```

The parser also tells you *which* DAGs **can't** take the static
path — the seven [dynamic-fallback markers](#what-this-crate-does)
each map to a specific Python idiom that requires runtime
evaluation. An orchestrator routes those (and only those) to a
`CPython` embed; everything else stays in Rust.

We have not yet published a head-to-head benchmark against the
upstream `DagBag` import path, so the speedup figure above is a
back-of-the-envelope claim from comparing component costs (Python
import vs Rust AST walk on equivalent inputs). Production figures
will land alongside the [`FerroAir`](https://github.com/abyo-software/ferro-air) performance report.

## What this crate does

Apache Airflow's reference scheduler imports every `dags/*.py` file
through `CPython` on every poll cycle so it can read the resulting
`DAG` objects. That works, but it pays the full cost of evaluating
every import-time expression — including the ones that have no side
effects relevant to the scheduler.

This crate parses the Python source with the
[`ruff_python_parser`][ruff] (vendored as
[`littrs-ruff-python-parser`][ruff-mirror]) and walks the AST to
recover the same information statically:

- DAG ID — from `with DAG(dag_id="…")` or `@dag def fn():`.
- Task IDs — from every `task_id="…"` operator kwarg and every
  `@task`-decorated function name, deduplicated, source-order
  preserved.
- Schedule — `schedule="@daily"` / `schedule_interval="…"` /
  `timetable=…`. Best-effort stringified for non-string literals.
- Dependency edges — `>>` / `<<` / `set_upstream` / `set_downstream`.
- `default_args={…}` presence flag.
- Source span (1-indexed inclusive lines) for error messages and
  jump-to-DAG features.

It also detects seven **dynamic-fallback markers** that say "static
analysis is incomplete; if you need full fidelity, route this DAG
through `CPython`":

1. `dag_id=Path(__file__).stem` (or any non-literal expression)
2. `chain(*list)` / `cross_downstream(*list)` splat
3. `task_id=f"task_{i}"` (f-string task IDs in a loop)
4. `schedule=Asset("…")` / `schedule=Timetable()` (non-literal schedule)
5. `@task(expand=…)` / `@task(partial=…)` / dynamic taskflow decorators
6. `if X: with DAG(...)` (import-time conditional DAG)
7. `for x in …: PythonOperator(...)` (operator construction in a loop)

[ruff]: https://github.com/astral-sh/ruff
[ruff-mirror]: https://crates.io/crates/littrs-ruff-python-parser

## What this crate does **not** do

- **Run the file.** This is a static analyzer, not an executor. If a
  DAG's structure depends on runtime state, this crate will surface
  a dynamic-fallback marker rather than try to evaluate the
  expression.
- **Validate semantics.** Recovered identifiers are validated against
  Airflow's identifier rule (1–250 chars, `[a-zA-Z0-9_\-\.]`).
  The crate does not check whether `task_ids` match operator
  contracts, whether DAG runs would succeed, or whether the schedule
  is sane.
- **Mirror the full upstream `DagBag` API.** Where `airflow.models.DagBag`
  reports import errors, plugin lookups, dag-pickle round-trips, and
  more, this crate reports `Result<ExtractedDag, ParseError>`. The
  call site decides what to do with parse failures.

## Quick start

```rust
use ferro_airflow_dag_parser::{extract_static_dag, dynamic_markers_for};

let src = r#"
from airflow import DAG
from airflow.operators.bash import BashOperator

with DAG(dag_id="hello", schedule="@daily"):
    a = BashOperator(task_id="a", bash_command="echo a")
    b = BashOperator(task_id="b", bash_command="echo b")
    a >> b
"#;

let dag = extract_static_dag(src).unwrap();
assert_eq!(dag.dag_id.as_ref().map(|d| d.as_str()), Some("hello"));
assert_eq!(dag.task_ids.len(), 2);
assert_eq!(dag.schedule.as_deref(), Some("@daily"));
assert!(dag.deps_edges.iter().any(|(u, d)| u.as_str() == "a" && d.as_str() == "b"));
assert!(dynamic_markers_for(src).is_empty());
```

## Cache (for filesystem-watching consumers)

If you are a DAG-folder watcher (the typical use case), construct
a process-local [`ParseCache`] and call `get_or_parse(path)` instead
of re-parsing on every poll:

```rust,no_run
use std::path::Path;
use ferro_airflow_dag_parser::ParseCache;

let cache = ParseCache::new();
let outcome = cache.get_or_parse(Path::new("dags/hello.py")).unwrap();
println!("{} DAG(s), source_hash = {:#x}", outcome.dags.len(), outcome.source_hash);
```

The cache uses a stat-only fast path (mtime + size fingerprint) before
falling back to re-hashing the file contents, matching the behaviour of
Airflow's reference DAG processor.

## Backend

The crate uses [`ruff_python_parser`][ruff] (vendored as the
[`littrs-ruff-python-parser`][ruff-mirror] crates.io mirror, pinned
for reproducibility). It is the only backend, gated behind the
`parser-ruff` feature which is on by default. Set
`default-features = false` if you only need the codec-free types
in [`common`] and [`line_index`].

A second `rustpython-parser` backend was used as a parity-checking
companion during the originating Ferro `PoC` and removed before
publication: its transitive dependency closure pulls
LGPL-3.0-only crates (the `malachite-*` family) and unmaintained
Unicode crates, neither of which are appropriate for this
workspace's Apache-2.0-clean license profile.

## Status

| Aspect | Status |
|---|---|
| API stability | **alpha** (`v0.0.x` — breaking changes allowed at any release) |
| Use in production | Yes, in [FerroAir](https://github.com/abyo-software/ferro-air) |
| MSRV | rustc **1.88** |
| Coverage target | 80%+ line; current measured in CI |
| Async runtime | None (synchronous; the `ParseCache` uses `dashmap` for thread safety) |
| Test fixtures | Inline source strings only — no vendored Apache Airflow™ DAGs |

## Used in production by

- [**FerroAir**](https://github.com/abyo-software/ferro-air) — Apache
  Airflow-3-compatible orchestrator written in Rust. The static
  fast-path uses this crate (private at time of writing; will switch
  to `ferro-airflow-dag-parser` once published).

## Compatibility note

Apache Airflow™ is a registered trademark of the Apache Software
Foundation. This crate implements a static analyzer compatible with
the Airflow DAG Python API; it is not endorsed by, or affiliated
with, the ASF.

The identifier-validation rule (1–250 characters drawn from
`[a-zA-Z0-9_\-\.]`) is taken from Airflow's reference implementation
(`airflow.models.dag.DAG_ID_RE_VALID_CHARS`).

## Triage policy

See the workspace [`CONTRIBUTING.md`](../../CONTRIBUTING.md). In
short: security 48h, bugs (with a reproducer) 14 days best-effort,
features collected for the next minor.

## License

Apache-2.0. See [`LICENSE`](../../LICENSE).
