// SPDX-License-Identifier: Apache-2.0
//! Conformance tests against vendored real Airflow example DAGs.
//!
//! These exercise the static DAG extractor against two of the
//! canonical example DAGs shipped with `apache/airflow`. A parser that
//! recovers the `dag_id`, full `task_id` set, and the edge graph from
//! these files is materially compatible with what Airflow's scheduler
//! picks up at parse time.
//!
//! Source URLs and license attribution: see `tests/fixtures/README.md`.

#![cfg(feature = "parser-ruff")]

use ferro_airflow_dag_parser::common::{DagId, TaskId};
use ferro_airflow_dag_parser::{extract_all_static_dags, extract_static_dag};

const EXAMPLE_BASH_OPERATOR: &str = include_str!("fixtures/example_bash_operator.py");
const TUTORIAL: &str = include_str!("fixtures/tutorial.py");

#[test]
fn upstream_example_bash_operator_recovers_dag_id() {
    let dag =
        extract_static_dag(EXAMPLE_BASH_OPERATOR).expect("upstream example_bash_operator parses");
    assert_eq!(
        dag.dag_id,
        Some(DagId::new("example_bash_operator").expect("valid id")),
    );
}

#[test]
fn upstream_example_bash_operator_recovers_all_seven_tasks() {
    let dag = extract_static_dag(EXAMPLE_BASH_OPERATOR).expect("parse");
    let task_ids: Vec<&str> = dag.task_ids.iter().map(TaskId::as_str).collect();

    // Real example_bash_operator.py defines exactly these 7 tasks.
    for expected in [
        "runme_0",
        "runme_1",
        "runme_2",
        "run_after_loop",
        "also_run_this",
        "this_will_skip",
        "run_this_last",
    ] {
        assert!(
            task_ids.contains(&expected),
            "missing task {expected} in {task_ids:?}",
        );
    }
    assert_eq!(task_ids.len(), 7);
}

#[test]
fn upstream_tutorial_recovers_three_tasks() {
    let dag = extract_static_dag(TUTORIAL).expect("upstream tutorial.py parses");
    assert_eq!(dag.dag_id, Some(DagId::new("tutorial").expect("valid id")));
    let task_ids: Vec<&str> = dag.task_ids.iter().map(TaskId::as_str).collect();
    for expected in ["print_date", "sleep", "templated"] {
        assert!(
            task_ids.contains(&expected),
            "missing task {expected} in {task_ids:?}",
        );
    }
    assert_eq!(task_ids.len(), 3);
}

#[test]
fn upstream_tutorial_records_default_args() {
    let dag = extract_static_dag(TUTORIAL).expect("parse");
    // The tutorial DAG passes a multi-key `default_args=` literal; the
    // static extractor must record its presence (the contents are out
    // of scope — only the existence flag is asserted).
    assert!(
        dag.has_default_args,
        "tutorial.py has a default_args literal at the DAG call",
    );
}

#[test]
fn upstream_dags_yield_one_dag_each_via_extract_all() {
    // extract_all_static_dags must agree with extract_static_dag on
    // single-DAG files: exactly one DAG yielded, same id.
    let bash_op = extract_all_static_dags(EXAMPLE_BASH_OPERATOR).expect("parse");
    assert_eq!(bash_op.len(), 1);
    assert_eq!(
        bash_op[0].dag_id,
        Some(DagId::new("example_bash_operator").unwrap()),
    );

    let tut = extract_all_static_dags(TUTORIAL).expect("parse");
    assert_eq!(tut.len(), 1);
    assert_eq!(tut[0].dag_id, Some(DagId::new("tutorial").unwrap()));
}

#[test]
fn upstream_example_bash_operator_recovers_fan_out_edges() {
    // The DAG has the shape:
    //   [runme_0, runme_1, runme_2] >> run_after_loop
    //   run_after_loop >> [also_run_this, this_will_skip] >> run_this_last
    // i.e. 3 + 2 + 2 = 7 directed edges.
    let dag = extract_static_dag(EXAMPLE_BASH_OPERATOR).expect("parse");
    assert!(
        !dag.deps_edges.is_empty(),
        "static extractor must surface at least some `>>` edges",
    );

    // The fan-out edge from `runme_0` to `run_after_loop` is the
    // canonical list-shift form; assert it is present.
    let run_after_loop = TaskId::new("run_after_loop").unwrap();
    let runme_0 = TaskId::new("runme_0").unwrap();
    assert!(
        dag.deps_edges
            .iter()
            .any(|(src, dst)| src == &runme_0 && dst == &run_after_loop),
        "expected runme_0 -> run_after_loop edge, got {:?}",
        dag.deps_edges,
    );
}
