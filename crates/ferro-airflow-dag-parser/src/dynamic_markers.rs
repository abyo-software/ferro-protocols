// SPDX-License-Identifier: Apache-2.0
//! Detect dynamic patterns in a Python DAG file that the static AST
//! extractor cannot resolve and that therefore need `PyO3` fallback.
//!
//! Phase 0 `PoC` surfaced three known fallback markers (see
//! `docs/due-diligence/python-interop-strategy/02-parser-comparison.md`):
//!
//! 1. `dag_id=Path(__file__).stem` (or any attribute call) — runtime
//!    `dag_id` resolution.
//! 2. `chain(*list)` / `cross_downstream` etc. — splat-arg helpers that
//!    expand to an unknown number of edges.
//! 3. `task_id=f"task_{i}"` — f-string `task_id` literal that depends on
//!    a loop variable.
//!
//! Phase 1 extends the surface to four additional patterns the
//! `dag-processor` needs to make a fallback / no-fallback decision:
//!
//! 4. `schedule=...` passed a non-string-literal expression
//!    (e.g. `schedule=Asset("…")` or `schedule=Timetable()`).
//! 5. `@task` used in a way the static extractor cannot follow
//!    (e.g. `@task(...)` decorator on a non-function or with dynamic
//!    arguments).
//! 6. `if X: with DAG(...)` — DAG defined inside an import-time `if`
//!    body, where `X` is not a constant. Static extraction would still
//!    pick the DAG up but the activation is conditional and Airflow
//!    requires the same evaluation order.
//! 7. `for x in ...: PythonOperator(...)` — DAG construction inside a
//!    `for` body. Task count depends on the iterable.
//!
//! Markers are emitted only when the construct lives inside a DAG
//! context (a `with DAG(...)` block or a `@dag`-decorated function);
//! patterns that appear at module scope outside any DAG are not
//! actionable for the dag-processor and would just be noise.

use ruff_python_ast::{
    self as ast, Decorator, Expr, ExprAttribute, ExprCall, ExprFString, ExprName, ModModule, Stmt,
    StmtAssign, StmtClassDef, StmtFor, StmtFunctionDef, StmtIf, StmtTry, StmtWhile, StmtWith,
    WithItem,
};
use ruff_text_size::Ranged;

use crate::common::ParseError;
use crate::line_index::LineIndex;
use crate::panic_safe::parse_module_safely;

/// Detected dynamic-pattern marker. Each variant carries enough context
/// to surface a precise diagnostic and route the file to `PyO3` fallback
/// (`ferroair-py-embed`) when the dag-processor sees it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind")]
pub enum DynamicMarker {
    /// `dag_id=Path(__file__).stem` (or any call/attribute as `dag_id`).
    /// Phase 0 known-fallback case.
    PathStemDagId {
        /// 1-indexed source line.
        line: u32,
        /// 1-indexed source column.
        col: u32,
    },
    /// `chain(*list)` / `cross_downstream(*list)` — splat argument to a
    /// dependency helper. Phase 0 known-fallback case.
    ChainSplat {
        /// 1-indexed source line.
        line: u32,
        /// 1-indexed source column.
        col: u32,
    },
    /// `task_id=f"task_{i}"` — f-string `task_id` literal. Phase 0
    /// known-fallback case.
    FStringTaskId {
        /// 1-indexed source line.
        line: u32,
        /// 1-indexed source column.
        col: u32,
        /// Best-effort textual rendering of the f-string (literal parts
        /// kept verbatim, expression slots elided as `{…}`). Used for
        /// log lines and DD evidence; not load-bearing for routing.
        source: String,
    },
    /// `schedule=...` passed an expression that is not a string literal,
    /// `None`, or a `timedelta(...)`-like literal. Could be an
    /// `Asset(...)` schedule, a custom timetable, or a runtime variable.
    DynamicScheduleExpr {
        /// 1-indexed source line.
        line: u32,
        /// 1-indexed source column.
        col: u32,
    },
    /// `@task(...)` invocation that the static extractor cannot follow
    /// (decorator factory call with non-static arguments). Today this
    /// is conservative — any `@task(...)` with an `expand=` or
    /// `partial=` kwarg, or any non-name positional arg, trips the
    /// marker.
    UnsupportedTaskFlow {
        /// 1-indexed source line.
        line: u32,
        /// 1-indexed source column.
        col: u32,
    },
    /// DAG construct inside an `if X:` body where `X` is not a constant.
    /// Airflow allows import-time branching but the activation is
    /// conditional, so the dag-processor needs to know to evaluate the
    /// branch in `CPython` rather than trust the static extraction.
    ImportTimeBranching {
        /// 1-indexed source line.
        line: u32,
        /// 1-indexed source column.
        col: u32,
    },
    /// Operator instantiation inside a `for ... in …:` loop body. Task
    /// count depends on the iterable.
    ForLoopTaskGeneration {
        /// 1-indexed source line.
        line: u32,
        /// 1-indexed source column.
        col: u32,
    },
}

