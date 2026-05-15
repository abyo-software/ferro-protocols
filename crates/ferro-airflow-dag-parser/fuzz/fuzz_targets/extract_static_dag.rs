// SPDX-License-Identifier: Apache-2.0
//! Fuzz target: feed arbitrary bytes into `extract_static_dag` and
//! confirm the parser never panics. The bytes do not have to be
//! valid Python — the parser reports `ParseError::Parse` for invalid
//! inputs and the call should always return `Ok` or `Err`, never
//! unwind.
//!
//! Silent panic hook: see top-level `fuzz/fuzz_targets/airflow_dag_extract.rs`
//! (b88a6e2). libfuzzer-sys installs a global hook that aborts on
//! every panic before any `catch_unwind` further up the stack runs.
//! That bypasses the production `panic_safe::parse_module_safely`
//! shim. Override with a no-op hook (Once-guarded) so the shim's
//! `catch_unwind` runs to completion just as it does for real
//! callers. A genuine escape (shim bypassed via SIGSEGV) still
//! aborts via the non-panic path.

#![no_main]

use std::sync::Once;

use ferro_airflow_dag_parser::extract_static_dag;
use libfuzzer_sys::fuzz_target;

static INSTALL_HOOK: Once = Once::new();

fn install_silent_panic_hook() {
    INSTALL_HOOK.call_once(|| {
        std::panic::set_hook(Box::new(|_info| {}));
    });
}

fuzz_target!(|data: &[u8]| {
    install_silent_panic_hook();
    let Ok(src) = std::str::from_utf8(data) else {
        return;
    };
    let _ = extract_static_dag(src);
});
