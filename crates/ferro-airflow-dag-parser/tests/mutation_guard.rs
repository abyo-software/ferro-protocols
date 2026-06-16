// SPDX-License-Identifier: Apache-2.0
//! Mutation-kill integration suite.
//!
//! Every test here drives the crate's PUBLIC API only
//! ([`extract_static_dag`], [`extract_all_static_dags`],
//! [`dynamic_markers_for`], [`parse_dag_path`], [`ParseCache`]) and pins
//! an exact observable output (extracted ids, error kinds, marker line /
//! column numbers, source hashes). Each assertion is chosen to die under
//! a specific `cargo mutants` mutation of the library source. The
//! library source is intentionally NOT edited (this crate is shared with
//! a concurrent session); the only artefacts are this file and the
//! sibling rationale doc.
//!
//! The crate's `parser-ruff` feature is the production default, so these
//! tests assume it is enabled (the public extractor functions exist only
//! under it).

#![cfg(feature = "parser-ruff")]

use std::io::Write as _;

use ferro_airflow_dag_parser::{
    DynamicMarker, ExtractedDag, ParseCache, ParseError, dynamic_markers_for,
    extract_all_static_dags, extract_static_dag, parse_dag_path,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn dag_id_of(dag: &ExtractedDag) -> Option<&str> {
    dag.dag_id
        .as_ref()
        .map(ferro_airflow_dag_parser::DagId::as_str)
}

fn task_id_strings(dag: &ExtractedDag) -> Vec<&str> {
    dag.task_ids
        .iter()
        .map(ferro_airflow_dag_parser::TaskId::as_str)
        .collect()
}

fn edge_strings(dag: &ExtractedDag) -> Vec<(&str, &str)> {
    dag.deps_edges
        .iter()
        .map(|(a, b)| (a.as_str(), b.as_str()))
        .collect()
}

/// Write `body` to a temp `.py` file and parse it through the on-disk
/// public entrypoint, returning the resulting outcome.
fn parse_temp(body: &str) -> ferro_airflow_dag_parser::ParseOutcome {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("dag.py");
    let mut f = std::fs::File::create(&path).expect("create");
    f.write_all(body.as_bytes()).expect("write");
    f.sync_all().ok();
    parse_dag_path(&path).expect("parse_dag_path")
}

// ===========================================================================
// common.rs — validate_safe_identifier `>` vs `>=` (250-char boundary)
// ===========================================================================

#[test]
fn dag_id_at_exact_max_len_is_accepted() {
    // 250 chars is exactly MAX_IDENTIFIER_LEN. `len > max_len` must be
    // false here; the `>=` mutant would reject this valid id.
    let id = "a".repeat(250);
    let src = format!("with DAG(dag_id=\"{id}\"):\n    pass\n");
    let dags = extract_all_static_dags(&src).expect("250-char dag_id must be accepted");
    assert_eq!(dags.len(), 1);
    assert_eq!(dag_id_of(&dags[0]), Some(id.as_str()));
}

#[test]
fn dag_id_one_over_max_len_is_rejected() {
    // 251 chars must trip `len > max_len`. Pins the boundary from the
    // other side so neither `>=` nor `<` style mutants survive.
    let id = "a".repeat(251);
    let src = format!("with DAG(dag_id=\"{id}\"):\n    pass\n");
    let err = extract_all_static_dags(&src).expect_err("251-char dag_id must be rejected");
    match err {
        ParseError::InvalidIdentifier { kind, reason, .. } => {
            assert_eq!(kind, "dag_id");
            assert!(
                reason.contains("251"),
                "reason should report length 251: {reason}"
            );
        }
        other => panic!("expected InvalidIdentifier, got {other:?}"),
    }
}

#[test]
fn task_id_at_exact_max_len_is_accepted() {
    let id = "t".repeat(250);
    let src = format!("with DAG(dag_id=\"d\"):\n    x = BashOperator(task_id=\"{id}\")\n");
    let dags = extract_all_static_dags(&src).expect("250-char task_id must be accepted");
    assert_eq!(task_id_strings(&dags[0]), vec![id.as_str()]);
}

// ===========================================================================
// panic_safe.rs — stack-safety caps (reached via the public extractor)
//
// The guard is two-layer (see `src/panic_safe.rs`): an iterative bracket
// pre-scan with cap `MAX_PARSE_NESTING_DEPTH = 256`, and a lexer pass with
// the combined-recursion cap `MAX_PARSE_RECURSION = 1024` that also catches
// non-bracket recursion (`not`/`await`/`**`/conditional/`lambda`/indent).
// These integration tests pin the observable boundary through the public
// API so a `>`/`>=`/`==` mutation of either cap comparison dies.
// ===========================================================================

#[test]
fn bracket_depth_exactly_at_cap_passes() {
    // Depth of exactly 256 (the bracket cap) must NOT be rejected
    // (`depth > cap` is false at 256). The `>=` mutant would reject this.
    let src = format!("x = {}1{}\n", "(".repeat(256), ")".repeat(256));
    // Either parses cleanly or fails for a *non-cap* reason — but it must
    // not be rejected by the nesting pre-screen.
    match extract_all_static_dags(&src) {
        Ok(_) => {}
        Err(ParseError::Parse(msg)) => {
            assert!(
                !msg.contains("nesting depth"),
                "depth-256 must not trip the nesting cap: {msg}"
            );
        }
        Err(other) => panic!("unexpected error: {other:?}"),
    }
}

#[test]
fn bracket_depth_one_over_cap_rejected() {
    // Depth 257 = first depth strictly greater than 256. Kills `> -> >=`
    // (which would admit 257) by requiring rejection here.
    let src = format!("x = {}1{}\n", "(".repeat(257), ")".repeat(257));
    let err = extract_all_static_dags(&src).expect_err("depth 257 must be rejected");
    match err {
        ParseError::Parse(msg) => assert!(
            msg.contains("nesting depth") && msg.contains("256"),
            "expected nesting-cap message: {msg}"
        ),
        other => panic!("expected Parse, got {other:?}"),
    }
}

#[test]
fn bracket_depth_far_over_cap_rejected() {
    // Depth 4096 (>> 256), unbalanced openers. Kills `> -> ==` (the `==`
    // variant only fires at exactly 257, so a depth-4096 input would slip
    // through it) — the real `>` operator rejects everything above 256.
    let src = "(".repeat(4096);
    let err = extract_all_static_dags(&src).expect_err("depth 4096 must be rejected");
    match err {
        ParseError::Parse(msg) => assert!(
            msg.contains("nesting depth"),
            "expected nesting-cap message: {msg}"
        ),
        other => panic!("expected Parse, got {other:?}"),
    }
}

#[test]
fn closing_brackets_reduce_depth_so_balanced_input_passes() {
    // 400 balanced open/close *pairs* never nested deeper than 1. Kills
    // the "delete match arm `)` `]` `}`" mutant: without the closing arm
    // depth would climb to 400 ( > 256 ) and be wrongly rejected.
    let mut src = String::from("x = ");
    for _ in 0..400 {
        src.push_str("(1)");
    }
    src.push('\n');
    match extract_all_static_dags(&src) {
        Ok(_) => {}
        Err(ParseError::Parse(msg)) => assert!(
            !msg.contains("nesting depth"),
            "balanced brackets must not trip the cap: {msg}"
        ),
        Err(other) => panic!("unexpected error: {other:?}"),
    }
}

#[test]
fn recursion_run_exactly_at_cap_passes() {
    // A run of exactly 1024 `-` (combined recursion depth = 1024) must NOT
    // be rejected (`max > cap` false at 1024). Kills `> -> >=`.
    let src = format!("x = {}1\n", "-".repeat(1024));
    match extract_all_static_dags(&src) {
        Ok(_) => {}
        Err(ParseError::Parse(msg)) => assert!(
            !msg.contains("recursion depth"),
            "run-1024 must not trip the recursion cap: {msg}"
        ),
        Err(other) => panic!("unexpected error: {other:?}"),
    }
}

#[test]
fn recursion_run_far_over_cap_rejected() {
    // Run of 2000 `~` (>> 1024). The lexer-pass recursion cap rejects it
    // even though no bracket is present — the vector a bracket-only cap
    // would miss entirely.
    let src = format!("{}x\n", "~".repeat(2000));
    let err = extract_all_static_dags(&src).expect_err("run 2000 must be rejected");
    match err {
        ParseError::Parse(msg) => assert!(
            msg.contains("recursion depth"),
            "expected recursion-cap message: {msg}"
        ),
        other => panic!("expected Parse, got {other:?}"),
    }
}

#[test]
fn keyword_not_chain_rejected_by_recursion_cap() {
    // `not not not …` recurses one frame per `not` with NO bracket and NO
    // single-byte operator — the exact shape the old bracket+byte caps
    // missed. The lexer pass must reject it as a graceful skip-file error.
    let src = format!("x = {}True\n", "not ".repeat(3000));
    let err = extract_all_static_dags(&src).expect_err("not-chain must be rejected");
    match err {
        ParseError::Parse(msg) => assert!(
            msg.contains("recursion depth"),
            "expected recursion-cap message: {msg}"
        ),
        other => panic!("expected Parse, got {other:?}"),
    }
}

// ===========================================================================
// cache.rs — hash_source `^=` vs `|=`
// ===========================================================================

#[test]
fn source_hash_matches_exact_fxhash_xor_value() {
    // The FNV-1a-style hash XORs each byte into the accumulator. Pinning
    // the exact value of a known source kills `^= -> |=` (which only
    // ever sets bits and produces a different digest). The body bytes
    // below are exactly "with DAG(dag_id=\"h\"):\n    pass\n" (31 bytes);
    // the XOR digest is precomputed independently.
    let body = "with DAG(dag_id=\"h\"):\n    pass\n";
    let outcome = parse_temp(body);
    assert_eq!(
        outcome.source_hash, 4_461_567_911_320_149_738_u64,
        "hash_source XOR digest drifted"
    );
}

#[test]
fn distinct_sources_hash_differently() {
    // Two sources differing only in the dag_id literal must hash to
    // different values — reinforces that the per-byte mixing is live.
    let a = parse_temp("with DAG(dag_id=\"aa\"):\n    pass\n");
    let b = parse_temp("with DAG(dag_id=\"bb\"):\n    pass\n");
    assert_ne!(a.source_hash, b.source_hash);
}

// ===========================================================================
// ruff_impl.rs — extractor walker, decorator/callable matching, edges
// ===========================================================================

#[test]
fn extract_recovers_full_dag_shape() {
    // Drives the happy path: dag_id, ordered task_ids, schedule,
    // default_args, a `>>` edge, and a span. Broadly anchors the walker
    // (`extract -> Ok(Default::default())` mutant dies here).
    let src = r#"
from airflow import DAG
from airflow.operators.bash import BashOperator

with DAG(dag_id="full", schedule="@daily", default_args={"a": 1}) as dag:
    a = BashOperator(task_id="a", bash_command="echo a")
    b = BashOperator(task_id="b", bash_command="echo b")
    a >> b
"#;
    let dag = extract_static_dag(src).expect("parse");
    assert_eq!(dag_id_of(&dag), Some("full"));
    assert_eq!(task_id_strings(&dag), vec!["a", "b"]);
    assert_eq!(dag.schedule.as_deref(), Some("@daily"));
    assert!(dag.has_default_args);
    assert_eq!(edge_strings(&dag), vec![("a", "b")]);
    assert!(dag.source_span.is_some());
}

#[test]
fn ann_assign_target_collects_task() {
    // `x: BashOperator = BashOperator(task_id="annotated")` exercises the
    // `Stmt::AnnAssign { value: Some(value) }` walker arm. Deleting that
    // arm drops the task.
    let src = r#"
from airflow import DAG
from airflow.operators.bash import BashOperator

with DAG(dag_id="ann"):
    x: BashOperator = BashOperator(task_id="annotated", bash_command="echo")
"#;
    let dag = extract_static_dag(src).expect("parse");
    assert_eq!(task_id_strings(&dag), vec!["annotated"]);
}

#[test]
fn nested_class_body_is_walked_for_dags() {
    // A `with DAG(...)` nested inside an `if:` block must still be
    // discovered — kills "delete match arm ClassDef|If|For|While|Try"
    // in the walker's `visit_stmt`.
    let src = r#"
from airflow import DAG
from airflow.operators.bash import BashOperator

if True:
    with DAG(dag_id="under_if"):
        a = BashOperator(task_id="t", bash_command="echo")
"#;
    let dags = extract_all_static_dags(src).expect("parse");
    assert_eq!(dags.len(), 1);
    assert_eq!(dag_id_of(&dags[0]), Some("under_if"));
    assert_eq!(task_id_strings(&dags[0]), vec!["t"]);
}

#[test]
fn set_downstream_records_directed_edge() {
    // `a.set_downstream(b)` yields edge (a, b). Exercises the
    // `attr == "set_downstream"` comparison (kills `== -> !=`) and the
    // `resolve_to_task_id` Call/Attribute arms.
    let src = r#"
from airflow import DAG
from airflow.operators.bash import BashOperator

with DAG(dag_id="setters"):
    a = BashOperator(task_id="a", bash_command="echo")
    b = BashOperator(task_id="b", bash_command="echo")
    a.set_downstream(b)
"#;
    let dag = extract_static_dag(src).expect("parse");
    assert_eq!(edge_strings(&dag), vec![("a", "b")]);
}

#[test]
fn set_upstream_records_reversed_edge() {
    // `a.set_upstream(b)` yields edge (b, a) — the *else* branch of the
    // `set_downstream` comparison. Confirms the `==` direction split.
    let src = r#"
from airflow import DAG
from airflow.operators.bash import BashOperator

with DAG(dag_id="setters2"):
    a = BashOperator(task_id="a", bash_command="echo")
    b = BashOperator(task_id="b", bash_command="echo")
    a.set_upstream(b)
"#;
    let dag = extract_static_dag(src).expect("parse");
    assert_eq!(edge_strings(&dag), vec![("b", "a")]);
}

#[test]
fn duplicate_edges_are_deduplicated() {
    // Two identical `a >> b` lines must collapse to a single edge. Kills
    // the `push_unique_edge` comparison mutants (`&& -> ||`,
    // `== -> !=` on either tuple element): any of those would either
    // duplicate the edge or drop it.
    let src = r#"
from airflow import DAG
from airflow.operators.bash import BashOperator

with DAG(dag_id="dedup"):
    a = BashOperator(task_id="a", bash_command="echo")
    b = BashOperator(task_id="b", bash_command="echo")
    a >> b
    a >> b
"#;
    let dag = extract_static_dag(src).expect("parse");
    assert_eq!(edge_strings(&dag), vec![("a", "b")]);
}

#[test]
fn distinct_edges_sharing_an_endpoint_are_all_kept() {
    // `a >> b` and `a >> c` share the upstream node. Kills
    // `push_unique_edge`'s `&& -> ||` (which would treat the second edge
    // as a duplicate because the first element matches) and the
    // `== -> !=` mutants on the individual element comparisons.
    let src = r#"
from airflow import DAG
from airflow.operators.bash import BashOperator

with DAG(dag_id="fanout"):
    a = BashOperator(task_id="a", bash_command="echo")
    b = BashOperator(task_id="b", bash_command="echo")
    c = BashOperator(task_id="c", bash_command="echo")
    a >> b
    a >> c
"#;
    let dag = extract_static_dag(src).expect("parse");
    assert_eq!(edge_strings(&dag), vec![("a", "b"), ("a", "c")]);
}

#[test]
fn dag_callable_via_attribute_is_recognized() {
    // `with airflow.DAG(dag_id=...)` — the callee is an Attribute, not a
    // bare Name. Kills "delete match arm Expr::Attribute in
    // is_dag_callable" and the `-> true` short-circuit (a non-DAG
    // attribute below must NOT be picked up).
    let src = r#"
import airflow

with airflow.DAG(dag_id="via_attr"):
    pass
"#;
    let dags = extract_all_static_dags(src).expect("parse");
    assert_eq!(dags.len(), 1);
    assert_eq!(dag_id_of(&dags[0]), Some("via_attr"));
}

#[test]
fn non_dag_attribute_call_is_not_a_dag() {
    // `with contextlib.suppress(Exception):` must yield zero DAGs. Kills
    // `is_dag_callable -> true` (which would treat every call as a DAG).
    let src = r"
import contextlib

with contextlib.suppress(Exception):
    pass
";
    let dags = extract_all_static_dags(src).expect("parse");
    assert!(
        dags.is_empty(),
        "non-DAG context manager produced DAGs: {dags:?}"
    );
}

#[test]
fn dag_decorator_via_attribute_is_recognized() {
    // `@airflow.sdk.dag(...)` — the decorator callee is an Attribute.
    // Kills the `match_dag_decorator` attribute guard mutants and the
    // "delete Attribute arm".
    let src = r#"
import airflow

@airflow.dag(schedule="@daily")
def attr_pipeline():
    pass
"#;
    let dags = extract_all_static_dags(src).expect("parse");
    assert_eq!(dags.len(), 1);
    assert_eq!(dag_id_of(&dags[0]), Some("attr_pipeline"));
}

#[test]
fn bare_name_decorator_that_is_not_dag_is_ignored() {
    // `@functools.cache def f(): ...` must NOT register a DAG. Kills the
    // `match_dag_decorator` guard `DAG_DECORATOR_NAMES.contains -> true`
    // mutants (which would treat any decorator as `@dag`).
    let src = r"
import functools

@functools.cache
def helper():
    pass
";
    let dags = extract_all_static_dags(src).expect("parse");
    assert!(
        dags.is_empty(),
        "non-dag decorator produced a DAG: {dags:?}"
    );
}

#[test]
fn task_decorator_via_attribute_collects_function_name() {
    // `@airflow.task` inside a `@dag` body registers a task. Kills the
    // `is_task_decorator` `-> true` mutant and its Attribute/Call inner
    // arms.
    let src = r#"
import airflow

@airflow.dag(schedule="@daily")
def deco_pipeline():
    @airflow.task
    def step():
        pass
    step()
"#;
    let dags = extract_all_static_dags(src).expect("parse");
    assert_eq!(dags.len(), 1);
    assert_eq!(task_id_strings(&dags[0]), vec!["step"]);
}

#[test]
fn non_task_decorated_function_is_not_a_task() {
    // A plain helper function inside the @dag body must NOT become a
    // task. Kills `is_task_decorator -> true`.
    let src = r#"
from airflow.sdk import dag, task

@dag(schedule="@daily")
def pipe():
    @task
    def real_task():
        pass
    def plain_helper():
        pass
    real_task()
"#;
    let dags = extract_all_static_dags(src).expect("parse");
    assert_eq!(task_id_strings(&dags[0]), vec!["real_task"]);
}

#[test]
fn schedule_stringifies_each_literal_kind() {
    // stringify_expr arms: None, bool, number, Name, Attribute, Call.
    // Each schedule= value below pins the exact recovered string so
    // deleting any one stringify arm changes an observable output.
    let cases = [
        ("None", "None"),
        ("True", "true"),
        ("5", "5"),
        ("legacy", "legacy"),
        ("module.timetable", "module.timetable"),
        ("Timetable()", "Timetable(...)"),
    ];
    for (expr, expected) in cases {
        let src = format!("with DAG(dag_id=\"s\", schedule={expr}):\n    pass\n");
        let dag = extract_static_dag(&src).expect("parse");
        assert_eq!(
            dag.schedule.as_deref(),
            Some(expected),
            "schedule={expr} should stringify to {expected:?}"
        );
    }
}

#[test]
fn ruff_impl_extract_returns_first_dag_not_default() {
    // `ruff_impl::extract` is part of the crate's public surface
    // (`pub mod ruff_impl`). It returns the FIRST extracted DAG, which
    // must carry the real dag_id — not the `Default::default()` empty
    // DAG the `extract -> Ok(Default::default())` mutant would yield.
    let src = "with DAG(dag_id=\"first\"):\n    pass\n";
    let dag = ferro_airflow_dag_parser::ruff_impl::extract(src).expect("parse");
    assert_eq!(dag_id_of(&dag), Some("first"));
}

// ===========================================================================
// dynamic_markers.rs — marker visitor, line/col, callable matching
// ===========================================================================

#[test]
fn path_stem_marker_reports_exact_line_and_col() {
    // The marker line/col come from MarkerVisitor::line_col. Pinning the
    // exact (line, col) kills the `line_col -> (0,0)|(0,1)|(1,0)|(1,1)`
    // constant-replacement mutants.
    let src = "from airflow import DAG\nwith DAG(dag_id=Path(__file__).stem):\n    pass\n";
    let markers = dynamic_markers_for(src);
    let m = markers
        .iter()
        .find_map(|m| match m {
            DynamicMarker::PathStemDagId { line, col } => Some((*line, *col)),
            _ => None,
        })
        .expect("PathStemDagId marker");
    // `Path(__file__).stem` begins at line 2, col 17 (1-indexed): the
    // `with DAG(dag_id=` prefix is 16 chars, so the value starts at 17.
    assert_eq!(m, (2, 17), "PathStemDagId line/col drifted");
}

#[test]
fn fstring_task_id_marker_reports_exact_line_and_rendering() {
    // Exercises render_fstring (literal + interpolation) and the
    // FStringTaskId line/col. Pins both the rendered source and the
    // line, killing `render_fstring -> String::new()|"xyzzy"` and the
    // visitor line/col constant mutants.
    let src = "from airflow import DAG\nwith DAG(dag_id=\"d\"):\n    for i in range(2):\n        BashOperator(task_id=f\"t_{i}\")\n";
    let markers = dynamic_markers_for(src);
    let (line, source) = markers
        .iter()
        .find_map(|m| match m {
            DynamicMarker::FStringTaskId { line, source, .. } => Some((*line, source.clone())),
            _ => None,
        })
        .expect("FStringTaskId marker");
    assert_eq!(line, 4, "f-string marker line drifted");
    assert_eq!(source, "t_{…}", "f-string rendering drifted");
}

#[test]
fn chain_splat_inside_dag_is_flagged() {
    // `chain(*items)` inside a DAG with in_dag_ctx > 0 must flag
    // ChainSplat. Anchors `callee_is_chain_helper`, `visit_call_args`,
    // and the `in_dag_ctx > 0 && callee_is_chain_helper` guard.
    let src = r#"
from airflow import DAG
from airflow.models.baseoperator import chain

with DAG(dag_id="cs"):
    chain(*items)
"#;
    let markers = dynamic_markers_for(src);
    assert!(
        markers
            .iter()
            .any(|m| matches!(m, DynamicMarker::ChainSplat { .. })),
        "chain splat inside a DAG must be flagged: {markers:?}"
    );
}

#[test]
fn chain_splat_outside_dag_is_not_flagged() {
    // The same `chain(*items)` at module scope (in_dag_ctx == 0) must
    // NOT flag. Kills BOTH `> -> >=` (which would fire at depth 0) and
    // `&& -> ||` (which would fire because the helper name matches) in
    // visit_call.
    let src = r"
from airflow.models.baseoperator import chain

chain(*items)
";
    let markers = dynamic_markers_for(src);
    assert!(
        !markers
            .iter()
            .any(|m| matches!(m, DynamicMarker::ChainSplat { .. })),
        "chain splat at module scope must NOT be flagged: {markers:?}"
    );
}

#[test]
fn chain_helper_via_attribute_is_recognized() {
    // `airflow.models.baseoperator.chain(*x)` — Attribute callee. Kills
    // the `callee_is_chain_helper -> true` mutant and "delete Attribute
    // arm" (paired with the module-scope negative above).
    let src = r#"
import airflow

with DAG(dag_id="cs2"):
    airflow.chain(*items)
"#;
    let markers = dynamic_markers_for(src);
    assert!(
        markers
            .iter()
            .any(|m| matches!(m, DynamicMarker::ChainSplat { .. })),
        "attribute chain helper must be flagged: {markers:?}"
    );
}

#[test]
fn for_loop_operator_construction_is_flagged_via_attribute() {
    // `for ...: x = module.BashOperator(...)` inside a DAG flags
    // ForLoopTaskGeneration. The operator callee is an Attribute, so
    // this kills `is_operator_constructor -> true` (paired with the
    // negative below) and its Attribute arm.
    let src = r#"
from airflow import DAG

with DAG(dag_id="loopgen"):
    for i in range(3):
        x = airflow.BashOperator(task_id="t")
"#;
    let markers = dynamic_markers_for(src);
    assert!(
        markers
            .iter()
            .any(|m| matches!(m, DynamicMarker::ForLoopTaskGeneration { .. })),
        "operator constructed in a for-loop must be flagged: {markers:?}"
    );
}

#[test]
fn for_loop_non_operator_call_is_not_flagged() {
    // A non-operator call (`print(i)`) in the loop body must NOT flag.
    // Kills `is_operator_constructor -> true`.
    let src = r#"
from airflow import DAG

with DAG(dag_id="loopgen2"):
    for i in range(3):
        print(i)
"#;
    let markers = dynamic_markers_for(src);
    assert!(
        !markers
            .iter()
            .any(|m| matches!(m, DynamicMarker::ForLoopTaskGeneration { .. })),
        "non-operator loop call must NOT flag: {markers:?}"
    );
}

#[test]
fn import_time_branching_under_nonconstant_if_is_flagged() {
    // `if os.environ.get(...): with DAG(...)` — non-constant test flags
    // ImportTimeBranching. Kills `is_constant_bool -> false` (which
    // would suppress the marker by treating the test as constant).
    let src = r#"
import os
from airflow import DAG

if os.environ.get("ENABLE"):
    with DAG(dag_id="conditional"):
        pass
"#;
    let markers = dynamic_markers_for(src);
    assert!(
        markers
            .iter()
            .any(|m| matches!(m, DynamicMarker::ImportTimeBranching { .. })),
        "DAG under non-constant if must flag: {markers:?}"
    );
}

#[test]
fn constant_if_guarding_dag_is_not_branching() {
    // `if True: with DAG(...)` — a constant test must NOT flag
    // ImportTimeBranching. Kills `is_constant_bool -> false`.
    let src = r#"
from airflow import DAG

if True:
    with DAG(dag_id="always"):
        pass
"#;
    let markers = dynamic_markers_for(src);
    assert!(
        !markers
            .iter()
            .any(|m| matches!(m, DynamicMarker::ImportTimeBranching { .. })),
        "DAG under constant `if True` must NOT flag branching: {markers:?}"
    );
}

#[test]
fn dynamic_schedule_marker_reports_exact_line() {
    // `schedule=Asset(...)` flags DynamicScheduleExpr. Pins the exact
    // line so the MarkerVisitor::line_col constant mutants die on this
    // path too, and confirms the dynamic-schedule branch fires.
    let src = "from airflow import DAG\nwith DAG(dag_id=\"d\", schedule=Asset(\"x\")):\n    pass\n";
    let markers = dynamic_markers_for(src);
    let line = markers
        .iter()
        .find_map(|m| match m {
            DynamicMarker::DynamicScheduleExpr { line, .. } => Some(*line),
            _ => None,
        })
        .expect("DynamicScheduleExpr marker");
    assert_eq!(line, 2, "dynamic schedule marker line drifted");
}

#[test]
fn taskflow_expand_decorator_is_flagged_but_bare_task_is_not() {
    // `@task(expand=True)` flags UnsupportedTaskFlow; a bare `@task`
    // does not. Kills `task_decorator_is_dynamic -> true`,
    // `delete ! in task_decorator_is_dynamic`, and
    // `is_task_decorator_call -> true`.
    let dynamic = r#"
from airflow.sdk import dag, task

@dag(schedule="@daily")
def p():
    @task(expand=True)
    def fan(x):
        return x
    fan([1])
"#;
    let bare = r#"
from airflow.sdk import dag, task

@dag(schedule="@daily")
def q():
    @task
    def step():
        pass
    step()
"#;
    let dyn_markers = dynamic_markers_for(dynamic);
    assert!(
        dyn_markers
            .iter()
            .any(|m| matches!(m, DynamicMarker::UnsupportedTaskFlow { .. })),
        "@task(expand=True) must flag: {dyn_markers:?}"
    );
    let bare_markers = dynamic_markers_for(bare);
    assert!(
        !bare_markers
            .iter()
            .any(|m| matches!(m, DynamicMarker::UnsupportedTaskFlow { .. })),
        "bare @task must NOT flag UnsupportedTaskFlow: {bare_markers:?}"
    );
}

#[test]
fn taskflow_decorator_call_with_only_positional_arg_is_dynamic() {
    // `@task("group")` (a positional arg, no expand/partial kwarg) is
    // dynamic because `args` is non-empty. Kills the
    // `task_decorator_is_dynamic` `|| -> &&` style and the args-empty
    // short-circuit by exercising the left operand alone.
    let src = r#"
from airflow.sdk import dag, task

@dag(schedule="@daily")
def p():
    @task("positional")
    def step():
        pass
    step()
"#;
    let markers = dynamic_markers_for(src);
    assert!(
        markers
            .iter()
            .any(|m| matches!(m, DynamicMarker::UnsupportedTaskFlow { .. })),
        "@task('positional') must flag UnsupportedTaskFlow: {markers:?}"
    );
}

#[test]
fn while_and_try_bodies_are_walked_for_dag_context() {
    // A `for`-loop operator construction nested inside a `while` and a
    // `try` body must still be discovered, exercising the
    // `Stmt::While` and `Stmt::Try | Stmt::ClassDef` visitor arms.
    // Deleting either arm stops the walk and drops the marker.
    let while_src = r#"
from airflow import DAG

with DAG(dag_id="w"):
    while running:
        for i in range(3):
            x = BashOperator(task_id="t")
"#;
    let try_src = r#"
from airflow import DAG

with DAG(dag_id="t"):
    try:
        for i in range(3):
            x = BashOperator(task_id="t")
    except Exception:
        pass
"#;
    assert!(
        dynamic_markers_for(while_src)
            .iter()
            .any(|m| matches!(m, DynamicMarker::ForLoopTaskGeneration { .. })),
        "marker under a while body must survive"
    );
    assert!(
        dynamic_markers_for(try_src)
            .iter()
            .any(|m| matches!(m, DynamicMarker::ForLoopTaskGeneration { .. })),
        "marker under a try body must survive"
    );
}

#[test]
fn assign_value_inside_dag_is_walked_for_markers() {
    // `dynamic = chain(*items)` (an assignment whose VALUE is a flagged
    // call) must surface ChainSplat. Exercises the `Stmt::Assign` arm of
    // the marker visitor (kills its deletion / the `-= -> +=|/=` ctx
    // bookkeeping mutants, which would leave in_dag_ctx wrong and
    // suppress or spuriously emit the marker).
    let src = r#"
from airflow import DAG
from airflow.models.baseoperator import chain

with DAG(dag_id="asg"):
    edges = chain(*items)
"#;
    let markers = dynamic_markers_for(src);
    assert!(
        markers
            .iter()
            .any(|m| matches!(m, DynamicMarker::ChainSplat { .. })),
        "chain splat on the RHS of an assignment must flag: {markers:?}"
    );
}

#[test]
fn ann_assign_value_inside_dag_is_walked_for_markers() {
    // `edges: list = chain(*items)` — an annotated assignment whose
    // VALUE is a flagged call must surface ChainSplat. Exercises the
    // marker visitor's `Stmt::AnnAssign { value: Some(value) }` arm;
    // deleting that arm drops the marker.
    let src = r"
from airflow import DAG
from airflow.models.baseoperator import chain

with DAG(dag_id='ann_asg'):
    edges: list = chain(*items)
";
    let markers = dynamic_markers_for(src);
    assert!(
        markers
            .iter()
            .any(|m| matches!(m, DynamicMarker::ChainSplat { .. })),
        "chain splat on the RHS of an annotated assignment must flag: {markers:?}"
    );
}

#[test]
fn setter_arg_resolved_through_call_and_attribute() {
    // `@task` functions `up`/`down` registered as aliases; the edge is
    // declared with `up().set_downstream(down.output)`. The setter
    // operands are a Call (`up()`) and an Attribute (`down.output`), so
    // resolving them exercises the `resolve_to_task_id` Call (301) and
    // Attribute (302) arms — deleting either drops the edge.
    let src = r"
from airflow.sdk import dag, task

@dag(schedule='@daily')
def pipe():
    @task
    def up():
        pass
    @task
    def down():
        pass
    up().set_downstream(down.output)
";
    let dag = extract_static_dag(src).expect("parse");
    assert_eq!(edge_strings(&dag), vec![("up", "down")]);
}

#[test]
fn dag_context_does_not_leak_past_the_with_block() {
    // After the `with DAG(...)` block closes, in_dag_ctx must return to
    // 0, so a `chain(*items)` AFTER the block must NOT flag. This kills
    // the `in_dag_ctx -= 1 -> += 1 | /= 1` mutants on the with/function
    // exit: if the decrement is wrong the context stays "open" and the
    // trailing chain splat would be flagged.
    let src = r#"
from airflow import DAG
from airflow.models.baseoperator import chain

with DAG(dag_id="scoped"):
    pass

chain(*items)
"#;
    let markers = dynamic_markers_for(src);
    assert!(
        !markers
            .iter()
            .any(|m| matches!(m, DynamicMarker::ChainSplat { .. })),
        "chain splat after the DAG block must NOT flag (context leaked): {markers:?}"
    );
}

#[test]
fn dag_decorator_context_does_not_leak_past_the_function() {
    // Same scope-leak check for the `@dag def` path: a `chain(*items)`
    // after the decorated function must NOT flag. Kills the function-exit
    // `in_dag_ctx -= 1 -> += 1 | /= 1` mutants.
    let src = r#"
from airflow.sdk import dag
from airflow.models.baseoperator import chain

@dag(schedule="@daily")
def scoped():
    pass

chain(*items)
"#;
    let markers = dynamic_markers_for(src);
    assert!(
        !markers
            .iter()
            .any(|m| matches!(m, DynamicMarker::ChainSplat { .. })),
        "chain splat after the @dag function must NOT flag: {markers:?}"
    );
}

// ===========================================================================
// dynamic_markers.rs — second-wave targeted negatives for the DAG-detection
// helpers reached ONLY via `dynamic_markers_for` (these helpers are a
// SEPARATE copy from the identically-named ones in ruff_impl.rs; the
// `extract_*` tests above kill the ruff_impl copy but leave these alive).
// Every test pairs a positive (marker must fire) with a negative whose
// shape looks similar but must NOT fire, so an always-`true` detector or a
// deleted match arm changes the asserted marker set.
// ===========================================================================

/// Convenience: does the marker set contain a `ChainSplat`?
fn has_chain_splat(src: &str) -> bool {
    dynamic_markers_for(src)
        .iter()
        .any(|m| matches!(m, DynamicMarker::ChainSplat { .. }))
}

/// Convenience: does the marker set contain an `UnsupportedTaskFlow`?
fn has_unsupported_taskflow(src: &str) -> bool {
    dynamic_markers_for(src)
        .iter()
        .any(|m| matches!(m, DynamicMarker::UnsupportedTaskFlow { .. }))
}

#[test]
fn dyn_non_dag_with_block_does_not_open_dag_context() {
    // dynamic_markers::is_dag_callable (line 419). A non-DAG `with` block
    // (`with open(...)`) whose body contains `chain(*items)` must NOT flag
    // ChainSplat, because `is_dag_callable` returns false so in_dag_ctx
    // stays 0. The `is_dag_callable -> true` mutant (420) would open the
    // context for EVERY `with` callee and spuriously flag the splat.
    let non_dag = r#"
from airflow.models.baseoperator import chain

with open("f") as fh:
    chain(*items)
"#;
    assert!(
        !has_chain_splat(non_dag),
        "chain splat inside a non-DAG `with` must NOT flag (is_dag_callable wrongly true): {:?}",
        dynamic_markers_for(non_dag)
    );
    // Positive companion: a real bare-Name `with DAG(...)` opens the
    // context and the same splat flags — confirms the detector path is live.
    let bare_dag = r#"
from airflow import DAG
from airflow.models.baseoperator import chain

with DAG(dag_id="ctx"):
    chain(*items)
"#;
    assert!(
        has_chain_splat(bare_dag),
        "bare DAG `with` must flag the splat"
    );
}

#[test]
fn dyn_dag_callable_via_attribute_opens_context() {
    // dynamic_markers::is_dag_callable Attribute arm (line 422). A
    // `with airflow.DAG(...)` opens the context via the Attribute arm; the
    // contained `chain(*items)` must flag. Deleting the Attribute arm
    // leaves in_dag_ctx at 0 and the splat would be dropped. Paired with
    // the non-DAG negative above so `-> true` cannot satisfy both.
    let attr_dag = r#"
import airflow
from airflow.models.baseoperator import chain

with airflow.DAG(dag_id="via_attr"):
    chain(*items)
"#;
    assert!(
        has_chain_splat(attr_dag),
        "attribute-callee DAG must open the context and flag the splat: {:?}",
        dynamic_markers_for(attr_dag)
    );
}

#[test]
fn dyn_non_dag_decorator_does_not_open_dag_context() {
    // dynamic_markers::match_dag_decorator (line 427). A `@functools.cache`
    // decorated function whose body contains `chain(*items)` must NOT flag
    // ChainSplat — the decorator is not `@dag`, so in_dag_ctx stays 0. The
    // `match_dag_decorator -> true` mutant (428) would open the context for
    // any decorated function and spuriously flag the splat.
    let non_dag = r"
import functools
from airflow.models.baseoperator import chain

@functools.cache
def helper():
    chain(*items)
";
    assert!(
        !has_chain_splat(non_dag),
        "chain splat under a non-@dag decorator must NOT flag (match_dag_decorator wrongly true): {:?}",
        dynamic_markers_for(non_dag)
    );
    // Positive companion: a bare-Name `@dag` decorator opens the context.
    let dag_deco = r#"
from airflow.sdk import dag
from airflow.models.baseoperator import chain

@dag(schedule="@daily")
def pipe():
    chain(*items)
"#;
    assert!(
        has_chain_splat(dag_deco),
        "@dag decorator must open the context"
    );
}

#[test]
fn dyn_dag_decorator_via_attribute_opens_context() {
    // dynamic_markers::match_dag_decorator Attribute arm (line 431). An
    // `@airflow.dag(...)` decorator (callee inside the decorator Call is an
    // Attribute) opens the context via the Attribute arm; the contained
    // `chain(*items)` must flag. Deleting the Attribute arm leaves
    // in_dag_ctx at 0 and the splat is dropped.
    let attr_deco = r#"
import airflow
from airflow.models.baseoperator import chain

@airflow.dag(schedule="@daily")
def pipe():
    chain(*items)
"#;
    assert!(
        has_chain_splat(attr_deco),
        "attribute @airflow.dag decorator must open the context and flag the splat: {:?}",
        dynamic_markers_for(attr_deco)
    );
}

#[test]
fn dyn_non_chain_splat_call_inside_dag_is_not_flagged() {
    // dynamic_markers::callee_is_chain_helper (line 439). Inside a real DAG
    // context, a SPLAT call to a non-chain helper (`schedule_tasks(*items)`)
    // must NOT flag ChainSplat. The `callee_is_chain_helper -> true` mutant
    // (440) would flag every splat call regardless of callee name.
    let src = r#"
from airflow import DAG

with DAG(dag_id="nc"):
    schedule_tasks(*items)
"#;
    assert!(
        !has_chain_splat(src),
        "splat to a non-chain helper inside a DAG must NOT flag (callee_is_chain_helper wrongly true): {:?}",
        dynamic_markers_for(src)
    );
    // Positive companion: the real `chain(*items)` in the same context flags.
    let chain_src = r#"
from airflow import DAG
from airflow.models.baseoperator import chain

with DAG(dag_id="nc2"):
    chain(*items)
"#;
    assert!(has_chain_splat(chain_src), "real chain(*items) must flag");
}

#[test]
fn dyn_nested_chain_splat_in_call_args_is_reached_by_recursion() {
    // dynamic_markers::visit_call_args (line 409). Here the chain helper is
    // nested as an ARGUMENT of an outer non-chain call:
    // `register(chain(*items))`. Detecting the inner ChainSplat requires
    // `visit_call_args` to recurse into the outer call's arguments. The
    // `visit_call_args -> ()` mutant (410) drops that recursion, so the
    // inner splat would never be visited and the marker would vanish.
    let src = r#"
from airflow import DAG
from airflow.models.baseoperator import chain

with DAG(dag_id="nested"):
    register(chain(*items))
"#;
    assert!(
        has_chain_splat(src),
        "chain(*items) nested inside another call's args must be reached by recursion: {:?}",
        dynamic_markers_for(src)
    );
}

#[test]
fn dyn_non_task_decorator_call_does_not_flag_taskflow() {
    // dynamic_markers::is_task_decorator_call (line 456). Inside a `@dag`
    // body, a NON-task decorator that is ALSO a dynamic Call (positional
    // arg, so `task_decorator_is_dynamic` is satisfied) — `@retrying(3)` —
    // must NOT flag UnsupportedTaskFlow, because `is_task_decorator_call`
    // returns false. The `is_task_decorator_call -> true` mutant (457) would
    // (combined with the satisfied dynamic check) flag this non-task
    // decorator. The positional arg is essential: it keeps
    // `task_decorator_is_dynamic` TRUE so the `&&` cannot mask the mutant.
    let src = r#"
from airflow.sdk import dag

@dag(schedule="@daily")
def pipe():
    @retrying(3)
    def memo(x):
        return x
    memo(1)
"#;
    assert!(
        !has_unsupported_taskflow(src),
        "non-task dynamic decorator call must NOT flag taskflow (is_task_decorator_call wrongly true): {:?}",
        dynamic_markers_for(src)
    );
}

#[test]
fn dyn_task_decorator_call_via_attribute_flags_taskflow() {
    // dynamic_markers::is_task_decorator_call Attribute arm (line 460).
    // The inner resolver's Attribute arm extracts the RIGHTMOST attribute
    // name, which must itself be a TASK_DECORATORS entry. `@sdk.task(...)`
    // (attr == "task") is a dynamic task decorator call and must flag
    // UnsupportedTaskFlow. Deleting the Attribute arm (inner returns None
    // for the Attribute) drops the flag. The decorator carries a POSITIONAL
    // arg so `task_decorator_is_dynamic` is satisfied independently of the
    // Attribute-arm question. The negative companion below (`@app.helper`,
    // attr not in TASK_DECORATORS) keeps the `-> Some(attr)` honest so the
    // `-> true` mutant cannot satisfy both.
    let src = r#"
from airflow.sdk import dag

@dag(schedule="@daily")
def pipe():
    @sdk.task("grp")
    def step():
        return "echo"
    step()
"#;
    assert!(
        has_unsupported_taskflow(src),
        "@sdk.task(...) attribute decorator must flag taskflow (Attribute arm): {:?}",
        dynamic_markers_for(src)
    );
    let non_task = r#"
from airflow.sdk import dag

@dag(schedule="@daily")
def pipe():
    @app.helper("grp")
    def step():
        return "echo"
    step()
"#;
    assert!(
        !has_unsupported_taskflow(non_task),
        "@app.helper(...) (attr not a task decorator) must NOT flag: {:?}",
        dynamic_markers_for(non_task)
    );
}

#[test]
fn dyn_task_decorator_call_via_call_callee_flags_taskflow() {
    // dynamic_markers::is_task_decorator_call Call arm (line 461). When the
    // decorator expression is a Call whose own func is itself a Call
    // (`@task()(expand=True)`), the inner resolver must recurse through the
    // Call arm to find the "task" name. Deleting the Call arm makes the
    // inner resolver return None for that shape, dropping the flag.
    let src = r#"
from airflow.sdk import dag

@dag(schedule="@daily")
def pipe():
    @task()(expand=True)
    def step():
        return 1
    step()
"#;
    assert!(
        has_unsupported_taskflow(src),
        "@task()(...) call-callee decorator must flag taskflow (Call arm): {:?}",
        dynamic_markers_for(src)
    );
}

#[test]
fn dyn_zero_arg_task_decorator_call_is_not_dynamic() {
    // dynamic_markers::task_decorator_is_dynamic (line 470). A `@task()`
    // decorator that IS a Call but has NO positional args and no
    // expand/partial kwarg is NOT dynamic, so it must NOT flag
    // UnsupportedTaskFlow. The `task_decorator_is_dynamic -> true` mutant
    // (471) would flag this empty-arg call as dynamic. (The existing bare
    // `@task` test never reaches this branch because bare `@task` is a Name,
    // not a Call, so `visit_decorator_list`'s `if let Expr::Call` guard
    // skips it — only an empty-arg CALL exercises the `-> true` mutant.)
    let zero_arg = r#"
from airflow.sdk import dag, task

@dag(schedule="@daily")
def pipe():
    @task()
    def step():
        return 1
    step()
"#;
    assert!(
        !has_unsupported_taskflow(zero_arg),
        "@task() with no args must NOT flag taskflow (task_decorator_is_dynamic wrongly true): {:?}",
        dynamic_markers_for(zero_arg)
    );
    // Positive companion: @task(expand=True) IS dynamic and must flag.
    let dynamic = r#"
from airflow.sdk import dag, task

@dag(schedule="@daily")
def pipe():
    @task(expand=True)
    def step(x):
        return x
    step([1])
"#;
    assert!(
        has_unsupported_taskflow(dynamic),
        "@task(expand=True) must flag taskflow"
    );
}

// ===========================================================================
// ruff_impl.rs — second-wave targeted negatives for decorator detection in
// the STATIC extractor (`extract_all_static_dags`). The first-wave tests use
// only the bare-Name and Attribute-Call decorator forms; these add the bare
// `@dag` Name form, Name/Attribute call NEGATIVES, and the `@task(...)` Call
// decorator form so each match guard / arm becomes load-bearing.
// ===========================================================================

#[test]
fn ruff_bare_name_dag_decorator_registers_dag() {
    // ruff_impl::match_dag_decorator Name-guard (line 362,
    // `DAG_DECORATOR_NAMES.contains(id)`). A BARE `@dag` decorator (no
    // parentheses) must register a DAG. The `contains(id) -> false` mutant
    // (362) makes the bare-Name arm reject `@dag`, dropping the DAG. (The
    // first-wave tests only use `@dag(...)`/`@airflow.dag(...)` CALL forms,
    // which hit lines 366/370 — never the bare-Name arm.)
    let src = r"
from airflow.sdk import dag

@dag
def bare_pipeline():
    pass
";
    let dags = extract_all_static_dags(src).expect("parse");
    assert_eq!(
        dags.len(),
        1,
        "bare @dag must register exactly one DAG: {dags:?}"
    );
    assert_eq!(dag_id_of(&dags[0]), Some("bare_pipeline"));
}

#[test]
fn ruff_non_dag_name_call_decorator_is_ignored() {
    // ruff_impl::match_dag_decorator Name-Call guard (line 366,
    // `DAG_DECORATOR_NAMES.contains(id)`). A `@retry(times=3)` decorator —
    // a bare-Name CALL whose name is NOT in DAG_DECORATOR_NAMES — must NOT
    // register a DAG. The `contains(id) -> true` mutant (366) would treat
    // every Name-call decorator as `@dag`.
    let src = r"
def retry(times):
    def deco(fn):
        return fn
    return deco

@retry(times=3)
def not_a_dag():
    pass
";
    let dags = extract_all_static_dags(src).expect("parse");
    assert!(
        dags.is_empty(),
        "non-dag Name-call decorator produced a DAG: {dags:?}"
    );
}

#[test]
fn ruff_non_dag_attribute_call_decorator_is_ignored() {
    // ruff_impl::match_dag_decorator Attribute-Call guard (line 370,
    // `DAG_DECORATOR_NAMES.contains(attr)`). A `@app.task(bind=True)`
    // decorator — an Attribute CALL whose attr is NOT `dag` — must NOT
    // register a DAG. The `contains(attr) -> true` mutant (370) would treat
    // every attribute-call decorator as `@dag`. (The first-wave positive
    // `@airflow.dag(...)` exercises the true branch; this pins the false.)
    let src = r"
import app

@app.task(bind=True)
def celery_task():
    pass
";
    let dags = extract_all_static_dags(src).expect("parse");
    assert!(
        dags.is_empty(),
        "non-dag attribute-call decorator produced a DAG: {dags:?}"
    );
}

#[test]
fn ruff_non_task_decorated_function_in_dag_is_not_a_task() {
    // ruff_impl::is_task_decorator (line 380). A function carrying a
    // NON-task decorator (`@staticmethod`) inside a `@dag` body must NOT
    // become a task. The `is_task_decorator -> true` mutant (381) would
    // register every DECORATED function as a task. (The first-wave negative
    // used an UN-decorated `def plain_helper()`, whose empty decorator list
    // makes `any(...)` false regardless of the mutant — so it never killed
    // `-> true`. A decorated-but-non-task function is required.)
    let src = r#"
from airflow.sdk import dag, task

@dag(schedule="@daily")
def pipe():
    @task
    def real_task():
        pass
    @staticmethod
    def decorated_helper():
        pass
    real_task()
"#;
    let dag = extract_static_dag(src).expect("parse");
    assert_eq!(
        task_id_strings(&dag),
        vec!["real_task"],
        "only the @task function may register; @staticmethod must not"
    );
}

#[test]
fn ruff_task_decorator_call_form_registers_task() {
    // ruff_impl::is_task_decorator Call arm (line 385,
    // `Expr::Call(call) => inner_name(&call.func)`). A `@task(multiple_outputs=True)`
    // decorator — the Call form — inside a `@dag` body must register the
    // function as a task. Deleting the Call arm (385) makes `inner_name`
    // return None for the Call shape, so the function is not registered.
    // (The first-wave positive used bare `@task` / `@airflow.task`, i.e. the
    // Name / Attribute arms — never the Call arm.)
    let src = r#"
from airflow.sdk import dag, task

@dag(schedule="@daily")
def pipe():
    @task(multiple_outputs=True)
    def call_form():
        return {"a": 1}
    call_form()
"#;
    let dag = extract_static_dag(src).expect("parse");
    assert_eq!(
        task_id_strings(&dag),
        vec!["call_form"],
        "@task(...) call-form decorator must register the task: {dag:?}"
    );
}

// ===========================================================================
// ParseCache — drives hash_source + parse_dag_file through the public type
// ===========================================================================

#[test]
fn cache_hit_returns_identical_hash() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("c.py");
    std::fs::write(&path, "with DAG(dag_id=\"cached\"):\n    pass\n").expect("write");
    let cache = ParseCache::new();
    let first = cache.get_or_parse(&path).expect("first parse");
    let second = cache.get_or_parse(&path).expect("cache hit");
    assert_eq!(first.source_hash, second.source_hash);
    assert_eq!(first.dags.len(), second.dags.len());
}
