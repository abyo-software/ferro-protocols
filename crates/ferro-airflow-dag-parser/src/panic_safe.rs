// SPDX-License-Identifier: Apache-2.0
//! Stack-safety + panic isolation around the upstream
//! `littrs-ruff-python-parser` recursive-descent parser.
//!
//! `littrs-ruff-python-parser` (the crates.io mirror of
//! `astral-sh/ruff`'s parser) can take down the host process on
//! adversarial DAG source in two distinct ways, and a dag-processor
//! that watches an attacker-writable folder is exposed to both:
//!
//! 1. **Stack overflow.** The parser descends one frame per grouping
//!    construct (`([{`, dict/set, f-string field) *and* per
//!    prefix-unary / right-associative / conditional operator, with no
//!    upstream recursion limit (verified against `ParseOptions` in
//!    0.6.2). A pathologically nested input overflows the parse thread's
//!    guard page, which Linux delivers as a fatal `SIGSEGV`. A
//!    guard-page fault is **not** an unwinding panic, so
//!    [`std::panic::catch_unwind`] / thread-`join` CANNOT intercept it.
//! 2. **Unwinding panic.** The vendored parser reaches an
//!    `unreachable!()` (`expression.rs:1633`) on PEP 750 t-strings and a
//!    handful of f-/t-string token-confusion shapes (the 2026-05-03 fuzz
//!    wave, 7 distinct repros). That is a normal Rust panic.
//!
//! The defence is three layers, ported from the `FerroAir` dag-parser
//! (`ferroair-dag-parser::api`, FA1 / F-2026-06-08), which shares the
//! exact same 0.6.2 backend:
//!
//! * **Layer 1 — bracket pre-scan** ([`nesting_depth_exceeds`]): an
//!   iterative byte scan that rejects grouping-delimiter nesting deeper
//!   than [`MAX_PARSE_NESTING_DEPTH`] before the parser runs.
//! * **Layer 1b — lexer pre-scan** ([`lex_reject_reason`]): a single
//!   iterative tokenizer pass that rejects t-strings (which the parser
//!   panics on) and source whose expression-recursion depth
//!   (`brackets + operator-run + per-line right-recursion + indent`)
//!   would exceed [`MAX_PARSE_RECURSION`] — this catches the *non*-bracket
//!   recursion vectors (`not`/`await`/`~`/`-` chains, `a**b**c`,
//!   `a if b else …`, `lambda: lambda:`, deep statement indent) that a
//!   bracket-only cap misses entirely.
//! * **Layer 2 — large dedicated stack** ([`PARSE_STACK_SIZE`]): the
//!   parse *and* the AST walk run on a 128 MiB thread so the numeric cap,
//!   never the (possibly ~2 MiB) caller stack, is the binding limit; the
//!   thread's `join()` also folds any unwinding panic into
//!   [`ParseError::Internal`].
//!
//! This is not a claim of bulletproof input handling — the true
//! guarantee is exactly those three layers: a numeric depth cap that
//! sits far above any real DAG and far below the overflow threshold, a
//! real-tokenizer reject pass with no byte-heuristic false positives,
//! and a dedicated stack that makes the cap (not the host stack) the
//! limit.

use std::panic::AssertUnwindSafe;

use ruff_python_parser as ruff_parser;
use ruff_python_parser::{ParseError as RuffParseError, Parsed};

use crate::common::ParseError;

/// Maximum grouping-delimiter nesting depth accepted before the source
/// is rejected WITHOUT invoking the recursive-descent parser.
///
/// `256` is chosen with measured headroom: across the entire vendored
/// Apache Airflow source tree the deepest legitimate file nests to 96
/// (an AWS `SageMaker` example DAG with a deeply nested boto config); the
/// next deepest is 17. A cap of 256 clears the real-world maximum by
/// 2.6× while still rejecting the multi-hundred-deep adversarial inputs
/// the fuzz corpus accumulates.
///
/// The byte scan never *under*-counts a real grouping delimiter (every
/// `([{` is a literal byte the scan sees), so any input that would
/// recurse past the cap is rejected; it may *over*-count brackets that
/// live inside string / comment literals, which only makes the guard
/// reject *more* (the safe direction).
const MAX_PARSE_NESTING_DEPTH: usize = 256;

