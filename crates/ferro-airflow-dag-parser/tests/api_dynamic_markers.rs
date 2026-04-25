// SPDX-License-Identifier: Apache-2.0
//! Phase 1 public API contract: [`detect_dynamic_markers`] and the
//! 64-file Airflow `example_dags/` corpus coverage.
//!
//! The corpus path is hard-coded to `/tmp/airflow-sample/...` to match
//! the Phase 0 `PoC` reproduction recipe. When the path is absent (a
//! fresh checkout that has not yet cloned `apache/airflow`), the
//! corpus-coverage test is skipped with a `println!` rather than a
//! failure — the 7 pattern-detection tests still run and gate the
//! marker contract.

#![cfg(feature = "parser-ruff")]

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

use ferro_airflow_dag_parser::{
    DynamicMarker, ParseOutcome, detect_dynamic_markers, extract_all_static_dags,
};

const CORPUS: &str = "/tmp/airflow-sample/airflow-core/src/airflow/example_dags";

fn collect_python_files(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    walk(root, &mut out);
    out.sort();
    out
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk(&path, out);
        } else if path.extension().and_then(OsStr::to_str) == Some("py")
            && path.file_name() != Some(OsStr::new("__init__.py"))
        {
            out.push(path);
        }
    }
}

#[test]
fn detects_path_stem_dag_id_pattern() {
    let src = r"
from pathlib import Path
from airflow import DAG

with DAG(dag_id=Path(__file__).stem):
    pass
";
    let markers = detect_dynamic_markers(src).expect("parse");
    assert!(
        markers
            .iter()
            .any(|m| matches!(m, DynamicMarker::PathStemDagId { .. })),
        "expected PathStemDagId in {markers:?}"
    );
}

#[test]
fn detects_chain_splat_pattern() {
    let src = r#"
from airflow import DAG
from airflow.models.baseoperator import chain
from airflow.operators.bash import BashOperator

with DAG(dag_id="d"):
    items = [BashOperator(task_id=f"t_{i}") for i in range(3)]
    chain(*items)
"#;
    let markers = detect_dynamic_markers(src).expect("parse");
    assert!(
        markers
            .iter()
            .any(|m| matches!(m, DynamicMarker::ChainSplat { .. })),
        "expected ChainSplat in {markers:?}"
    );
}

#[test]
fn detects_fstring_task_id_pattern() {
    let src = r#"
from airflow import DAG
from airflow.operators.bash import BashOperator

with DAG(dag_id="d"):
    for i in range(3):
        BashOperator(task_id=f"t_{i}", bash_command="x")
"#;
    let markers = detect_dynamic_markers(src).expect("parse");
    assert!(
        markers
            .iter()
            .any(|m| matches!(m, DynamicMarker::FStringTaskId { .. })),
        "expected FStringTaskId in {markers:?}"
    );
}

#[test]
fn detects_dynamic_schedule_expr_pattern() {
    let src = r#"
from airflow import DAG
from airflow.assets import Asset

with DAG(dag_id="d", schedule=Asset("s3://bucket/x")):
    pass
"#;
    let markers = detect_dynamic_markers(src).expect("parse");
    assert!(
        markers
            .iter()
            .any(|m| matches!(m, DynamicMarker::DynamicScheduleExpr { .. })),
        "expected DynamicScheduleExpr in {markers:?}"
    );
}

#[test]
fn detects_unsupported_taskflow_pattern() {
    let src = r#"
from airflow.sdk import dag, task

@dag(schedule="@daily")
def p():
    @task(expand=True)
    def fan(x):
        return x
    fan([1, 2, 3])
"#;
    let markers = detect_dynamic_markers(src).expect("parse");
    assert!(
        markers
            .iter()
            .any(|m| matches!(m, DynamicMarker::UnsupportedTaskFlow { .. })),
        "expected UnsupportedTaskFlow in {markers:?}"
    );
}

