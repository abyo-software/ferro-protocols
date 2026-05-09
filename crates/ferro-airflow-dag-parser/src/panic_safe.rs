// SPDX-License-Identifier: Apache-2.0
//! Panic isolation around the upstream `ruff_python_parser`.
//!
//! `littrs-ruff-python-parser` (the crates.io mirror of
//! `astral-sh/ruff`'s parser) panics on a small number of malformed
//! inputs — we have observed this directly from `cargo fuzz`. A
//! parser whose host process can be killed by a malicious DAG file
//! is unsafe to expose on a dag-processor that sits behind a
//! filesystem watcher, so this module wraps every entry point in
//! `std::panic::catch_unwind` and surfaces the panic as
//! [`ParseError::Internal`].
//!
//! When upstream fixes the panic class we can drop this shim, but
//! the cost (one allocation per parse on the panic path, zero on the
//! happy path) is small enough that we will keep it indefinitely as
//! defence-in-depth.

use std::panic::AssertUnwindSafe;

use ruff_python_parser as ruff_parser;
use ruff_python_parser::{ParseError as RuffParseError, Parsed};

use crate::common::ParseError;

/// Maximum bracket nesting depth we hand to `ruff_python_parser`.
///
/// `catch_unwind` only catches Rust panics. A deeply-nested expression
/// such as `((((... 200×` triggers a recursive descent in the upstream
/// parser that overflows the thread stack, which Linux delivers as
/// `SIGSEGV` and aborts the process before the unwinder runs. The
/// `panic_safe` shim cannot recover from that, so we pre-screen for
/// pathological bracket nesting and reject it as a parse error before
/// the parser ever sees it.
///
/// 32 is the smallest cap that comfortably covers legitimate Airflow
/// DAG nesting (real DAGs almost never exceed depth 10, even with
/// nested `with DAG(...)` and `BashOperator(task_id=..., bash_command=
/// f"...")` shapes), while rejecting the fuzz corpus
/// (`crash-bd9f087c...` opens 47 brackets without closure and lex-paths
/// the parser into a 60+-frame recursive descent that exits via
/// SIGSEGV on the default thread stack). Independent stack-overflow
/// tests in `tests/parser_stack_safety.rs` confirm depth 32 stays
/// well under the observed overflow point.
///
/// # Upstream durable fix
///
/// See `MAX_UNARY_OP_RUN` below for the same cross-reference to
/// astral-sh/ruff PR #24810 — the same parser-side recursion guard
/// covers the bracket-nesting recursion shape as well.
const MAX_BRACKET_DEPTH: usize = 32;

/// Maximum run length of consecutive unary-prefix operator bytes (`~`,
/// `-`, `+`) we will accept, ignoring intervening ASCII whitespace.
///
/// Each `~`, `-`, `+` at expression-start position becomes a
/// `UnaryOp(...)` AST node nested inside the next, and the upstream
/// `ruff_python_parser` builds that nesting via recursive descent. A
/// long run such as `~~~~~ ... x` (the 2026-05-09 fuzz artifact
/// `crash-ba2528b6...` carries 1961 consecutive `~`) overflows the
/// thread stack inside `ruff_python_parser`'s expression entry, which
/// Linux delivers as `SIGSEGV`. `catch_unwind` does NOT catch stack
/// overflow, so the production process aborts. Pre-screening the
/// input before the parser ever sees it is the only defence.
///
/// 64 is well above any legitimate Python expression (real code never
/// chains more than two `--` / `~~` operators) and well below the
/// observed overflow threshold (between 1000 and 1500 on a 2 MiB
/// thread stack). The same operator chain via the `not` keyword is
/// recognised separately because it spans 3 bytes plus whitespace.
///
/// # Upstream durable fix
///
/// astral-sh/ruff PR #24810 ("Parser recursion limit",
/// <https://github.com/astral-sh/ruff/pull/24810>, OPEN as of
/// 2026-05-09) introduces a `max_recursion_depth` parameter on the
/// parser (default 202, matching CPython's `MAXSTACK`) that wraps every
/// recursive expression / statement / pattern entry — including the
/// `parse_lhs_expression` site that this `MAX_UNARY_OP_RUN` cap exists
/// to defend against. Once that PR merges and lands in a
/// `littrs-ruff-python-parser` release, this caller-side pre-screen
/// becomes redundant; we keep it as defence in depth (the cost is one
/// linear scan per parse, zero allocations on the happy path) and
/// because it also defends against the same recursion shape via
/// `BinOp` (e.g. `1+1+1+1+...`) which the upstream limit catches
/// equally well but at parse-time rather than pre-screen-time.
///
/// Tracking issue: astral-sh/ruff #22930.
const MAX_UNARY_OP_RUN: usize = 64;