impl DynamicMarker {
    /// Short human-readable kind label (for log/metric grouping).
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::PathStemDagId { .. } => "path_stem_dag_id",
            Self::ChainSplat { .. } => "chain_splat",
            Self::FStringTaskId { .. } => "fstring_task_id",
            Self::DynamicScheduleExpr { .. } => "dynamic_schedule_expr",
            Self::UnsupportedTaskFlow { .. } => "unsupported_task_flow",
            Self::ImportTimeBranching { .. } => "import_time_branching",
            Self::ForLoopTaskGeneration { .. } => "for_loop_task_generation",
        }
    }
}

const DAG_NAMES: &[&str] = &["DAG"];
const DAG_DECORATOR_NAMES: &[&str] = &["dag"];
const CHAIN_HELPERS: &[&str] = &["chain", "chain_linear", "cross_downstream"];
const OPERATOR_SUFFIXES: &[&str] = &["Operator", "Sensor"];
const TASK_DECORATORS: &[&str] = &[
    "task",
    "task_group",
    "setup",
    "teardown",
    "sensor",
    "short_circuit",
];

/// Detect dynamic markers in `source`. Errors only when the underlying
/// Python parser rejects the file; an empty `Vec` simply means the
/// source contains no detected dynamic patterns.
///
/// # Errors
///
/// Returns [`ParseError::Parse`] when ruff cannot parse the source.
pub fn detect_dynamic_markers(source: &str) -> Result<Vec<DynamicMarker>, ParseError> {
    let parsed = parse_module_safely(source)?;
    let module: &ModModule = parsed.syntax();
    let line_index = LineIndex::new(source);
    let mut visitor = MarkerVisitor {
        line_index: &line_index,
        markers: Vec::new(),
        in_dag_ctx: 0,
    };
    visitor.visit_stmts(&module.body);
    Ok(visitor.markers)
}

struct MarkerVisitor<'a> {
    line_index: &'a LineIndex,
    markers: Vec<DynamicMarker>,
    /// Depth counter for "we are inside a `with DAG(...)` block or a
    /// `@dag`-decorated function". Markers are only emitted when this
    /// is non-zero, which is what makes the helper actionable for the
    /// dag-processor (rather than every `for` loop in any module).
    in_dag_ctx: u32,
}