/// Maximum recursive-expression depth (grouping delimiters + operator
/// recursion + statement indent) accepted before the source is rejected.
///
/// The bracket pre-scan above bounds `([{` nesting, but the vendored
/// parser also recurses with no grouping delimiter on prefix-unary
/// chains (`not not not …`, `- - - …`, `~~~…`, `await await …` — one
/// frame per operator), on the right-associative power operator
/// (`a ** b ** c ** …`), on conditional / lambda chains
/// (`a if b else …`, `lambda: lambda: …`), and at the *statement* level
/// on deeply nested compound statements (`if a:` / `for …:` …, one frame
/// per indentation level). None of these are caught by counting
/// brackets, yet each overflows even a 128 MiB stack within a realistic
/// input size. A stack-overflow abort is uncatchable, so it must be
/// bounded *before* the parser runs.
///
/// `1024` clears any realistic DAG by a wide margin — across the
/// vendored Apache Airflow tree the deepest combined depth is well under
/// ~120 (the depth-96 `SageMaker` bracket nest) — while sitting far below
/// the ~100 k-frame depth at which the parser overflows.
const MAX_PARSE_RECURSION: usize = 1024;

/// Stack size for the dedicated parse / walk thread. A `tokio` worker or
/// `std::thread::spawn` thread defaults to only ~2 MiB, which a
/// legitimately deep DAG (the depth-96 `SageMaker` example) can already
/// exhaust under instrumentation. Parsing on a 128 MiB stack gives the
/// recursive descent ample headroom so that the depth cap above — never
/// a stack overflow — is what stops a deep input, regardless of how
/// small the *caller's* stack is.
const PARSE_STACK_SIZE: usize = 128 * 1024 * 1024;

/// Parse a Python module, surfacing an upstream unwinding panic as
/// [`ParseError::Internal`] and a syntax error as [`ParseError::Parse`].
///
/// This is the *inner* parse: it assumes the caller has already run the
/// [`shield_parser_panic`] pre-scans (so pathologically nested input
/// never reaches it) and runs inside the shield's large stack. The
/// `catch_unwind` is retained as defence-in-depth so a direct call is
/// still panic-safe.
pub fn parse_module_safely(source: &str) -> Result<Parsed<ruff_python_ast::ModModule>, ParseError> {
    let result: Result<Result<Parsed<_>, RuffParseError>, _> =
        std::panic::catch_unwind(AssertUnwindSafe(|| ruff_parser::parse_module(source)));
    match result {
        Ok(Ok(parsed)) => Ok(parsed),
        Ok(Err(e)) => Err(ParseError::Parse(e.to_string())),
        Err(panic_payload) => {
            let msg = panic_payload_message(panic_payload.as_ref());
            Err(ParseError::Internal(format!(
                "ruff_python_parser panicked on input: {msg}"
            )))
        }
    }
}

