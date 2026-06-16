// SPDX-License-Identifier: Apache-2.0
//! Regression coverage for FP5 — the fuzz-discovered stack-overflow denial-of-service
//! in the vendored `littrs-ruff-python-parser` 0.6.2 recursive-descent
//! parser (and in this crate's recursive AST walkers).
//!
//! `littrs-ruff-python-parser` descends one frame per grouping construct
//! AND per prefix-unary / right-associative / conditional / lambda
//! operator, with no upstream recursion limit. Sufficiently nested
//! attacker-controlled DAG source overflows the parse thread's stack,
//! which Linux delivers as a fatal `SIGSEGV` — an abort that
//! `catch_unwind` cannot intercept. The three-layer guard in
//! `src/panic_safe.rs` (bracket pre-scan, lexer-pass recursion cap, and a
//! 128 MiB dedicated parse/walk stack) turns every such input into a
//! graceful skip-file `Err`, never a process abort.
//!
//! Each `*_rejected` test below is non-vacuous: with the guard removed
//! (temporarily revert `shield_parser_panic` to a plain parse on the
//! caller stack), the matching input aborts this test binary with
//! `SIGSEGV` instead of returning an `Err`. The `*_passes` tests pin that
//! the guard does not reject legitimate deep-but-realistic DAG shapes.

#![cfg(feature = "parser-ruff")]

use std::fmt::Write as _;

use ferro_airflow_dag_parser::{
    ParseError, detect_dynamic_markers, dynamic_markers_for, extract_all_static_dags,
    extract_static_dag,
};

/// Drive `src` through every public AST entry point and assert each one
/// RETURNS (Ok or Err) rather than aborting the process. If the guard
/// regressed, the call site overflows the stack and the test binary dies
/// with SIGSEGV here — which is exactly the failure this pins against.
fn assert_all_paths_return(src: &str) {
    let _ = extract_static_dag(src);
    let _ = extract_all_static_dags(src);
    let _ = detect_dynamic_markers(src);
    let _ = dynamic_markers_for(src);
}