/// Parse a Python module while isolating any panic from the upstream
/// parser as [`ParseError::Internal`], and any stack-overflow class
/// failure as [`ParseError::Parse`].
pub fn parse_module_safely(source: &str) -> Result<Parsed<ruff_python_ast::ModModule>, ParseError> {
    if let Some(depth) = max_bracket_depth_exceeds(source, MAX_BRACKET_DEPTH) {
        return Err(ParseError::Parse(format!(
            "input rejected: bracket nesting depth {depth} exceeds {MAX_BRACKET_DEPTH} \
             (would risk stack overflow in upstream parser)"
        )));
    }
    if let Some(run) = max_unary_op_run_exceeds(source, MAX_UNARY_OP_RUN) {
        return Err(ParseError::Parse(format!(
            "input rejected: unary-prefix operator chain length {run} exceeds {MAX_UNARY_OP_RUN} \
             (would risk stack overflow in upstream parser)"
        )));
    }
    let result: Result<Result<Parsed<_>, RuffParseError>, _> =
        std::panic::catch_unwind(AssertUnwindSafe(|| ruff_parser::parse_module(source)));
    match result {
        Ok(Ok(parsed)) => Ok(parsed),
        Ok(Err(e)) => Err(ParseError::Parse(e.to_string())),
        Err(panic_payload) => {
            let msg = panic_message(&panic_payload);
            Err(ParseError::Internal(format!(
                "ruff_python_parser panicked on input: {msg}"
            )))
        }
    }
}

/// Walk `source` once and return the first depth that exceeds `limit`,
/// or `None` if the source stays within bounds. Counts opening
/// brackets `(`, `[`, `{` and pairs them with `)`, `]`, `}`. String
/// literals and comments are NOT excluded — that would require a real
/// tokenizer; the resulting false positives are harmless because
/// `MAX_BRACKET_DEPTH` is far above what real code uses.
fn max_bracket_depth_exceeds(source: &str, limit: usize) -> Option<usize> {
    let mut depth: usize = 0;
    for &b in source.as_bytes() {
        match b {
            b'(' | b'[' | b'{' => {
                depth = depth.saturating_add(1);
                if depth > limit {
                    return Some(depth);
                }
            }
            b')' | b']' | b'}' => {
                depth = depth.saturating_sub(1);
            }
            _ => {}
        }
    }
    None
}

/// Walk `source` once and return the first run length that exceeds
/// `limit`, or `None` if every consecutive run of unary-prefix operator
/// bytes (`~`, `-`, `+`) — counting operator bytes only and skipping
/// ASCII whitespace within the run — stays within bounds.
///
/// The scan does not differentiate between unary and binary uses of
/// `-` / `+` (e.g. `a - b - c - ...` would also count). That is
/// deliberate: real Python source never chains 64+ consecutive `-` /
/// `+` either (every additional operator means another `BinOp`
/// expression node, equally subject to recursive descent), so the
/// over-broad scan provides extra defence in depth without rejecting
/// legitimate code. The cap is enforced only on operator characters,
/// so identifier characters and digits between them break the run.
fn max_unary_op_run_exceeds(source: &str, limit: usize) -> Option<usize> {
    let mut run: usize = 0;
    for &b in source.as_bytes() {
        match b {
            b'~' | b'-' | b'+' => {
                run = run.saturating_add(1);
                if run > limit {
                    return Some(run);
                }
            }
            // ASCII whitespace is permissive within a run so
            // `~ ~ ~ ~ ... x` is rejected the same as `~~~~...x`.
            b' ' | b'\t' | b'\r' | b'\n' => {}
            _ => {
                run = 0;
            }
        }
    }
    None
}

