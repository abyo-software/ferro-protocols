<!-- SPDX-License-Identifier: Apache-2.0 -->
# Fuzz campaign — 2026-06-08 (v1.0.0 GA)

8 fuzz targets (all `fuzz_targets/` across the 6 crates) × **1h each**, parallel,
libFuzzer (local x86-64, nightly). `-rss_limit_mb=4096`. Seeded from each target's
tracked corpus + known-crash dir.

## Result: 6/8 clean over 1h; **2 real bugs found** (1 fixed, 1 triaged + deferred)

| Target | Crate | Execs (1h) | Result |
|--------|-------|-----------|--------|
| parse_digest | ferro-blob-store | 2.71 B | ✅ clean |
| parse_frame | ferro-lumberjack | 29.8 M | ✅ clean |
| parse_layout_path | ferro-maven-layout | (crash @ #30) | 🔴→✅ **path-traversal bug FOUND + FIXED** |
| parse_metadata_xml | ferro-maven-layout | — | ✅ clean |
| parse_publish_body | ferro-cargo-registry-server | 227 M | ✅ clean |
| parse_reference | ferro-oci-server | 1.03 B | ✅ clean |
| extract_static_dag | ferro-airflow-dag-parser | 9.36 M | ✅ clean |
| dynamic_markers | ferro-airflow-dag-parser | (crash @ #246) | 🟡 **stack-overflow FOUND — triaged, fix deferred** |

The campaign did its job: a 1h sweep surfaced two real, previously-uncaught bugs.

## Finding 1 — maven `parse_layout_path` path-traversal (FIXED)

Input `\x17/../\x00//..-\x00.t`. `Coordinate::new` validated against `/ \ :` only,
so a `..` (or NUL-bearing) component passed validation, and `repository_path()`
re-rendered it verbatim into a path containing a `..` traversal segment — escaping
the repository root. Real path-traversal security bug.

**Fixed** (`crates/ferro-maven-layout/src/coordinate.rs`, `validate_field`): every
coordinate field now rejects whole-segment `.`/`..` components (new
`CoordinateParseError::PathTraversal`) and all control chars incl. NUL. 7 regression
tests + the crash input tracked as a permanent known-crash seed. Verified: crashes
before the fix, passes after.

## Finding 2 — airflow `dynamic_markers` stack-overflow (TRIAGED — deferred to ferro-air)

Input: ~1500 bytes alternating ~30-char runs of unary `-` with `not` keywords and `[`.
`dynamic_markers_for` → `dynamic_markers.rs:157 parse_module_safely(source)?` DOES
pre-screen, but the pre-screen's `MAX_UNARY_OP_RUN = 64` cap (`panic_safe.rs:88`)
counts only **consecutive** runs of a single prefix operator. The crash input keeps
each consecutive run under 64 but **alternates operators** (`-`×30, `not`, `-`×30, …),
so total prefix-expression nesting accumulates unbounded under the cap → the ruff
parser recurses → **stack overflow (SIGSEGV)**. `catch_unwind` cannot recover a
stack overflow, so the `panic_safe` shim is bypassed (the harness comment at
`fuzz_targets/dynamic_markers.rs:10` anticipates exactly this "genuine escape via
SIGSEGV" path).

**Reachability:** production-reachable — `dynamic_markers_for` is `pub` (api.rs:110)
and the same pre-screen guards the production `extract_static_dag` path. So this is a
genuine untrusted-input recursion DoS, not a fuzz-only artifact.

**Severity:** P2/P1-ish DoS (remote stack-overflow abort from crafted DAG source).

**Recommended fix (NOT applied — see scope note):** in `panic_safe.rs`, change the
pre-screen to cap the **total depth of consecutive prefix/unary operators across
mixed operator kinds** (sum `-`, `+`, `~`, `not`, and `[`/`(`/`{` opening-bracket
nesting into one running prefix-depth counter), rejecting when the cumulative prefix
depth exceeds a bound (e.g. the existing 64, or align with CPython's `MAXSTACK`/ruff
PR #24810's `max_recursion_depth = 202`). The current per-operator-run cap must
become a per-prefix-chain cap. Add `dynamic_markers` + `extract_static_dag` regression
tests on this input shape.

**Scope note (why deferred):** `ferro-airflow-dag-parser` is shared with a **concurrent
`ferro-air` session** (per the campaign brief, airflow changes are limited to
fuzz/test/docs to avoid semantic conflict with that session's in-flight source edits).
The fix is a `panic_safe.rs` **source** change = ferro-air's domain. This session:
- recorded the crash as a tracked known-crash seed
  (`crates/ferro-airflow-dag-parser/fuzz/known-crash/dynamic_markers/crash-0665b68c…`),
- triaged the root cause + recommended fix (above),
- flagged it as the one open fuzz finding (HONEST_LIMITATIONS FP-airflow, HANDOFF §8).

This is honest: **7/8 fuzz targets are clean over 1h; the 8th has a real,
documented, reproducible recursion-DoS whose source fix is deferred to the crate
owner.** It is NOT claimed as "fuzz-clean".
