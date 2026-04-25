// SPDX-License-Identifier: Apache-2.0
//! Phase 1 public API contract: [`extract_static_dag`].

#![cfg(feature = "parser-ruff")]

use ferro_airflow_dag_parser::common::{DagId, TaskId};
use ferro_airflow_dag_parser::extract_static_dag;

#[test]
fn returns_default_for_empty_source() {
    let dag = extract_static_dag("").expect("empty source parses");
    assert!(dag.dag_id.is_none());
    assert!(dag.task_ids.is_empty());
}

#[test]
fn picks_first_dag_when_multiple_present() {
    let src = r#"
from airflow import DAG

with DAG(dag_id="alpha"):
    pass

with DAG(dag_id="beta"):
    pass
"#;
    let dag = extract_static_dag(src).expect("parse");
    assert_eq!(dag.dag_id, Some(DagId::new("alpha").unwrap()));
}

#[test]
fn recovers_dag_id_task_ids_and_edges() {
    let src = r#"
from airflow import DAG
from airflow.operators.bash import BashOperator

with DAG(dag_id="example", schedule="@hourly", default_args={"owner": "u"}) as dag:
    a = BashOperator(task_id="a", bash_command="x")
    b = BashOperator(task_id="b", bash_command="x")
    a >> b
"#;
    let dag = extract_static_dag(src).expect("parse");
    assert_eq!(dag.dag_id, Some(DagId::new("example").unwrap()));
    assert_eq!(
        dag.task_ids,
        vec![TaskId::new("a").unwrap(), TaskId::new("b").unwrap()]
    );
    assert_eq!(dag.schedule.as_deref(), Some("@hourly"));
    assert!(dag.has_default_args);
    assert_eq!(
        dag.deps_edges,
        vec![(TaskId::new("a").unwrap(), TaskId::new("b").unwrap())]
    );
    let span = dag.source_span.expect("ruff backend records span");
    assert!(span.start_line > 0);
    assert!(span.end_line >= span.start_line);
}

#[test]
fn syntax_error_surfaces() {
    let err = extract_static_dag("def !!!").expect_err("syntax error");
    assert!(matches!(
        err,
        ferro_airflow_dag_parser::ParseError::Parse(_)
    ));
}