impl MarkerVisitor<'_> {
    fn line_col(&self, node: &impl Ranged) -> (u32, u32) {
        self.line_index.line_col(node.range().start().to_u32())
    }

    fn visit_stmts(&mut self, stmts: &[Stmt]) {
        for stmt in stmts {
            self.visit_stmt(stmt);
        }
    }

    fn visit_stmt(&mut self, stmt: &Stmt) {
        match stmt {
            Stmt::With(StmtWith { items, body, .. }) => {
                let opened = self.enter_with(items);
                self.visit_stmts(body);
                if opened {
                    self.in_dag_ctx -= 1;
                }
            }
            Stmt::FunctionDef(StmtFunctionDef {
                decorator_list,
                body,
                ..
            }) => {
                let dag_dec = decorator_list
                    .iter()
                    .any(|d| match_dag_decorator(&d.expression));
                if dag_dec {
                    self.in_dag_ctx += 1;
                }
                self.visit_decorator_list(decorator_list);
                self.visit_stmts(body);
                if dag_dec {
                    self.in_dag_ctx -= 1;
                }
            }
            Stmt::Assign(StmtAssign { targets, value, .. }) => {
                self.visit_expr(value);
                for t in targets {
                    self.visit_expr(t);
                }
            }
            Stmt::AnnAssign(ast::StmtAnnAssign {
                target,
                value: Some(value),
                ..
            }) => {
                self.visit_expr(target);
                self.visit_expr(value);
            }
            Stmt::Expr(ast::StmtExpr { value, .. }) => {
                self.visit_expr(value);
            }
            Stmt::If(StmtIf {
                test,
                body,
                elif_else_clauses,
                ..
            }) => {
                let conditional = !is_constant_bool(test);
                if conditional {
                    self.flag_branching_in_body(body);
                    for clause in elif_else_clauses {
                        self.flag_branching_in_body(&clause.body);
                    }
                }
                self.visit_expr(test);
                self.visit_stmts(body);
                for clause in elif_else_clauses {
                    self.visit_stmts(&clause.body);
                }
            }
            Stmt::For(StmtFor { body, iter, .. }) => {
                self.flag_for_loop_body(body);
                self.visit_expr(iter);
                self.visit_stmts(body);
            }
            Stmt::While(StmtWhile { body, test, .. }) => {
                self.visit_expr(test);
                self.visit_stmts(body);
            }
            Stmt::Try(StmtTry { body, .. }) | Stmt::ClassDef(StmtClassDef { body, .. }) => {
                self.visit_stmts(body);
            }
            _ => {}
        }
    }

    /// Returns `true` if a DAG context was opened by this `with` block.
    fn enter_with(&mut self, items: &[WithItem]) -> bool {
        let mut opened = false;
        for item in items {
            if let Expr::Call(call) = &item.context_expr
                && is_dag_callable(&call.func)
            {
                self.in_dag_ctx += 1;
                opened = true;
                self.scan_dag_kwargs(call);
            } else if let Expr::Call(call) = &item.context_expr {
                self.visit_call_args(call);
            }
        }
        opened
    }

    fn scan_dag_kwargs(&mut self, call: &ExprCall) {
        // `dag_id=Path(__file__).stem` — Path(...).stem is an attribute
        // call expression, not a constant.
        for kw in &call.arguments.keywords {
            let Some(name) = kw.arg.as_ref() else {
                continue;
            };
            match name.as_str() {
                "dag_id" => match &kw.value {
                    Expr::StringLiteral(_) => {}
                    other => {
                        let (line, col) = self.line_col(other);
                        self.markers
                            .push(DynamicMarker::PathStemDagId { line, col });
                    }
                },
                "schedule" | "schedule_interval" | "timetable"
                    if !is_acceptable_schedule_literal(&kw.value) =>
                {
                    let (line, col) = self.line_col(&kw.value);
                    self.markers
                        .push(DynamicMarker::DynamicScheduleExpr { line, col });
                }
                _ => {}
            }
        }
    }

    fn visit_decorator_list(&mut self, decorators: &[Decorator]) {
        for d in decorators {
            self.visit_expr(&d.expression);
            // `@task(expand=…, partial=…)` or `@task(...)` with non-trivial
            // args — flag it as unsupported when we are inside a DAG.
            if self.in_dag_ctx == 0 {
                continue;
            }
            if let Expr::Call(call) = &d.expression
                && is_task_decorator_call(call)
                && task_decorator_is_dynamic(call)
            {
                let (line, col) = self.line_col(call);
                self.markers
                    .push(DynamicMarker::UnsupportedTaskFlow { line, col });
            }
        }
    }

    fn flag_branching_in_body(&mut self, body: &[Stmt]) {
        for stmt in body {
            if let Stmt::With(StmtWith { items, .. }) = stmt {
                for item in items {
                    if let Expr::Call(call) = &item.context_expr
                        && is_dag_callable(&call.func)
                    {
                        let (line, col) = self.line_col(call);
                        self.markers
                            .push(DynamicMarker::ImportTimeBranching { line, col });
                    }
                }
            }
        }
    }

    fn flag_for_loop_body(&mut self, body: &[Stmt]) {
        if self.in_dag_ctx == 0 {
            return;
        }
        for stmt in body {
            self.flag_for_stmt(stmt);
        }
    }

    fn flag_for_stmt(&mut self, stmt: &Stmt) {
        let value = match stmt {
            Stmt::Expr(ast::StmtExpr { value, .. }) | Stmt::Assign(StmtAssign { value, .. }) => {
                value.as_ref()
            }
            _ => return,
        };
        if let Expr::Call(call) = value
            && is_operator_constructor(&call.func)
        {
            let (line, col) = self.line_col(call);
            self.markers
                .push(DynamicMarker::ForLoopTaskGeneration { line, col });
        }
    }

    fn visit_expr(&mut self, expr: &Expr) {
        if let Expr::Call(call) = expr {
            self.visit_call(call);
        }
    }

    fn visit_call(&mut self, call: &ExprCall) {
        // chain(*list) / cross_downstream(*list).
        if self.in_dag_ctx > 0 && callee_is_chain_helper(&call.func) {
            for arg in &call.arguments.args {
                if let Expr::Starred(_) = arg {
                    let (line, col) = self.line_col(arg);
                    self.markers.push(DynamicMarker::ChainSplat { line, col });
                    break;
                }
            }
        }

        // task_id=f"...".
        for kw in &call.arguments.keywords {
            if let Some(name) = kw.arg.as_ref()
                && name.as_str() == "task_id"
                && let Expr::FString(fstr) = &kw.value
            {
                let (line, col) = self.line_col(fstr);
                self.markers.push(DynamicMarker::FStringTaskId {
                    line,
                    col,
                    source: render_fstring(fstr),
                });
            }
        }

        self.visit_call_args(call);
    }

    fn visit_call_args(&mut self, call: &ExprCall) {
        for arg in &call.arguments.args {
            self.visit_expr(arg);
        }
        for kw in &call.arguments.keywords {
            self.visit_expr(&kw.value);
        }
    }
}

