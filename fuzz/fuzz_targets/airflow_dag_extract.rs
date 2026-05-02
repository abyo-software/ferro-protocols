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
//! Watching for: panics that escape the `catch_unwind` shim,
//! unbounded recursion (stack overflow) on deeply nested Python
//! syntax, OOM from huge identifier vectors, and infinite loops on
//! adversarial input.

#![no_main]

use libfuzzer_sys::fuzz_target;

use ferro_airflow_dag_parser::extract_all_static_dags;

fuzz_target!(|data: &[u8]| {
    let Ok(src) = std::str::from_utf8(data) else {
        return;
    };
    let _ = extract_all_static_dags(src);
});