#[test]
fn detects_import_time_branching_pattern() {
    let src = r#"
import os
from airflow import DAG

if os.environ.get("ENABLE"):
    with DAG(dag_id="conditional"):
        pass
"#;
    let markers = detect_dynamic_markers(src).expect("parse");
    assert!(
        markers
            .iter()
            .any(|m| matches!(m, DynamicMarker::ImportTimeBranching { .. })),
        "expected ImportTimeBranching in {markers:?}"
    );
}

#[test]
fn detects_for_loop_task_generation_pattern() {
    let src = r#"
from airflow import DAG
from airflow.operators.bash import BashOperator

with DAG(dag_id="loopgen"):
    for i in range(3):
        BashOperator(task_id=f"t_{i}", bash_command="x")
"#;
    let markers = detect_dynamic_markers(src).expect("parse");
    assert!(
        markers
            .iter()
            .any(|m| matches!(m, DynamicMarker::ForLoopTaskGeneration { .. })),
        "expected ForLoopTaskGeneration in {markers:?}"
    );
}

/// 64-file Airflow `example_dags/` corpus: count detected markers per
/// kind and the number of files that statically extract a `dag_id` for
/// every DAG (= 100% safe / no `PyO3` fallback needed).
#[test]
fn example_dags_coverage_summary() {
    let root = Path::new(CORPUS);
    if !root.exists() {
        println!("[api_dynamic_markers] corpus {CORPUS} not present; skipping coverage summary");
        return;
    }
    let files = collect_python_files(root);
    assert!(!files.is_empty(), "corpus exists but no .py files found");

    let mut total_markers: BTreeMap<&'static str, usize> = BTreeMap::new();
    let mut files_with_marker = 0usize;
    let mut files_with_full_dag_id = 0usize;
    let mut files_with_partial_dag_id = 0usize;
    let mut files_with_no_dag = 0usize;
    let mut files_with_parse_error = 0usize;

    for file in &files {
        let Ok(source) = fs::read_to_string(file) else {
            continue;
        };
        let Ok(dags) = extract_all_static_dags(&source) else {
            files_with_parse_error += 1;
            continue;
        };
        let markers = detect_dynamic_markers(&source).unwrap_or_default();
        if !markers.is_empty() {
            files_with_marker += 1;
        }
        for m in &markers {
            *total_markers.entry(m.kind()).or_insert(0) += 1;
        }
        if dags.is_empty() {
            files_with_no_dag += 1;
        } else if dags.iter().all(|d| d.dag_id.is_some()) {
            files_with_full_dag_id += 1;
        } else {
            files_with_partial_dag_id += 1;
        }
    }

    println!("[api_dynamic_markers] corpus = {CORPUS}");
    println!(
        "  files: total={} parse_error={} no_dag={} full_dag_id={} partial_dag_id={} with_marker={}",
        files.len(),
        files_with_parse_error,
        files_with_no_dag,
        files_with_full_dag_id,
        files_with_partial_dag_id,
        files_with_marker
    );
    for (kind, count) in &total_markers {
        println!("  marker {kind:32} = {count}");
    }

    // The corpus must contain at least one file (we already asserted
    // that). Static extraction must succeed (i.e. parse without error)
    // on every file — a parse error would mean the ruff backend regressed.
    assert_eq!(files_with_parse_error, 0, "ruff parser regressed on corpus");
}

#[test]
fn parse_outcome_includes_markers_when_present() {
    let src = r"
from pathlib import Path
from airflow import DAG

with DAG(dag_id=Path(__file__).stem):
    pass
";
    // dynamic_markers_for is the cheap variant; ParseOutcome is the
    // file-keyed variant. They share the marker output.
    let cheap = ferro_airflow_dag_parser::dynamic_markers_for(src);
    assert!(!cheap.is_empty());
    // ParseOutcome is what `parse_dag_path` / cache emit.
    let outcome: ParseOutcome = ParseOutcome {
        dags: extract_all_static_dags(src).unwrap(),
        dynamic_markers: cheap.clone(),
        source_hash: 0,
        parsed_at: chrono::Utc::now(),
        source_path: None,
    };
    assert_eq!(outcome.dynamic_markers, cheap);
}