fn is_dag_callable(expr: &Expr) -> bool {
    match expr {
        Expr::Name(ExprName { id, .. }) => DAG_NAMES.contains(&id.as_str()),
        Expr::Attribute(ExprAttribute { attr, .. }) => DAG_NAMES.contains(&attr.as_str()),
        _ => false,
    }
}

fn match_dag_decorator(expr: &Expr) -> bool {
    fn inner(expr: &Expr) -> Option<&str> {
        match expr {
            Expr::Name(ExprName { id, .. }) => Some(id.as_str()),
            Expr::Attribute(ExprAttribute { attr, .. }) => Some(attr.as_str()),
            Expr::Call(call) => inner(&call.func),
            _ => None,
        }
    }
    matches!(inner(expr), Some(name) if DAG_DECORATOR_NAMES.contains(&name))
}

fn callee_is_chain_helper(expr: &Expr) -> bool {
    match expr {
        Expr::Name(ExprName { id, .. }) => CHAIN_HELPERS.contains(&id.as_str()),
        Expr::Attribute(ExprAttribute { attr, .. }) => CHAIN_HELPERS.contains(&attr.as_str()),
        _ => false,
    }
}

fn is_operator_constructor(expr: &Expr) -> bool {
    let name = match expr {
        Expr::Name(ExprName { id, .. }) => id.as_str(),
        Expr::Attribute(ExprAttribute { attr, .. }) => attr.as_str(),
        _ => return false,
    };
    OPERATOR_SUFFIXES.iter().any(|suf| name.ends_with(suf))
}

fn is_task_decorator_call(call: &ExprCall) -> bool {
    fn inner(expr: &Expr) -> Option<&str> {
        match expr {
            Expr::Name(ExprName { id, .. }) => Some(id.as_str()),
            Expr::Attribute(ExprAttribute { attr, .. }) => Some(attr.as_str()),
            Expr::Call(c) => inner(&c.func),
            _ => None,
        }
    }
    matches!(inner(&call.func), Some(name) if TASK_DECORATORS.contains(&name))
}

/// Conservative: anything that is not `@task()` (zero-arg) is considered
/// dynamic. Phase 1 dag-processor can refine this once it has more data.
fn task_decorator_is_dynamic(call: &ExprCall) -> bool {
    !call.arguments.args.is_empty()
        || call.arguments.keywords.iter().any(|kw| {
            kw.arg
                .as_ref()
                .is_some_and(|n| matches!(n.as_str(), "expand" | "partial"))
        })
}

fn is_acceptable_schedule_literal(expr: &Expr) -> bool {
    matches!(
        expr,
        Expr::StringLiteral(_) | Expr::NoneLiteral(_) | Expr::EllipsisLiteral(_)
    ) || matches!(
        expr,
        Expr::Call(c) if matches!(
            c.func.as_ref(),
            Expr::Name(ExprName { id, .. }) if id.as_str() == "timedelta"
        )
    )
}

const fn is_constant_bool(expr: &Expr) -> bool {
    matches!(
        expr,
        Expr::BooleanLiteral(_) | Expr::NoneLiteral(_) | Expr::NumberLiteral(_)
    )
}

