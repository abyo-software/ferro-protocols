# Disclosure timeline — littrs-ruff-python-parser 0.6.2 panic

## TL;DR

The bug we found in `littrs-ruff-python-parser-0.6.2` (`expression.rs:1633:25
unreachable!()`) is **fixed upstream in astral-sh/ruff**. littrs vendored
ruff at commit `56eb6b62` on 2026-02-07, **5 days before** the fix landed
in ruff main. The fix is to **re-vendor at `6ded4bed` or later** in the
next littrs release.

No new disclosure to ruff is needed — it was reported and fixed in
February 2026 (issue #23198 / PR #23232).

## Timeline (all dates UTC)

| Date | Event | Source |
|------|-------|--------|
| 2026-02-07 | littrs vendor SHA `56eb6b62` (released as `littrs-ruff-python-parser` 0.6.2) | `chonkie-inc/littrs` Cargo.toml `# Vendored ruff crates (from github.com/astral-sh/ruff, commit 56eb6b62)` |
| 2026-02-10 | ruff issue #23198 reported: `Panic: entered unreachable code: t-string: unexpected token FStringMiddle` | https://github.com/astral-sh/ruff/issues/23198 |
| 2026-02-11 | First fix attempt `4c22ad76` "Fix f-string middle panic when parsing t-strings" | astral-sh/ruff |
| 2026-02-12 | Final fix landed `6ded4bed` (PR #23232), issue closed as `completed` | astral-sh/ruff |
| 2026-05-03 | Bug rediscovered via fuzz of our `airflow_dag_extract` target (first crash in 35 sec, 7 distinct inputs collected, all hitting same panic site) | this repo |

## What littrs needs to do

1. Re-vendor `vendor/ruff_python_parser` at commit ≥ `6ded4bed` (any
   ruff main from 2026-02-12 onward).
2. Bump `littrs-ruff-python-parser` version on crates.io.
3. Optional: add a regression test exercising t-string with FStringMiddle
   token (the upstream PR #23232 includes one we can mirror).

## What we (ferrosearch / ferro-protocols) need to do

1. Wait for next littrs release.
2. Bump `Cargo.lock` to pull in the fix.
3. Verify the corpus inputs at `fuzz/known-crash/airflow_dag_extract/` no
   longer panic the binary; if so, move them into `fuzz/corpus/` as
   permanent regression seeds.
4. **Defense-in-depth (recommended regardless)**: wrap `ferro-airflow-dag-parser`
   entry points in `panic::catch_unwind`. The upstream `ruff_python_parser`
   has many `unreachable!()` arms in `expression.rs` (we counted 14+ in
   astral-sh/ruff main); future similar bugs are likely. A `catch_unwind`
   boundary turns process-abort into a recoverable error.

## Crash inputs in this directory

7 distinct inputs (175B-708B), all hitting `expression.rs:1633:25` upstream
(`expression.rs:1646:25` in current astral-sh/ruff main, after line shift):

- `crash-f1701ee9fdfe2139cada2e70fe7a9ad829435cd2` (175B, smallest)
- `crash-b2ba4d6bf95915404ac31f4a83e11a32dd5874be` (199B, f-string variant)
- `crash-4dc14fedabc0174274bc6a3a46b7f0002a4c1a53` (222B)
- `crash-22a32acc97fe6d827cfcf7000d15472194dfc8d9`
- `crash-95c97080d71f4a0729f70e80b8bee0925ba9fcf7`
- `crash-bd72f9ccbb2de252b9adca4078b9f1087f764f13`
- `crash-ed5f25a43a56343e36eee9ffa86fd838f3ee9aa4` (708B, original cold-start hit)
- `2026-05-03-littrs-fstring-unreachable.bin` (snapshot of original 708B)
