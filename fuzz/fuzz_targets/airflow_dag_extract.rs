// SPDX-License-Identifier: Apache-2.0
//! Focused fuzz target for the Airflow DAG static extractor.
//!
//! `extract_all_static_dags` (`crates/ferro-airflow-dag-parser/src/api.rs`)
//! parses arbitrary Python source via `littrs-ruff-python-parser` and
//! returns the DAGs declared statically in the file. The HARDENING
//! report (`HARDENING_REPORT.md` §5) records that fuzz already
//! discovered an upstream parser panic at
//! `parser/expression.rs:1633:25`, which is now caught by the
//! `panic_safe` shim. This target is the regression sentinel for that
//! shim — a future cargo-fuzz crash here means either a NEW upstream
//! parser shape or that the shim was bypassed.
//!
//! ## Why we override the panic hook
//!
//! `libfuzzer-sys` installs a global panic hook in `initialize()` that
//! calls `process::abort()` on every panic, _before_ any
//! `catch_unwind` boundary further up the stack runs. That kills the
//! production `panic_safe` shim's `catch_unwind` (the panic still
//! propagates as `Err`, but the host process is dead first). This
//! shim is exactly the contract we want to exercise here, so we
//! restore the default hook (silent print) and let the production
//! `catch_unwind` convert the panic into `ParseError::Internal` as it
//! does for real callers.
//!
//! Watching for: panics that escape the `catch_unwind` shim,
//! unbounded recursion (stack overflow) on deeply nested Python
//! syntax, OOM from huge identifier vectors, and infinite loops on
//! adversarial input. A genuine escape (the shim being bypassed) will
//! still abort because the production code re-panics or aborts via a
//! non-panic path; the hook override only neutralises the false-abort
//! caused by libfuzzer-sys aborting on a panic the production shim
//! would have caught.

#![no_main]

use std::sync::Once;

use libfuzzer_sys::fuzz_target;

use ferro_airflow_dag_parser::extract_all_static_dags;

static INSTALL_HOOK: Once = Once::new();

fn install_silent_panic_hook() {
    INSTALL_HOOK.call_once(|| {
        std::panic::set_hook(Box::new(|_info| {
            // Intentionally no-op: the production `panic_safe` shim
            // catches the panic via `catch_unwind` and returns
            // `ParseError::Internal`. We do not want libfuzzer-sys to
            // abort the process on a panic the shim handles.
        }));
    });
}

fuzz_target!(|data: &[u8]| {
    install_silent_panic_hook();
    let Ok(src) = std::str::from_utf8(data) else {
        return;
    };
    let _ = extract_all_static_dags(src);
});
