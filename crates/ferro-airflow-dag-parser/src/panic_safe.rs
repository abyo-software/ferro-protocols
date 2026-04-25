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

/// Parse a Python module while isolating any panic from the upstream
/// parser as [`ParseError::Internal`].
pub fn parse_module_safely(source: &str) -> Result<Parsed<ruff_python_ast::ModModule>, ParseError> {
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