/// Iterative (non-recursive, overflow-proof) bracket pre-scan. Returns
/// the maximum grouping-delimiter nesting depth iff it exceeds `cap`.
///
/// Counts opening brackets `(`, `[`, `{` and pairs them with `)`, `]`,
/// `}`. String literals and comments are NOT excluded — that would
/// require a real tokenizer; the resulting false positives are harmless
/// because `cap` is far above what real code uses.
fn nesting_depth_exceeds(source: &str, cap: usize) -> Option<usize> {
    let mut depth: usize = 0;
    let mut max: usize = 0;
    for &b in source.as_bytes() {
        match b {
            b'(' | b'[' | b'{' => {
                depth += 1;
                if depth > max {
                    max = depth;
                }
            }
            b')' | b']' | b'}' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    (max > cap).then_some(max)
}

/// Single iterative lexer pass that rejects source the vendored parser
/// cannot handle safely, returning a skip-file reason or `None`.
///
/// The lexer is iterative (a mode *stack*, never recursion) so it cannot
/// itself overflow, and — being the real tokenizer — it classifies
/// `t"`/`{{`/operators-inside-strings correctly, so there are no
/// byte-heuristic false positives. Two rejections:
///
/// 1. **t-strings** (`TStringStart`) — the parser PANICS on PEP 750
///    t-strings (`unreachable!()` at `expression.rs:1633`) rather than
///    erroring. Real Airflow DAGs (Python <= 3.12) never use them and
///    the vendored parser cannot parse them anyway.
/// 2. **Excessive expression recursion** — a running depth of
///    `brackets + consecutive-operator-run + per-line right-recursive
///    operators + indent`. The consecutive-operator run resets at every
///    operand, so left-associative chains (`a+b+c…`, big list/dict
///    literals) stay at depth ~1 and are NOT rejected; only genuine
///    prefix / `**` / conditional / lambda recursion and statement
///    indent accumulate.
fn lex_reject_reason(source: &str) -> Option<String> {
    use ruff_python_ast::token::TokenKind as T;
    use ruff_python_parser::Mode;
    use ruff_python_parser::lexer::lex;

    let mut lexer = lex(source, Mode::Module);
    let mut brackets: usize = 0;
    // Consecutive operator tokens since the last operand/delimiter —
    // captures prefix-unary chains; resets at any non-operator so
    // left-associative binary chains do not accumulate.
    let mut op_run: usize = 0;
    // Right-recursive operators (`**`, conditional `if`/`else`, `lambda`)
    // on the current logical line — these do NOT reduce at each operand,
    // so they are counted per line and reset at a top-level newline.
    let mut line_right_rec: usize = 0;
    // Block-statement nesting depth (Indent − Dedent). Deeply nested
    // compound statements (`if a:` / `for …:` / `def …:` …) recurse at
    // the *statement* level — one frame per indentation level —
    // independently of any expression nesting, so they must be bounded
    // too.
    let mut indent: usize = 0;
    let mut max: usize = 0;

    loop {
        let tk = lexer.next_token();
        match tk {
            T::EndOfFile => break,
            T::Indent => {
                indent += 1;
                op_run = 0;
            }
            T::Dedent => {
                indent = indent.saturating_sub(1);
                op_run = 0;
            }
            T::TStringStart => {
                return Some(
                    "input rejected: Python 3.14 t-strings (PEP 750) are not supported by \
                     the vendored parser"
                        .to_owned(),
                );
            }
            T::Lpar | T::Lsqb | T::Lbrace => {
                brackets += 1;
                op_run = 0;
            }
            T::Rpar | T::Rsqb | T::Rbrace => {
                brackets = brackets.saturating_sub(1);
                op_run = 0;
            }
            // Top-level statement boundary: the right-recursive run ends.
            T::Newline => {
                op_run = 0;
                if brackets == 0 {
                    line_right_rec = 0;
                }
            }
            // Trivia: a non-logical newline (a physical line break INSIDE
            // a bracket / implicit continuation) and a comment are NOT
            // operands and must be transparent to the run counters. A
            // prefix chain split across physical lines inside parens —
            // `x = (\n not\n not\n … a\n)` — recurses one parser frame per
            // `not` regardless of the line breaks; if the default arm
            // below reset `op_run` on each `NonLogicalNewline` the chain
            // would never accumulate and would overflow the parser (Codex
            // DD R6, 2026-06-16). Skipping them here makes the consecutive
            // run survive the line breaks, exactly like the operands that
            // genuinely separate operators do not.
            T::NonLogicalNewline | T::Comment => {}
            // Operators that drive parser recursion. Counting a generous
            // superset (incl. left-assoc binaries) only ever *over*-counts
            // — the safe direction — and the per-operand reset keeps real
            // binary chains shallow.
            T::Not
            | T::Minus
            | T::Plus
            | T::Tilde
            | T::Star
            | T::DoubleStar
            | T::Await
            | T::Lambda
            | T::Slash
            | T::Percent
            | T::And
            | T::Or
            | T::If
            | T::Else
            // `yield` / `yield from` recurse one parser frame per keyword:
            // `parse_yield[_from]_expression` parses another expression to
            // its right, which can be another `yield`. `yield from yield
            // from … x` (and `yield yield … x`) has NO operand between the
            // keywords, so counting `Yield` + `From` in the consecutive-run
            // makes the chain accumulate and be rejected (Codex DD R2,
            // 2026-06-16). A single `yield from x` is op_run 2 — well clear.
            | T::Yield
            | T::From
            // `async` recurses at the STATEMENT level via error recovery:
            // `parse_async_statement` consumes a stray `async`, records the
            // error, then calls `parse_statement` again — so `async async
            // … def f(): pass` is one parser frame per `async` with no
            // bracket / indent / operand between (Codex DD R3, 2026-06-16).
            // A well-formed `async def` / `async for` / `async with` is
            // op_run 1 (the next token is an operand/keyword that resets).
            | T::Async => {
                op_run += 1;
                if matches!(tk, T::DoubleStar | T::If | T::Else | T::Lambda) {
                    line_right_rec += 1;
                }
            }
            // Any operand / other token ends a consecutive-operator run.
            _ => op_run = 0,
        }
        max = max.max(brackets + op_run + line_right_rec + indent);
        if max > MAX_PARSE_RECURSION {
            return Some(format!(
                "input rejected: expression recursion depth {max} exceeds the stack-safety \
                 cap of {MAX_PARSE_RECURSION}"
            ));
        }
    }
    None
}

/// Shield a parse + AST-walk against the two ways adversarial DAG source
/// can take down the host thread (see the module docs): stack overflow
/// (headed off before the parser runs by the two pre-scans, then given a
/// 128 MiB stack so the cap is the binding limit) and an unwinding panic
/// (captured by the parse thread's `join()` and folded into
/// [`ParseError::Internal`]).
///
/// `f` should perform the *whole* unit of work that touches the AST
/// (parse + recursive visitor walk), so the walk is bounded by the same
/// large stack and depth caps as the parse — a deep tree that survives
/// the parser would otherwise overflow a recursive walker on the
/// caller's small stack.
///
/// On either rejection the production posture is: log at `warn`, skip the
/// offending file, keep serving the remaining DAGs. Callers already treat
/// [`ParseError::Parse`] / [`ParseError::Internal`] this way.
pub fn shield_parser_panic<T, F>(backend: &'static str, src: &str, f: F) -> Result<T, ParseError>
where
    F: FnOnce() -> Result<T, ParseError> + Send,
    T: Send,
{
    // Layer 1 — reject pathologically nested input before it can drive
    // the recursive-descent parser into a fatal (uncatchable) overflow.
    if let Some(depth) = nesting_depth_exceeds(src, MAX_PARSE_NESTING_DEPTH) {
        tracing::warn!(
            target: "ferro_airflow_dag_parser::shield",
            backend,
            depth,
            cap = MAX_PARSE_NESTING_DEPTH,
            "DAG source rejected: grouping-delimiter nesting depth exceeds the \
             stack-safety cap; skipping file"
        );
        return Err(ParseError::Parse(format!(
            "input rejected: bracket/brace nesting depth {depth} exceeds the \
             stack-safety cap of {MAX_PARSE_NESTING_DEPTH}"
        )));
    }

    // Layer 1b — iterative lexer pass: reject t-strings (the parser
    // panics on them) and source whose expression recursion depth
    // (brackets + operator chains incl. non-bracket prefix / `**` /
    // conditional recursion + indent) would overflow even the 128 MiB
    // stack. The lexer cannot itself overflow.
    if let Some(reason) = lex_reject_reason(src) {
        tracing::warn!(
            target: "ferro_airflow_dag_parser::shield",
            backend,
            %reason,
            "DAG source rejected before parse (stack-safety / unsupported syntax); skipping file"
        );
        return Err(ParseError::Parse(reason));
    }

    // Layer 2 — run the parse + walk on a large dedicated stack and fold
    // any unwinding panic into a skip-file error. The scoped thread lets
    // `f` borrow `src` without a copy; `join()` both captures the panic
    // and guarantees PARSE_STACK_SIZE of headroom for the recursive
    // descent regardless of the caller's stack.
    let joined = std::thread::scope(|scope| {
        std::thread::Builder::new()
            .name("ferro-airflow-dag-parse".into())
            .stack_size(PARSE_STACK_SIZE)
            .spawn_scoped(scope, f)
            .map(std::thread::ScopedJoinHandle::join)
    });

    match joined {
        // Parse + walk ran to completion — propagate its Result.
        Ok(Ok(result)) => result,
        // The thread panicked (unwinding) — fold into Internal.
        Ok(Err(payload)) => {
            let msg = panic_payload_message(payload.as_ref());
            tracing::warn!(
                target: "ferro_airflow_dag_parser::shield",
                backend,
                payload = %msg,
                "upstream python parser panicked; surfacing as ParseError::Internal"
            );
            Err(ParseError::Internal(format!(
                "ruff_python_parser panicked on input: {msg}"
            )))
        }
        // Thread spawn itself failed (OS thread / address-space limit) —
        // surface as an internal error rather than panicking the caller.
        Err(e) => Err(ParseError::Internal(format!(
            "failed to spawn dag-parse thread: {e}"
        ))),
    }
}

/// Best-effort extraction of a panic payload's text. Handles the two
/// common payload shapes (`&'static str`, `String`) and falls back to a
/// sentinel for anything else.
fn panic_payload_message(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&'static str>()
        .map(|s| (*s).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "<non-string panic payload>".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::ExtractedDag;

    // -----------------------------------------------------------------
    // parse_module_safely — inner parse + catch_unwind
    // -----------------------------------------------------------------

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

    /// Regression test for an upstream `littrs-ruff-python-parser` panic
    /// found by `cargo fuzz` (artifact
    /// `crash-11c5872116de631c61335e4a170f3d1acf82241f`). The bytes
    /// trigger a panic at `parser/expression.rs:1633:25` in the upstream
    /// parser; the inner shim must catch it and return
    /// `ParseError::Internal` rather than aborting.
    #[test]
    fn parse_module_safely_catches_upstream_parser_panic() {
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
            return;
        };
        match parse_module_safely(src) {
            Ok(_) | Err(ParseError::Parse(_) | ParseError::Internal(_)) => {}
            Err(other) => panic!("unexpected variant: {other:?}"),
        }
    }

    // -----------------------------------------------------------------
    // shield_parser_panic — large stack + join panic capture
    // -----------------------------------------------------------------

    #[test]
    fn shield_returns_ok_when_closure_does_not_panic() {
        let out = shield_parser_panic("test", "", || -> Result<Vec<ExtractedDag>, ParseError> {
            Ok(Vec::new())
        })
        .expect("no panic, no parse error");
        assert!(out.is_empty());
    }

    #[test]
    fn shield_converts_static_str_panic_to_internal() {
        let err = shield_parser_panic("test", "", || -> Result<Vec<ExtractedDag>, ParseError> {
            panic!("static-str payload");
        })
        .expect_err("must convert panic to error");
        match err {
            ParseError::Internal(msg) => assert!(msg.contains("static-str payload")),
            other => panic!("expected Internal, got {other:?}"),
        }
    }

    #[test]
    fn shield_converts_string_panic_to_internal() {
        let err = shield_parser_panic("test", "", || -> Result<Vec<ExtractedDag>, ParseError> {
            panic!("{}", String::from("owned-string payload"));
        })
        .expect_err("must convert panic to error");
        match err {
            ParseError::Internal(msg) => assert!(msg.contains("owned-string payload")),
            other => panic!("expected Internal, got {other:?}"),
        }
    }

    #[test]
    fn panic_payload_message_handles_unknown_shape() {
        let payload: Box<dyn std::any::Any + Send> = Box::new(0_u32);
        let msg = panic_payload_message(payload.as_ref());
        assert_eq!(msg, "<non-string panic payload>");
    }

    // -----------------------------------------------------------------
    // nesting_depth_exceeds — bracket pre-scan boundary
    // -----------------------------------------------------------------

    #[test]
    fn nesting_depth_exceeds_reports_depth_only_past_cap() {
        assert_eq!(nesting_depth_exceeds("([{}])", 10), None);
        assert_eq!(nesting_depth_exceeds("", 10), None);
        // closers floor the running depth at 0 — unbalanced closers never
        // report a phantom depth.
        assert_eq!(nesting_depth_exceeds("))))]]]", 10), None);
        let deep = "[".repeat(20);
        assert_eq!(nesting_depth_exceeds(&deep, 10), Some(20));
    }

    #[test]
    fn nesting_depth_exceeds_is_exact_at_the_cap_boundary() {
        // Depth EXACTLY at the cap must be admitted (the comparison is
        // strictly `>`, not `>=`); cap+1 must be rejected and report the
        // true depth. Pins the boundary so a `>`/`>=` mutant is caught.
        let cap = 8usize;
        assert_eq!(nesting_depth_exceeds(&"(".repeat(cap), cap), None);
        assert_eq!(
            nesting_depth_exceeds(&"(".repeat(cap + 1), cap),
            Some(cap + 1)
        );
        // Each of the three delimiter classes counts, and the max is the
        // peak simultaneous depth (so a balanced-then-reopened sequence
        // reports the peak, not the sum). Pins the per-`+= 1` increment.
        assert_eq!(nesting_depth_exceeds("[][][]", 1), None); // peak 1
        assert_eq!(nesting_depth_exceeds("{{{", 2), Some(3));
        assert_eq!(nesting_depth_exceeds("((( )))((( )))", 3), None); // peak 3
    }

    #[test]
    fn nesting_depth_exceeds_at_production_cap() {
        // The production cap (256) admits depth-256 and rejects 257.
        assert_eq!(
            nesting_depth_exceeds(
                &"(".repeat(MAX_PARSE_NESTING_DEPTH),
                MAX_PARSE_NESTING_DEPTH
            ),
            None
        );
        assert_eq!(
            nesting_depth_exceeds(
                &"(".repeat(MAX_PARSE_NESTING_DEPTH + 1),
                MAX_PARSE_NESTING_DEPTH
            ),
            Some(MAX_PARSE_NESTING_DEPTH + 1)
        );
    }

    // -----------------------------------------------------------------
    // lex_reject_reason — non-bracket recursion vectors + t-strings
    // -----------------------------------------------------------------

    #[test]
    fn lex_reject_catches_non_bracket_recursion_vectors() {
        // Prefix-unary, right-assoc `**`, conditional, lambda, and deep
        // statement indent recurse with NO bracket, overflowing even the
        // 128 MiB stack. All must be rejected BEFORE the parser runs.
        let prefix_not = format!("x = {}True", "not ".repeat(3000));
        let prefix_neg = format!("x = {}1", "-".repeat(3000));
        let prefix_inv = format!("x = {}1", "~".repeat(3000));
        let prefix_await = format!("x = {}y", "await ".repeat(3000));
        let pow_chain = format!("x = {}2", "2**".repeat(3000));
        let cond_chain = format!("x = {}1", "1 if a else ".repeat(2000));
        let lambda_chain = format!("f = {}1", "lambda: ".repeat(3000));
        // Deeply nested compound statements recurse at the statement level.
        let mut nested_if = String::new();
        for i in 0..1200 {
            for _ in 0..i {
                nested_if.push(' ');
            }
            nested_if.push_str("if a:\n");
        }
        for _ in 0..1200 {
            nested_if.push(' ');
        }
        nested_if.push_str("pass\n");
        for (label, src) in [
            ("not-chain", &prefix_not),
            ("neg-chain", &prefix_neg),
            ("inv-chain", &prefix_inv),
            ("await-chain", &prefix_await),
            ("pow-chain", &pow_chain),
            ("cond-chain", &cond_chain),
            ("lambda-chain", &lambda_chain),
            ("nested-if", &nested_if),
        ] {
            assert!(
                lex_reject_reason(src).is_some(),
                "{label} must be rejected by the recursion guard"
            );
        }
    }

    #[test]
    fn lex_reject_flags_tstrings() {
        let reason = lex_reject_reason("x = t\"hello {name}\"\n").expect("t-string rejected");
        assert!(reason.contains("t-strings"), "unexpected reason: {reason}");
    }

    #[test]
    fn lex_reject_has_no_false_positives_on_real_expressions() {
        // Left-associative chains, big literals, normal f-strings, and a
        // deeply-but-legitimately nested config must NOT be rejected — the
        // per-operand reset keeps them shallow.
        let sum_chain = format!("total = {}0", "v + ".repeat(5000)); // left-assoc, iterative
        let big_list = format!("xs = [{}]", "a + b, ".repeat(5000)); // 5k elements of sums
        let fstring = "msg = f\"run {dag_id} at {ts} value={x + y}\"\n".to_owned();
        let nested = format!("cfg = {}1{}", "[".repeat(96), "]".repeat(96)); // depth-96 SageMaker
        // A realistically deep DAG: nested with/for/if blocks (~12 levels).
        let mut real_nest = String::new();
        for i in 0..12 {
            real_nest.push_str(&"    ".repeat(i));
            real_nest.push_str("with x:\n");
        }
        real_nest.push_str(&"    ".repeat(12));
        real_nest.push_str("pass\n");
        for (label, src) in [
            ("sum-chain", &sum_chain),
            ("big-list", &big_list),
            ("fstring", &fstring),
            ("nested-96", &nested),
            ("real-nested-blocks", &real_nest),
        ] {
            assert!(
                lex_reject_reason(src).is_none(),
                "{label} is a legitimate expression and must NOT be rejected"
            );
        }
    }
}
