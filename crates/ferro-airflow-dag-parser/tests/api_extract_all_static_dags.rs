// SPDX-License-Identifier: Apache-2.0
//! Phase 1 public API contract: [`extract_all_static_dags`].

#![cfg(feature = "parser-ruff")]

use ferro_airflow_dag_parser::common::DagId;
use ferro_airflow_dag_parser::extract_all_static_dags;

#[test]
fn returns_empty_vec_when_no_dag_present() {
    let src = r"
import os

print('no dag here')
";
    let dags = extract_all_static_dags(src).expect("parse");
    assert!(dags.is_empty(), "got {dags:?}");
}

#[test]
fn returns_each_dag_in_source_order() {
    let src = r#"
from airflow import DAG

with DAG(dag_id="alpha"):
    pass

with DAG(dag_id="beta"):
    pass

with DAG(dag_id="gamma"):
    pass
"#;
    let dags = extract_all_static_dags(src).expect("parse");
    assert_eq!(dags.len(), 3);
    assert_eq!(dags[0].dag_id, Some(DagId::new("alpha").unwrap()));
    assert_eq!(dags[1].dag_id, Some(DagId::new("beta").unwrap()));
    assert_eq!(dags[2].dag_id, Some(DagId::new("gamma").unwrap()));
}

#[test]
fn recovers_dag_decorator_and_with_dag_in_one_file() {
    let src = r#"
from airflow import DAG
from airflow.sdk import dag, task

with DAG(dag_id="legacy"):
    pass

@dag(schedule="@daily")
def my_pipeline():
    @task
    def step():
        pass
"#;
    let dags = extract_all_static_dags(src).expect("parse");
    assert_eq!(dags.len(), 2);
    let ids: Vec<&str> = dags
        .iter()
        .filter_map(|d| {
            d.dag_id
                .as_ref()
                .map(ferro_airflow_dag_parser::common::DagId::as_str)
        })
        .collect();
    assert!(ids.contains(&"legacy"));
    assert!(ids.contains(&"my_pipeline"));
}

#[test]
fn each_dag_has_independent_task_ids() {
    let src = r#"
from airflow import DAG
from airflow.operators.bash import BashOperator

with DAG(dag_id="a"):
    BashOperator(task_id="a_t", bash_command="x")

with DAG(dag_id="b"):
    BashOperator(task_id="b_t", bash_command="x")
"#;
    let dags = extract_all_static_dags(src).expect("parse");
    assert_eq!(dags.len(), 2);
    assert_eq!(dags[0].task_ids.len(), 1);
    assert_eq!(dags[0].task_ids[0].as_str(), "a_t");
    assert_eq!(dags[1].task_ids.len(), 1);
    assert_eq!(dags[1].task_ids[0].as_str(), "b_t");
}
