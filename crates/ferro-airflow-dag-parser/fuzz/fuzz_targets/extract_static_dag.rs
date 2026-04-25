// SPDX-License-Identifier: Apache-2.0
//! Fuzz target: feed arbitrary bytes into `extract_static_dag` and
//! confirm the parser never panics. The bytes do not have to be
//! valid Python — the parser reports `ParseError::Parse` for invalid
//! inputs and the call should always return `Ok` or `Err`, never
//! unwind.

#![no_main]

use ferro_airflow_dag_parser::extract_static_dag;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(src) = std::str::from_utf8(data) else {
        return;
    };
    let _ = extract_static_dag(src);
});