fn render_fstring(fstr: &ExprFString) -> String {
    use ruff_python_ast::{FStringPart, InterpolatedStringElement};
    let mut out = String::new();
    for part in &fstr.value {
        match part {
            FStringPart::Literal(lit) => {
                out.push_str(lit.value.as_ref());
            }
            FStringPart::FString(s) => {
                for el in &s.elements {
                    match el {
                        InterpolatedStringElement::Literal(lit) => {
                            out.push_str(lit.value.as_ref());
                        }
                        InterpolatedStringElement::Interpolation(_) => out.push_str("{…}"),
                    }
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn detect(src: &str) -> Vec<DynamicMarker> {
        detect_dynamic_markers(src).expect("parse")
    }

    #[test]
    fn detects_path_stem_dag_id() {
        let src = r"
from pathlib import Path
from airflow import DAG

with DAG(dag_id=Path(__file__).stem):
    pass
";
        let markers = detect(src);
        assert!(
            markers
                .iter()
                .any(|m| matches!(m, DynamicMarker::PathStemDagId { .. })),
            "missing PathStemDagId in {markers:?}"
        );
    }

    #[test]
    fn detects_chain_splat() {
        let src = r#"
from airflow import DAG
from airflow.models.baseoperator import chain
from airflow.operators.bash import BashOperator

with DAG(dag_id="chain_splat"):
    items = [BashOperator(task_id=f"t_{i}") for i in range(3)]
    chain(*items)
"#;
        let markers = detect(src);
        assert!(
            markers
                .iter()
                .any(|m| matches!(m, DynamicMarker::ChainSplat { .. })),
            "missing ChainSplat in {markers:?}"
        );
    }

    #[test]
    fn detects_fstring_task_id() {
        let src = r#"
from airflow import DAG
from airflow.operators.bash import BashOperator

with DAG(dag_id="fs"):
    for i in range(3):
        BashOperator(task_id=f"t_{i}", bash_command="echo")
"#;
        let markers = detect(src);
        assert!(
            markers
                .iter()
                .any(|m| matches!(m, DynamicMarker::FStringTaskId { .. })),
            "missing FStringTaskId in {markers:?}"
        );
    }

    #[test]
    fn detects_dynamic_schedule_expr() {
        let src = r#"
from airflow import DAG
from airflow.assets import Asset

with DAG(dag_id="sched", schedule=Asset("s3://bucket/x")):
    pass
"#;
        let markers = detect(src);
        assert!(
            markers
                .iter()
                .any(|m| matches!(m, DynamicMarker::DynamicScheduleExpr { .. })),
            "missing DynamicScheduleExpr in {markers:?}"
        );
    }

    #[test]
    fn detects_import_time_branching() {
        let src = r#"
import os
from airflow import DAG

if os.environ.get("ENABLE"):
    with DAG(dag_id="conditional"):
        pass
"#;
        let markers = detect(src);
        assert!(
            markers
                .iter()
                .any(|m| matches!(m, DynamicMarker::ImportTimeBranching { .. })),
            "missing ImportTimeBranching in {markers:?}"
        );
    }

    #[test]
    fn detects_for_loop_task_generation() {
        let src = r#"
from airflow import DAG
from airflow.operators.bash import BashOperator

with DAG(dag_id="loopgen"):
    for i in range(3):
        BashOperator(task_id=f"t_{i}", bash_command="echo")
"#;
        let markers = detect(src);
        assert!(
            markers
                .iter()
                .any(|m| matches!(m, DynamicMarker::ForLoopTaskGeneration { .. })),
            "missing ForLoopTaskGeneration in {markers:?}"
        );
    }

    #[test]
    fn detects_unsupported_taskflow_expand() {
        let src = r#"
from airflow.sdk import dag, task

@dag(schedule="@daily")
def my_pipe():
    @task(expand=True)
    def fan_out(x):
        return x
    fan_out([1, 2, 3])
"#;
        let markers = detect(src);
        assert!(
            markers
                .iter()
                .any(|m| matches!(m, DynamicMarker::UnsupportedTaskFlow { .. })),
            "missing UnsupportedTaskFlow in {markers:?}"
        );
    }

    #[test]
    fn no_markers_on_simple_dag() {
        let src = r#"
from airflow import DAG
from airflow.operators.bash import BashOperator

with DAG(dag_id="hello", schedule="@daily"):
    a = BashOperator(task_id="a", bash_command="echo")
    b = BashOperator(task_id="b", bash_command="echo")
    a >> b
"#;
        let markers = detect(src);
        assert!(markers.is_empty(), "expected no markers, got {markers:?}");
    }

    #[test]
    fn marker_kind_strings() {
        assert_eq!(
            DynamicMarker::PathStemDagId { line: 1, col: 1 }.kind(),
            "path_stem_dag_id"
        );
        assert_eq!(
            DynamicMarker::ChainSplat { line: 1, col: 1 }.kind(),
            "chain_splat"
        );
        assert_eq!(
            DynamicMarker::FStringTaskId {
                line: 1,
                col: 1,
                source: "x".into()
            }
            .kind(),
            "fstring_task_id"
        );
    }
}
