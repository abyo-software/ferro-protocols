<!-- SPDX-License-Identifier: Apache-2.0 -->
# ferro-airflow-dag-parser — known limitations

## FP5 — parser stack-overflow DoS — ✅ CLOSED (v1.0.1, 2026-06-16)

**Was:** parsing attacker-controlled Python DAG source could overflow the
vendored `littrs-ruff-python-parser` 0.6.2 recursive-descent parser and abort
the host process with a `SIGSEGV` (`catch_unwind` cannot intercept a guard-page
fault). The original pre-screen capped only bracket nesting (32) and a
*consecutive run of a single* prefix operator (64), so these vectors slipped
through and overflowed the parser:

- `not not not …` / `await await …` keyword prefix chains,
- `~~~…` / `---…` / `+++…` byte prefix runs,
- right-associative `a ** b ** c …`,
- `a if b else …` conditional and `lambda: lambda: …` chains,
- deeply nested compound statements (`if a:` / `for …:` …, statement-level
  recursion),
- **mixed** prefix-operator chains (`~not ~not …`) where each individual
  consecutive run stayed under 64 but the combined nesting was unbounded —
  the exact shape of fuzz Finding 2, `crash-0665b68…`,
- `yield from yield from … x` / `yield yield … x` (parser recurses per
  `yield` keyword — Codex DD R2),
- `async async … def f(): pass` (the parser's error recovery for a stray
  `async` recurses into `parse_statement` — Codex DD R3).

**Fix (v1.0.1):** ported FerroAir's complete three-layer recursion guard
(`ferroair-dag-parser`, FA1) into `src/panic_safe.rs`:

1. **Bracket pre-scan** — iterative byte scan, reject grouping-delimiter
   nesting deeper than 256.
2. **Lexer pre-scan** — one iterative real-tokenizer pass tracking
   `brackets + op_run + line_right_rec + indent` (cap 1024); `op_run` counts a
   consecutive run across *all* mixed prefix operators / recursion-driving
   keywords (`Yield`/`From`/`Async` included) and resets only at an
   operand/bracket/newline (this is what closes the mixed-prefix Finding 2 and
   the `yield`/`async` vectors). Also rejects PEP-750 t-strings (the parser
   panics on them).
3. **Dedicated 128 MiB stack** — the parse **and** the AST walk run on a
   128 MiB thread so the numeric cap, not the caller's ~2 MiB stack, is the
   binding limit; the thread's `join()` also folds any unwinding panic into
   `ParseError::Internal`.

The recursive AST walkers in `ruff_impl.rs` (`collect_shift_edges`,
`terminal_task_ids`, `resolve_to_task_id`, `stringify_expr`, decorator
`inner_name`) additionally truncate past `MAX_WALK_DEPTH = 1024`, so a deep
left-leaning `>>` / attribute / call chain that survives the parser (these are
left-associative, built iteratively, and not bounded by the lexer cap) cannot
overflow the walk either. This closed an additional Codex-DD-verified overflow:
`schedule=a.a.a…` (~200 k deep) overflowed `stringify_expr` even on the 128 MiB
stack; it now returns gracefully.

Regression coverage: `tests/stack_safety.rs` (each `*_rejected` test is
non-vacuous — removing the guard aborts the test binary with a stack overflow),
`tests/mutation_guard.rs` (cap boundaries), unit tests in `panic_safe.rs`.

This is **not** a claim of bulletproof input handling. The true guarantee is
exactly those three layers plus the walker depth cap: a numeric depth cap far
above any real DAG (deepest measured across the vendored Apache Airflow tree is
96) and far below the overflow threshold, a real-tokenizer reject pass with no
byte-heuristic false positives, and a dedicated stack that makes the cap the
binding limit.

### Residual (honest) — deep left-leaning AST construction / drop

A single left-leaning chain of **hundreds of thousands** of attribute / call /
shift trailers (`a.a.a…`, `f()()…`, `a >> a >> …`) requires a **multi-MB**
source file and builds a correspondingly deep `Box`-linked AST. Constructing or
**dropping** such an AST recurses (intrinsic to the ruff AST + Rust `Drop`), so
it can still overflow the 128 MiB parse stack at extreme depth (measured:
`~2 M` call trailers ≈ a 4 MiB file). This is:

- a **different mechanism** from FP5 (it is the recursive AST `Drop`, not the
  parser's recursive descent), and not a defect introduced by this crate;
- **shared with the upstream `ferroair-dag-parser`** (same vendored AST), where
  it is likewise bounded by the 128 MiB stack and accepted;
- **not cheaply closable pre-parse** — the lexer cannot distinguish a single
  N-deep chain (overflow) from N shallow siblings in a flat literal
  (`[a.b, c.d, …]`, safe) without parsing, so a token-count cap would
  false-positive on large flat DAG literals.

The realistic FP5 parser-recursion shapes (each well under 4 KiB) and the
Codex-DD-verified `stringify_expr` walk overflow are fully closed; the residual
requires a multi-MB single-chain file and is bounded by the 128 MiB stack.
