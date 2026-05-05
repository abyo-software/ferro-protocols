// SPDX-License-Identifier: Apache-2.0
//! Extract a DAG's structure from Python source without invoking
//! `CPython`.
//!
//! Demonstrates the static-fast-path that the Ferro orchestrator uses
//! to skip `CPython` evaluation when a DAG file's structure can be
//! determined by AST inspection alone, plus the dynamic-fallback
//! markers that flag files that *can't* take the fast path.
//!
//! Run with:
//!
//! ```bash
//! cargo run --example extract_dag -p ferro-airflow-dag-parser
//! ```

use ferro_airflow_dag_parser::{
    detect_dynamic_markers, extract_all_static_dags, extract_static_dag,
};

const STATIC_DAG: &str = r#"
from airflow import DAG
from airflow.operators.bash import BashOperator
from airflow.operators.python import PythonOperator

with DAG(
    dag_id="etl_daily",
    schedule="@daily",
) as dag:
    extract = BashOperator(task_id="extract", bash_command="echo hi")
    transform = PythonOperator(task_id="transform", python_callable=lambda: None)
    load = BashOperator(task_id="load", bash_command="echo done")
    extract >> transform >> load
"#;

const DYNAMIC_DAG: &str = r"
from airflow import DAG
from pathlib import Path

# Dynamic dag_id derived from the filename — needs runtime evaluation.
with DAG(dag_id=Path(__file__).stem) as dag:
    pass
";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // ----- Static fast-path -----------------------------------------------

    let dag = extract_static_dag(STATIC_DAG)?;
    println!("static fast-path OK:");
    println!("  dag_id   = {:?}", dag.dag_id);
    println!("  schedule = {:?}", dag.schedule);
    println!("  task_ids = {:?}", dag.task_ids);
    println!("  edges    = {} dependency edges", dag.deps_edges.len());
    for (a, b) in &dag.deps_edges {
        println!("    {a} >> {b}");
    }

    // Multi-DAG file (one per `with DAG(...)` block).
    let all = extract_all_static_dags(STATIC_DAG)?;
    println!("file contains {} DAG(s)", all.len());

    // ----- Dynamic-fallback marker detection ------------------------------

    let markers = detect_dynamic_markers(DYNAMIC_DAG)?;
    println!("dynamic markers in second source: {} found", markers.len());
    for marker in &markers {
        println!("  -> {marker:?}");
    }
    if !markers.is_empty() {
        println!("orchestrator should route this file to the CPython fallback.");
    }

    Ok(())
}
