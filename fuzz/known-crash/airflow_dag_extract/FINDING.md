# airflow_dag_extract — 7 crash inputs, 1 upstream bug

> **Status (2026-06-16, ferro-airflow-dag-parser v1.0.1): ✅ HANDLED.** All 7
> crash inputs here hit the upstream `expression.rs:1633` **t-/f-string panic**
> (an *unwinding* panic, not a stack overflow). They are now (a) rejected
> pre-parse by the lexer pass (PEP-750 t-strings) where applicable, and (b)
> otherwise caught by the shield's `catch_unwind` / thread-`join` and folded
> into `ParseError::Internal` — never a process abort. The separate
> **stack-overflow** recursion DoS (FP5 / fuzz Finding 2, including the
> `not`/`-`/`[` mixed-prefix `crash-0665b68…` seed) is also closed in v1.0.1
> by the ported three-layer recursion guard; see
> `dd-pack/11-known-limitations.md` §FP5 and `dd-pack/fuzz-campaign-2026-06-08.md`
> Finding 2. The OOM `*-falsepos-*` seeds in this directory remain benign
> records (libFuzzer RSS-accounting artifacts, not real OOMs).

**Date**: 2026-05-03 (extended ad-hoc fuzz wave, post-SSD-install)
**Target**: `ferro-protocols/fuzz/fuzz_targets/airflow_dag_extract.rs`
**Time-to-first-crash**: 35 seconds (cold-start with 15,530-file corpus)
**Total crashes before fuzzer stopped**: 7 distinct inputs, all same panic site

## Root cause

Upstream panic in `littrs-ruff-python-parser` 0.6.2:

```
panicked at .../littrs-ruff-python-parser-0.6.2/src/parser/expression.rs:1633:25:
internal error: entered unreachable code: t-string: unexpected token `FStringMiddle` at 183..347
```

All 7 crash inputs hit `expression.rs:1633:25 unreachable!()`. Two symmetric variants:

- `t-string: unexpected token FStringMiddle`
- `f-string: unexpected token TStringMiddle`

Token-routing bug in upstream's interpolated-string parser: tokenizer emits the
wrong "middle" token kind for the active string-parser context, the parser hits
an `unreachable!()` arm.

Smallest repro: 175B (well within attacker reach for untrusted Airflow DAG Python source).

## Impact

- **Process abort** in any service that calls `ferro_airflow_dag_parser::parse(...)`
  on attacker-controlled bytes. DoS vector.
- `panic_safe::with_bracket_cap()` (commit `060bc5d`) covers depth limits but
  does NOT wrap this entry — `catch_unwind` would be needed at the parser boundary.

## Files in this directory

- `2026-05-03-littrs-fstring-unreachable.bin` — first 708B repro (initial save)
- 7× `crash-*` — all hitting same site `expression.rs:1633:25`

## Suggested next steps (queued, not done in this wave)

1. `cargo fuzz tmin airflow_dag_extract crash-f1701ee...` for byte-minimal repro
2. Verify upstream `littrs-ruff-python-parser` HEAD for fix; if not patched, file
   issue + reproducer with project owners
3. Local mitigation: wrap `ferro-airflow-dag-parser` entry in `catch_unwind`,
   OR pin to a fixed upstream version when available
4. Add regression test under `ferro-airflow-dag-parser/tests/`