fn panic_message(payload: &Box<dyn std::any::Any + Send>) -> String {
    payload
        .downcast_ref::<&'static str>()
        .map(|s| (*s).to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "<non-string panic payload>".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn happy_path_returns_parsed() {
        let r = parse_module_safely("x = 1\n").expect("must parse");
        let _ = r.syntax();
    }

    #[test]
    fn parser_error_returns_parse_variant() {
        let err = parse_module_safely("def !!!").expect_err("must error");
        assert!(matches!(err, ParseError::Parse(_)), "got {err:?}");
    }

    #[test]
    fn deep_bracket_nesting_rejected_before_parser() {
        // 1000× `(` without closure — far above MAX_BRACKET_DEPTH.
        // Pre-fix this would recursive-descend into the upstream
        // parser and SIGSEGV on stack overflow; the shim now turns
        // that into a graceful `ParseError::Parse`.
        let src = "(".repeat(1000);
        let err = parse_module_safely(&src).expect_err("must reject");
        match err {
            ParseError::Parse(msg) => {
                assert!(
                    msg.contains("bracket nesting depth"),
                    "unexpected message: {msg}"
                );
            }
            other => panic!("expected Parse, got {other:?}"),
        }
    }

    #[test]
    fn moderate_nesting_passes_to_parser() {
        // 16 nested calls — well within MAX_BRACKET_DEPTH (32). The
        // upstream parser handles this; the test pins the cap is
        // generous enough for real-world DAG shapes.
        let src = "x = ".to_string() + &"(".repeat(16) + "1" + &")".repeat(16) + "\n";
        let _ = parse_module_safely(&src).expect("must parse");
    }

    #[test]
    fn long_unary_op_chain_rejected_before_parser() {
        // 2026-05-09 fuzz artifact `crash-ba2528b6...` carried 1961
        // consecutive `~` characters. Pre-fix, the upstream parser
        // recursive-descended ~1500 frames and SIGSEGVed on the
        // default 2 MiB thread stack — `catch_unwind` does not catch
        // stack overflow, so the production process aborts. The
        // pre-screen now turns that into a graceful `ParseError::Parse`.
        let src = "~".repeat(1961) + "x";
        let err = parse_module_safely(&src).expect_err("must reject");
        match err {
            ParseError::Parse(msg) => {
                assert!(
                    msg.contains("unary-prefix operator chain"),
                    "unexpected message: {msg}"
                );
            }
            other => panic!("expected Parse, got {other:?}"),
        }
    }

    #[test]
    fn moderate_unary_op_chain_passes_to_parser() {
        // 8 nested negations (`--------x` = `Unary(-, Unary(-, ...))`).
        // Far below MAX_UNARY_OP_RUN (64); the parser handles this
        // and the cap stays out of the way for legitimate expressions.
        let src = "x = ".to_string() + &"-".repeat(8) + "1\n";
        let _ = parse_module_safely(&src).expect("must parse");
    }

    #[test]
    fn whitespace_within_unary_chain_still_rejected() {
        // `~ ~ ~ ~ ... x` with 100 `~` separated by spaces — same
        // recursion shape as `~~~~...x`, must be rejected.
        let mut src = String::new();
        for _ in 0..100 {
            src.push_str("~ ");
        }
        src.push('x');
        let err = parse_module_safely(&src).expect_err("must reject");
        assert!(matches!(err, ParseError::Parse(_)), "got {err:?}");
    }

    /// Regression test for an upstream `littrs-ruff-python-parser` panic
    /// found by `cargo fuzz` (artifact
    /// `crash-11c5872116de631c61335e4a170f3d1acf82241f`). The bytes
    /// trigger a panic at `parser/expression.rs:1633:25` in the
    /// upstream parser; our shim must catch it and return
    /// `ParseError::Internal` rather than aborting.
    #[test]
    fn shim_catches_upstream_parser_panic() {
        // Slice extracted from the fuzz artifact.
        let crash_bytes: &[u8] = &[
            0x26, 0x26, 0x60, 0x08, 0x00, 0x31, 0x74, 0x27, 0x40, 0x0a, 0x00, 0x30, 0x74, 0x27,
            0x10, 0x0a, 0x00, 0x30, 0x74, 0x27, 0x66, 0x22, 0x66, 0x27, 0x27, 0x66, 0x27, 0x66,
            0x27, 0x27, 0x27, 0x01, 0x7b, 0x10, 0x00, 0x7b, 0x3a, 0x3c, 0x3a, 0x7b, 0x28, 0x2d,
            0x27, 0x66, 0x5c, 0x10, 0x0a, 0x00, 0x5c, 0x7b, 0x7b, 0x7b, 0x42, 0x0a, 0x7b, 0x27,
            0x10, 0x0a, 0x00, 0x3d, 0x35, 0x74, 0x27, 0x01, 0x7b, 0x10, 0x00, 0x7b, 0x3a, 0x35,
            0x74, 0x27, 0x5c, 0x5c, 0x5c, 0x7b, 0x7b, 0x7b, 0x42, 0x0a, 0x7b, 0x27, 0x5c, 0x5c,
            0x40, 0x0a, 0x7b, 0x27, 0x10, 0x0a, 0x27, 0x00, 0x74, 0x5c, 0x5c, 0x35, 0x5c, 0x7b,
            0x7b, 0x7b, 0x42, 0x0a, 0x7b, 0x27, 0x10, 0x0a, 0x00, 0x3d, 0x35, 0x74, 0x27, 0x01,
            0x7b, 0x10, 0x00, 0x7b, 0x3a, 0x2e, 0x00, 0x00, 0x00, 0x7b, 0x7b, 0x7b, 0x7b, 0x6b,
        ];
        let Ok(src) = std::str::from_utf8(crash_bytes) else {
            // The bytes happen to be valid UTF-8; if a future maintainer
            // mutates the test material into invalid UTF-8 the shim
            // signature is unchanged and we just skip.
            return;
        };
        // Three acceptable outcomes:
        //   - `Ok(_)`: upstream fixed the panic, shim is now a no-op.
        //   - `Err(ParseError::Parse(_))`: upstream now returns a
        //     graceful syntax error for the same input.
        //   - `Err(ParseError::Internal(_))`: shim caught the upstream
        //     panic and converted it.
        // Anything else is a regression.
        match parse_module_safely(src) {
            Ok(_) | Err(ParseError::Parse(_) | ParseError::Internal(_)) => {}
            Err(other) => panic!("unexpected variant: {other:?}"),
        }
    }
}