/// Assert the static extractor rejects `src` with a `Parse` error whose
/// message mentions the stack-safety cap (recursion or nesting).
fn assert_cap_rejected(label: &str, src: &str) {
    // Both AST paths must reject (and never abort).
    match extract_all_static_dags(src) {
        Err(ParseError::Parse(msg)) => assert!(
            msg.contains("recursion depth") || msg.contains("nesting depth"),
            "{label}: expected a stack-safety cap message, got: {msg}"
        ),
        other => panic!("{label}: expected ParseError::Parse (cap), got {other:?}"),
    }
    match detect_dynamic_markers(src) {
        Err(ParseError::Parse(msg)) => assert!(
            msg.contains("recursion depth") || msg.contains("nesting depth"),
            "{label}: marker path expected a stack-safety cap message, got: {msg}"
        ),
        other => panic!("{label}: marker path expected ParseError::Parse (cap), got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// The five parser-recursion overflow shapes the bracket-only cap missed.
// ---------------------------------------------------------------------------

#[test]
fn prefix_not_chain_rejected() {
    // `not not not … True` — one parser frame per `not`, a keyword the
    // old byte-scan never saw.
    assert_cap_rejected("not-chain", &format!("x = {}True\n", "not ".repeat(4000)));
}

#[test]
fn prefix_await_chain_rejected() {
    // `await await … y` inside an async function — keyword prefix-unary.
    let body = format!("    return {}y\n", "await ".repeat(4000));
    assert_cap_rejected("await-chain", &format!("async def f():\n{body}"));
}

#[test]
fn prefix_byte_operator_chains_rejected() {
    // `----…1`, `~~~…1`, `++++…1` — single-byte prefix-unary chains.
    assert_cap_rejected("neg-chain", &format!("x = {}1\n", "-".repeat(4000)));
    assert_cap_rejected("inv-chain", &format!("x = {}1\n", "~".repeat(4000)));
    assert_cap_rejected("pos-chain", &format!("x = {}1\n", "+".repeat(4000)));
}

#[test]
fn power_chain_rejected() {
    // `a ** b ** c …` — the right-associative power operator recurses.
    assert_cap_rejected("pow-chain", &format!("x = {}2\n", "2**".repeat(4000)));
}

#[test]
fn conditional_chain_rejected() {
    // `1 if a else 1 if a else …` — conditional-expression recursion.
    assert_cap_rejected(
        "cond-chain",
        &format!("x = {}1\n", "1 if a else ".repeat(3000)),
    );
}

#[test]
fn lambda_chain_rejected() {
    // `lambda: lambda: …` — nested lambda bodies recurse.
    assert_cap_rejected(
        "lambda-chain",
        &format!("f = {}1\n", "lambda: ".repeat(4000)),
    );
}

#[test]
fn deep_statement_indent_rejected() {
    // Deeply nested compound statements recurse at the *statement* level
    // (one parser frame per indentation level) with no expression nesting.
    let mut src = String::new();
    for i in 0..2000 {
        for _ in 0..i {
            src.push(' ');
        }
        src.push_str("if a:\n");
    }
    for _ in 0..2000 {
        src.push(' ');
    }
    src.push_str("pass\n");
    assert_cap_rejected("nested-if", &src);
}

#[test]
fn deep_bracket_nesting_rejected() {
    // The original bracket-nesting shape (still covered by Layer 1).
    let src = format!("x = {}1{}\n", "[".repeat(4096), "]".repeat(4096));
    assert_cap_rejected("bracket-nest", &src);
}

#[test]
fn yield_from_chain_rejected() {
    // Codex DD R2 (2026-06-16): `yield from yield from … x` recurses one
    // parser frame per `yield from` (`parse_yield_from_expression` parses
    // another expression to its right). The lexer pass did not count
    // `Yield`/`From`, so the chain bypassed the cap and overflowed the
    // parser at ~50 k (550 KiB). Now `Yield` + `From` are counted.
    assert_cap_rejected(
        "yield-from-chain",
        &format!(
            "def g():\n    yield from {}x\n",
            "yield from ".repeat(50_000)
        ),
    );
}

#[test]
fn yield_chain_rejected() {
    // `yield yield yield … x` = `yield (yield (yield … x))`, same parser
    // recursion without the `from`.
    assert_cap_rejected(
        "yield-chain",
        &format!("def g():\n    yield {}x\n", "yield ".repeat(80_000)),
    );
}

#[test]
fn async_keyword_chain_rejected() {
    // Codex DD R3 (2026-06-16): `async async … def f(): pass` recurses one
    // parser frame per `async` via error recovery (`parse_async_statement`
    // consumes a stray `async`, records the error, then calls
    // `parse_statement` again). No bracket / indent / operand between, so
    // the lexer pass did not count it until `T::Async` was added.
    assert_cap_rejected(
        "async-chain",
        &format!("{}def f():\n    pass\n", "async ".repeat(90_000)),
    );
}

#[test]
fn legitimate_async_not_rejected() {
    // `T::Async` counting must NOT trip on real async statements.
    for (label, src) in [
        ("async-def", "async def f():\n    pass\n"),
        (
            "async-for",
            "async def f():\n    async for x in y:\n        pass\n",
        ),
        (
            "async-with",
            "async def f():\n    async with y as z:\n        pass\n",
        ),
        ("async-await", "async def f():\n    x = await h()\n"),
    ] {
        if let Err(ParseError::Parse(msg)) = extract_all_static_dags(src) {
            assert!(
                !msg.contains("recursion depth"),
                "{label} must NOT hit the recursion cap: {msg}"
            );
        }
    }
}

#[test]
fn legitimate_yield_and_import_not_rejected() {
    // The `Yield`/`From` counting must NOT trip on real generators or
    // `from … import` / `raise … from` (a single keyword per line resets
    // at the operand).
    for (label, src) in [
        ("single-yield-from", "def g():\n    yield from gen()\n"),
        (
            "many-yield-lines",
            "def g():\n    yield 1\n    yield 2\n    yield from h()\n",
        ),
        (
            "import-from",
            "from airflow.operators.bash import BashOperator\nfrom a.b.c import d\n",
        ),
        ("raise-from", "def f():\n    raise ValueError() from err\n"),
    ] {
        // Ok or a non-cap parse error is fine; only a recursion-cap
        // rejection would be a false positive.
        if let Err(ParseError::Parse(msg)) = extract_all_static_dags(src) {
            assert!(
                !msg.contains("recursion depth"),
                "{label} must NOT hit the recursion cap: {msg}"
            );
        }
    }
}

#[test]
fn implicit_continuation_prefix_chain_rejected() {
    // Codex DD R6 (2026-06-16): a prefix chain split across physical lines
    // INSIDE parens — `x = (\n not\n not\n … a\n)` — recurses one parser
    // frame per `not`, but each physical line break is a
    // `NonLogicalNewline` token. The lexer pass used to reset `op_run` on
    // every unrecognised token (including `NonLogicalNewline`), so the run
    // never accumulated and the chain overflowed the parser. Trivia
    // (`NonLogicalNewline` / `Comment`) is now transparent to the run.
    assert_cap_rejected(
        "not-multiline-paren",
        &format!("x = (\n{}a\n)\n", "not\n".repeat(50_000)),
    );
    // The same with a comment after each operator (also trivia).
    assert_cap_rejected(
        "not-comment-multiline-paren",
        &format!("x = (\n{}a\n)\n", "not # c\n".repeat(50_000)),
    );
}

#[test]
fn legitimate_multiline_bracketed_expr_not_rejected() {
    // A real multi-line expression inside parens/brackets (operands and
    // commas between the operators reset the run) must NOT be rejected,
    // even with comments interleaved.
    for (label, src) in [
        (
            "sum-multiline",
            format!("x = (\n{}0\n)\n", "a +\n".repeat(5000)),
        ),
        (
            "list-multiline",
            format!("xs = [\n{}]\n", "a.b,\n".repeat(5000)),
        ),
        (
            "comment-heavy",
            "x = (\n  a  # c1\n  + b  # c2\n  + c  # c3\n)\n".to_owned(),
        ),
    ] {
        if let Err(ParseError::Parse(msg)) = extract_all_static_dags(&src) {
            assert!(
                !msg.contains("recursion depth"),
                "{label} must NOT hit the recursion cap: {msg}"
            );
        }
    }
}

#[test]
fn alternating_mixed_prefix_operators_rejected() {
    // FP5 / fuzz Finding 2 (crash-0665b68…): the OLD guard counted only a
    // consecutive run of a SINGLE prefix operator (`MAX_UNARY_OP_RUN = 64`),
    // so `~not ~not …` — each individual run < 64 but the combined prefix
    // nesting unbounded — slipped through and overflowed the parser. The
    // new `op_run` counts a run across ALL mixed prefix operators and only
    // resets at an operand/bracket/newline, so the alternation accumulates
    // and is rejected.
    let src = format!("x = {}True\n", "~not ".repeat(2000));
    assert_cap_rejected("alternating-prefix", &src);
}

// ---------------------------------------------------------------------------
// t-strings: the parser PANICS on them — must be rejected pre-parse.
// ---------------------------------------------------------------------------

#[test]
fn tstring_rejected_not_fatal() {
    let src = "x = t\"hello {name}\"\n";
    match extract_static_dag(src) {
        Err(ParseError::Parse(_)) => {}
        other => panic!("t-string should be a Parse rejection, got {other:?}"),
    }
    assert_all_paths_return(src);
    // A normal f-string must still parse fine (no false positive).
    assert!(extract_static_dag("y = f\"hello {name}\"\n").is_ok());
}

// ---------------------------------------------------------------------------
// Recursive AST WALKERS in `ruff_impl` (NOT the parser): `>>`/`<<` shift
// chains, attribute chains, and call chains build deep left-leaning trees
// the parser produces *iteratively* — so they are far deeper than the
// lexer-pass recursion cap bounds and would overflow even the 128 MiB
// parse stack. `MAX_WALK_DEPTH` truncates each walker so it returns
// instead of aborting. Every N below is past the measured overflow point
// (`stringify_expr` aborted at ~200 k on the 128 MiB stack, Codex DD
// 2026-06-16), so these are non-vacuous: remove the depth guards and they
// abort the test binary with a stack overflow.
// ---------------------------------------------------------------------------

#[test]
fn deep_shift_chain_in_dag_does_not_abort() {
    // `a >> a >> a >> …` inside `with DAG(...)` drives `collect_shift_edges`
    // to recurse once per `>>`. Must return, never abort.
    let mut src = String::from("with DAG('d'):\n    a");
    for _ in 0..600_000 {
        src.push_str(" >> a");
    }
    src.push('\n');
    let _ = extract_all_static_dags(&src);
    let _ = extract_static_dag(&src);
}

#[test]
fn deep_attribute_chain_schedule_does_not_abort() {
    // `schedule=a.a.a.a…` drives `stringify_expr` to recurse once per
    // attribute access (the exact Codex-DD-verified overflow). Must return.
    let mut src = String::from("with DAG('d', schedule=a");
    for _ in 0..300_000 {
        src.push_str(".a");
    }
    src.push_str("):\n    pass\n");
    match extract_all_static_dags(&src) {
        Ok(_) | Err(_) => {} // either is fine; the point is it RETURNS
    }
    let _ = dynamic_markers_for(&src);
}

#[test]
fn moderately_deep_call_chain_in_shift_returns() {
    // `a >> f()()()…` exercises the `resolve_to_task_id` call-chain guard.
    // Its frames are tiny, so the binding limit at extreme depth is the
    // recursive AST *drop* (intrinsic to the ruff AST + Rust `Drop`, a
    // documented residual — see `dd-pack/11-known-limitations.md`), not
    // this walker. At a realistic-but-deep 100 k it returns cleanly.
    let mut src = String::from("with DAG('d'):\n    a >> f");
    for _ in 0..100_000 {
        src.push_str("()");
    }
    src.push('\n');
    let _ = extract_all_static_dags(&src);
}

#[test]
fn marker_path_decorator_call_chain_does_not_abort() {
    // Codex DD R8 (2026-06-16): the MARKER path's `match_dag_decorator` /
    // `is_task_decorator_call` helpers also recurse through a decorator
    // call chain (`@dag()()…` / `@task()()…`). They are now capped at
    // `MAX_DECORATOR_CHAIN_DEPTH`, mirroring the static walker's
    // `inner_name`. This drives the marker path past the cap and asserts
    // it returns (the deepest extreme remains the documented multi-MB
    // AST-drop residual, not these helpers).
    let dag = format!("@dag{}\ndef p():\n    pass\n", "()".repeat(4000));
    let _ = dynamic_markers_for(&dag);
    let _ = detect_dynamic_markers(&dag);
    let task = format!(
        "@dag\ndef p():\n    @task{}\n    def t():\n        pass\n",
        "()".repeat(4000)
    );
    let _ = dynamic_markers_for(&task);
    let _ = detect_dynamic_markers(&task);
}

// ---------------------------------------------------------------------------
// No false positives: legitimate deep-but-realistic DAG shapes must pass.
// ---------------------------------------------------------------------------

#[test]
fn deepest_real_world_dag_depth_passes() {
    // 96 is the deepest nesting measured across the entire vendored Apache
    // Airflow source tree (an AWS SageMaker example DAG). It must parse,
    // NOT be rejected by the stack-safety cap, so the "run Airflow example
    // DAGs unmodified" contract holds.
    let src = format!("value = {}1{}\n", "[".repeat(96), "]".repeat(96));
    match extract_all_static_dags(&src) {
        Ok(_) => {}
        Err(ParseError::Parse(msg)) => assert!(
            !msg.contains("recursion depth") && !msg.contains("nesting depth"),
            "depth-96 real-world DAG must not hit the cap: {msg}"
        ),
        Err(other) => panic!("unexpected error for depth-96 DAG: {other:?}"),
    }
}

#[test]
fn long_legit_task_chain_passes() {
    // A real DAG that chains 200 tasks with `>>` and a left-associative
    // arithmetic default — both stay shallow in the recursion metric and
    // must NOT be rejected.
    let mut src = String::from("with DAG('pipeline') as dag:\n");
    for i in 0..200 {
        let _ = writeln!(src, "    t{i} = BashOperator(task_id=\"t{i}\")");
    }
    src.push_str("    t0");
    for i in 1..200 {
        let _ = write!(src, " >> t{i}");
    }
    src.push('\n');
    match extract_all_static_dags(&src) {
        Ok(dags) => assert_eq!(dags.len(), 1, "the legit chained DAG must be recovered"),
        Err(other) => panic!("legit 200-task chain must parse, got {other:?}"),
    }
}

#[test]
fn left_associative_arithmetic_chain_passes() {
    // `v + v + v + … + 0` is a 5000-term left-assoc chain. The parser
    // builds it iteratively and the recursion metric's per-operand reset
    // keeps it at depth ~1 — it must NOT be rejected.
    let src = format!("total = {}0\n", "v + ".repeat(5000));
    match extract_all_static_dags(&src) {
        Ok(_) => {}
        Err(ParseError::Parse(msg)) => assert!(
            !msg.contains("recursion depth") && !msg.contains("nesting depth"),
            "left-assoc arithmetic must not hit the cap: {msg}"
        ),
        Err(other) => panic!("unexpected error: {other:?}"),
    }
}
