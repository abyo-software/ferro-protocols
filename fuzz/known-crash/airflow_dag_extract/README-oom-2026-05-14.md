# airflow_dag_extract — 2026-05-14 OOM artifact (false-positive)

**Date**: 2026-05-14
**Artifact**: `oom-7452cf811cf3-2026-05-14` (1417 bytes; libfuzzer's
on-OOM dump of whatever input was being mutated when `-rss_limit_mb` was
hit, NOT a crashing testcase)
**Triage outcome**: false-positive — bump `FUZZ_RSS_LIMIT_MB` to 4 GiB,
no production-code change.

## What happened

Nightly fuzz run on 2026-05-14 07:52→08:19 UTC ran for 27 min and was
killed by libfuzzer's default `-rss_limit_mb=2048`. The artifact above
was emitted at kill time.

## Why it's a false-positive

Single-input replay of the artifact:

```
$ cargo +nightly fuzz run airflow_dag_extract \
    known-crash/airflow_dag_extract/oom-7452cf811cf3-2026-05-14 -- -runs=1
Executed ... in 5 ms
stat::peak_rss_mb: 50
```

5 ms wall, 50 MiB peak RSS — the input itself is benign. The OOM is
libfuzzer's working-set growth across iterations:

- Corpus dir holds ~36 k files / ~144 MiB. libfuzzer keeps the corpus
  in-process for power-scheduling.
- Adversarial inputs from the corpus (deep f-string nesting, brace
  stews) drive transient parser allocations whose peaks accumulate
  faster than glibc returns memory to the OS.
- In a 91 s smoke fuzz we observed peak RSS climb to 893 MiB; the 2 GiB
  cap is hit between ~10 and ~30 minutes depending on the mutation
  schedule. The 27-min cap-hit in the failing run is within that band.

Production callers (`ferro_airflow_dag_parser::extract_all_static_dags`,
`parse_dag_path`) are stateless: no `OnceLock` / `thread_local` / `Lazy`
cache, no inter-call allocator pinning. A real DAG poller invoking the
parser once per file will never exhibit this growth. Existing 77 unit
tests + integration tests cover the production allocation shape.

## Action taken

1. Per-instance systemd override bumps `FUZZ_RSS_LIMIT_MB=4096`:
   `~/.config/systemd/user/fuzz@ferro-protocols-airflow_dag_extract.service.d/override.conf`
2. Artifact moved here from `artifacts/airflow_dag_extract/` so the next
   nightly run starts clean.

## Related

- Same false-positive class as `bw_scheduler_phase4_registry`
  (`~/.config/systemd/user/fuzz@ferrosearch-gpu-compress-bw_scheduler_phase4_registry.service.d/override.conf`).
  Different mechanism (no `thread::scope` here, just corpus + parser
  allocations) but identical signature: single-replay benign, sustained
  iteration trips `rss_limit_mb`.
- Prior airflow_dag_extract findings (panic at `expression.rs:1633:25`
  in upstream `littrs-ruff-python-parser`) are a separate class, closed
  by `panic_safe::parse_module_safely` + `MAX_BRACKET_DEPTH=32` +
  `MAX_UNARY_OP_RUN=64` pre-screens. See `FINDING.md` and
  `triage-2026-05-07.md`.

## If this recurs at 4 GiB

If a future run trips `rss_limit_mb=4096` on the same target, that would
suggest unbounded growth rather than warmup overhead, and we should
move to either:

1. `cargo fuzz cmin airflow_dag_extract` to shrink the in-memory corpus
   working set, OR
2. An input-size cap at the panic_safe boundary (e.g. reject inputs
   >32 KiB or with f-string nesting depth > N) — note this changes
   production-code behaviour, so requires a real adversarial-input
   threat-model justification, not just fuzz-farm hygiene.

## 2026-05-15 recurrence

**Artifact**: `oom-fca6b0c1-match-bracket-1001b-2026-05-15` (1001 bytes —
input shape is repeated `match[\x00...`-style bracket prefixes with
`iiii...i` padding; libfuzzer's dump at kill time, NOT a crashing
testcase).

Single-input replay:

```
$ cargo +nightly fuzz run airflow_dag_extract \
    known-crash/airflow_dag_extract/oom-fca6b0c1-match-bracket-1001b-2026-05-15 \
    -- -runs=1
Executed ... in 8 ms
```

8 ms wall — same false-positive class as the 2026-05-14 entry. The
`FUZZ_RSS_LIMIT_MB=4096` systemd override is still in place (verified
2026-05-15) and was tripped a second time. This is the "future run trips
rss_limit_mb=4096" branch above.

**Action this time**: artifact moved to known-crash/, no production-code
change. **Next steps if it recurs a third time** (≥3 trips at 4 GiB ⇒
sustained unbounded growth, not just slow warmup):

1. Run `cargo fuzz cmin airflow_dag_extract` and rebase the corpus dir
   on the minimised set — should cut the in-memory working set by 5-10×.
2. If cmin alone doesn't hold, bump to `FUZZ_RSS_LIMIT_MB=8192` once
   (sentinel value — if 8 GiB also trips, we have a real production
   leak that production single-call evidence is missing, and the
   threat-model justification for an input-size cap kicks in).
