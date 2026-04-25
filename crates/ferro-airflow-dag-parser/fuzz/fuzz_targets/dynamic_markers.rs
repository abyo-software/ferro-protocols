// SPDX-License-Identifier: Apache-2.0
//! Fuzz target: arbitrary input → `dynamic_markers_for`. Surface area
//! is the AST visitor; this target covers the marker-detection branch
//! that the `extract_static_dag` target does not.

#![no_main]

use ferro_airflow_dag_parser::dynamic_markers_for;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(src) = std::str::from_utf8(data) else {
        return;
    };
    let _ = dynamic_markers_for(src);
});
