// SPDX-License-Identifier: Apache-2.0
//! Phase 1 public API contract: invalid `dag_id` / `task_id` literals
//! surface as [`ParseError::InvalidIdentifier`] instead of being
//! silently accepted.
//!
//! Airflow caps `dag_id` and `task_id` at 250 characters and restricts
//! the charset to `[a-zA-Z0-9_\-\.]`. The metadata-DB FK joins and the
//! REST URL routes both depend on this rule, so violating it has to be
//! a hard error at parse time.

#![cfg(feature = "parser-ruff")]

use ferro_airflow_dag_parser::{ParseError, extract_static_dag};

#[test]
fn dag_id_with_slash_is_rejected() {
    let src = r#"
from airflow import DAG

with DAG(dag_id="bad/id"):
    pass
"#;
    let err = extract_static_dag(src).expect_err("must reject");
    match err {
        ParseError::InvalidIdentifier {
            kind,
            value,
            reason,
        } => {
            assert_eq!(kind, "dag_id");
            assert_eq!(value, "bad/id");
            assert!(
                reason.contains("invalid character") || reason.contains("contains invalid"),
                "unexpected reason: {reason}"
            );
        }
        other => panic!("expected InvalidIdentifier, got {other:?}"),
    }
}

#[test]
fn dag_id_too_long_is_rejected() {
    let long = "a".repeat(251);
    let src = format!(
        r#"
from airflow import DAG

with DAG(dag_id="{long}"):
    pass
"#
    );
    let err = extract_static_dag(&src).expect_err("must reject");
    match err {
        ParseError::InvalidIdentifier { kind, value, .. } => {
            assert_eq!(kind, "dag_id");
            assert_eq!(value.len(), 251);
        }
        other => panic!("expected InvalidIdentifier, got {other:?}"),
    }
}

#[test]
fn dag_id_empty_is_rejected() {
    let src = r#"
from airflow import DAG

with DAG(dag_id=""):
    pass
"#;
    let err = extract_static_dag(src).expect_err("must reject");
    assert!(matches!(err, ParseError::InvalidIdentifier { kind, .. } if kind == "dag_id"));
}

#[test]
fn task_id_with_invalid_char_is_rejected() {
    let src = r#"
from airflow import DAG
from airflow.operators.bash import BashOperator

with DAG(dag_id="ok"):
    BashOperator(task_id="bad task", bash_command="x")
"#;
    let err = extract_static_dag(src).expect_err("must reject");
    assert!(matches!(err, ParseError::InvalidIdentifier { kind, .. } if kind == "task_id"));
}

#[test]
fn safe_punctuation_is_accepted() {
    // Underscore, dash, dot, digits, mixed-case ASCII are all OK.
    let src = r#"
from airflow import DAG
from airflow.operators.bash import BashOperator

with DAG(dag_id="a-b.c_D-1"):
    BashOperator(task_id="t.A-1_x", bash_command="x")
"#;
    let dag = extract_static_dag(src).expect("must accept");
    assert_eq!(dag.dag_id.as_ref().unwrap().as_str(), "a-b.c_D-1");
    assert_eq!(dag.task_ids[0].as_str(), "t.A-1_x");
}

#[test]
fn parse_error_outranks_invalid_identifier() {
    // Syntax error always wins; identifier validation never runs.
    let err = extract_static_dag("def !!!").expect_err("syntax error");
    assert!(matches!(err, ParseError::Parse(_)));
}
