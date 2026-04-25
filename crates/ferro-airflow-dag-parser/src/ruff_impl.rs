// SPDX-License-Identifier: Apache-2.0
//! `ruff_python_parser` (vendored as `littrs-ruff-python-parser`)
//! backend for static Airflow DAG extraction.
//!
//! Walks the AST to recover the [`ExtractedDag`] aggregate; the
//! visitor is purpose-built for Airflow DAG construction shapes
//! (`with DAG(...)`, `@dag def fn():`, dependency operators
//! `>>`/`<<`, `set_upstream`/`set_downstream` setters).

use crate::common::TaskId;
use ruff_python_ast::{
    self as ast, Expr, ExprAttribute, ExprBinOp, ExprCall, ExprName, ExprStringLiteral, ModModule,
    Operator, Stmt, StmtAssign, StmtClassDef, StmtFor, StmtFunctionDef, StmtIf, StmtTry, StmtWhile,
    StmtWith, WithItem,
};
use ruff_text_size::TextRange;

use crate::common::{ExtractedDag, ParseError, make_dag_id, make_task_id, push_unique_task};
use crate::line_index::LineIndex;
use crate::panic_safe::parse_module_safely;

const DAG_NAMES: &[&str] = &["DAG"];
const DAG_DECORATOR_NAMES: &[&str] = &["dag"];
const SETTER_METHODS: &[&str] = &["set_upstream", "set_downstream"];

/// Parse `source` and return all DAGs found.
///
/// # Errors
///
/// Returns [`ParseError::Parse`] when `ruff_python_parser` reports a
/// syntax error, or [`ParseError::InvalidIdentifier`] when the recovered
/// `dag_id` / `task_id` literal violates Airflow's 250-char / safe-charset
/// rule.
pub fn extract_all(source: &str) -> Result<Vec<ExtractedDag>, ParseError> {
    let parsed = parse_module_safely(source)?;
    let module: &ModModule = parsed.syntax();
    let line_index = LineIndex::new(source);
    let mut dags = Vec::new();
    let mut walker = Walker {
        dags: &mut dags,
        task_aliases: Vec::new(),
        line_index: &line_index,
    };
    walker.visit_stmts(&module.body, None)?;
    Ok(dags)
}

/// Convenience: extract the first DAG (or default) from `source`.
///
/// # Errors
///
/// Returns [`ParseError::Parse`] when `ruff_python_parser` reports a
/// syntax error.
pub fn extract(source: &str) -> Result<ExtractedDag, ParseError> {
    Ok(extract_all(source)?.into_iter().next().unwrap_or_default())
}

struct Walker<'a> {
    dags: &'a mut Vec<ExtractedDag>,
    task_aliases: Vec<(String, TaskId)>,
    line_index: &'a LineIndex,
}

impl Walker<'_> {
    fn visit_stmts(&mut self, stmts: &[Stmt], dag_idx: Option<usize>) -> Result<(), ParseError> {
        for stmt in stmts {
            self.visit_stmt(stmt, dag_idx)?;
        }
        Ok(())
    }

    fn visit_stmt(&mut self, stmt: &Stmt, dag_idx: Option<usize>) -> Result<(), ParseError> {
        match stmt {
            Stmt::With(StmtWith {
                items, body, range, ..
            }) => {
                self.visit_with(items, body, *range, dag_idx)?;
            }
            Stmt::FunctionDef(StmtFunctionDef {
                name,
                decorator_list,
                body,
                range,
                ..
            }) => {
                self.visit_function(name.as_str(), decorator_list, body, *range, dag_idx)?;
            }
            Stmt::Assign(StmtAssign { targets, value, .. }) => {
                self.visit_assign(targets, value, dag_idx)?;
            }
            Stmt::AnnAssign(ast::StmtAnnAssign {
                target,
                value: Some(value),
                ..
            }) => {
                self.visit_assign(std::slice::from_ref(target.as_ref()), value, dag_idx)?;
            }
            Stmt::Expr(ast::StmtExpr { value, .. }) => {
                self.visit_expr_stmt(value, dag_idx)?;
            }
            Stmt::ClassDef(StmtClassDef { body, .. })
            | Stmt::If(StmtIf { body, .. })
            | Stmt::For(StmtFor { body, .. })
            | Stmt::While(StmtWhile { body, .. })
            | Stmt::Try(StmtTry { body, .. }) => {
                self.visit_stmts(body, dag_idx)?;
            }
            _ => {}
        }
        Ok(())
    }

    fn visit_with(
        &mut self,
        items: &[WithItem],
        body: &[Stmt],
        range: TextRange,
        outer_dag: Option<usize>,
    ) -> Result<(), ParseError> {
        let mut new_dag_idx = outer_dag;
        for item in items {
            if let Expr::Call(call) = &item.context_expr
                && let Some(mut dag) = parse_dag_call(call)?
            {
                dag.source_span = Some(self.line_index.span_of(range));
                self.dags.push(dag);
                new_dag_idx = Some(self.dags.len() - 1);
            }
        }
        let alias_marker = self.task_aliases.len();
        self.visit_stmts(body, new_dag_idx)?;
        self.task_aliases.truncate(alias_marker);
        Ok(())
    }

    fn visit_function(
        &mut self,
        name: &str,
        decorators: &[ast::Decorator],
        body: &[Stmt],
        range: TextRange,
        outer_dag: Option<usize>,
    ) -> Result<(), ParseError> {
        let dag_decorator = decorators
            .iter()
            .find_map(|d| match_dag_decorator(&d.expression));
        let task_decorator = decorators.iter().any(|d| is_task_decorator(&d.expression));

        if let Some(call_kwargs) = dag_decorator {
            let dag_id = make_dag_id(name.to_string())?;
            let mut dag = ExtractedDag {
                dag_id: Some(dag_id),
                source_span: Some(self.line_index.span_of(range)),
                ..ExtractedDag::default()
            };
            if let DagDecoratorMatch::Call(call) = call_kwargs {
                merge_dag_kwargs(&mut dag, call)?;
            }
            self.dags.push(dag);
            let new_idx = Some(self.dags.len() - 1);
            let marker = self.task_aliases.len();
            self.visit_stmts(body, new_idx)?;
            self.task_aliases.truncate(marker);
            return Ok(());
        }

        if task_decorator && let Some(idx) = outer_dag {
            let task_id = make_task_id(name.to_string())?;
            push_unique_task(&mut self.dags[idx].task_ids, task_id.clone());
            self.task_aliases.push((name.to_string(), task_id));
        }

        self.visit_stmts(body, outer_dag)
    }

    fn visit_assign(
        &mut self,
        targets: &[Expr],
        value: &Expr,
        dag_idx: Option<usize>,
    ) -> Result<(), ParseError> {
        if let Expr::Call(call) = value {
            if let Some(dag) = parse_dag_call(call)? {
                self.dags.push(dag);
                return Ok(());
            }
            if let Some(task_id_str) = call_task_id(call)
                && let Some(name) = single_target_name(targets)
                && let Some(idx) = dag_idx
            {
                let task_id = make_task_id(task_id_str)?;
                push_unique_task(&mut self.dags[idx].task_ids, task_id.clone());
                self.task_aliases.push((name.to_string(), task_id));
            }
        }
        Ok(())
    }

    fn visit_expr_stmt(&mut self, expr: &Expr, dag_idx: Option<usize>) -> Result<(), ParseError> {
        if let Expr::Call(call) = expr
            && let Some(task_id_str) = call_task_id(call)
            && let Some(idx) = dag_idx
        {
            let task_id = make_task_id(task_id_str)?;
            push_unique_task(&mut self.dags[idx].task_ids, task_id);
            return Ok(());
        }
        if let Expr::BinOp(ExprBinOp {
            left, op, right, ..
        }) = expr
            && (matches!(op, Operator::RShift | Operator::LShift))
            && let Some(idx) = dag_idx
        {
            self.collect_shift_edges(left, *op, right, idx);
            return Ok(());
        }
        if let Expr::Call(call) = expr
            && let Expr::Attribute(ExprAttribute { value, attr, .. }) = call.func.as_ref()
            && SETTER_METHODS.contains(&attr.as_str())
            && let Some(idx) = dag_idx
        {
            let lhs = self.resolve_to_task_id(value);
            for arg in &call.arguments.args {
                let rhs = self.resolve_to_task_id(arg);
                if let (Some(l), Some(r)) = (lhs.clone(), rhs) {
                    if attr.as_str() == "set_downstream" {
                        push_unique_edge(&mut self.dags[idx].deps_edges, (l.clone(), r));
                    } else {
                        push_unique_edge(&mut self.dags[idx].deps_edges, (r, l.clone()));
                    }
                }
            }
        }
        Ok(())
    }

    fn collect_shift_edges(&mut self, left: &Expr, op: Operator, right: &Expr, dag_idx: usize) {
        let left_terms = self.terminal_task_ids(left, op);
        let right_terms = self.terminal_task_ids(right, op);
        for l in &left_terms {
            for r in &right_terms {
                let edge = if matches!(op, Operator::RShift) {
                    (l.clone(), r.clone())
                } else {
                    (r.clone(), l.clone())
                };
                push_unique_edge(&mut self.dags[dag_idx].deps_edges, edge);
            }
        }
        if let Expr::BinOp(ExprBinOp {
            left: l2,
            op: op2,
            right: r2,
            ..
        }) = left
            && matches!(op2, Operator::RShift | Operator::LShift)
        {
            self.collect_shift_edges(l2, *op2, r2, dag_idx);
        }
    }

    fn terminal_task_ids(&self, expr: &Expr, parent_op: Operator) -> Vec<TaskId> {
        if let Expr::BinOp(ExprBinOp {
            left: _, op, right, ..
        }) = expr
            && matches!(op, Operator::RShift | Operator::LShift)
        {
            let _ = parent_op;
            return self.terminal_task_ids(right, *op);
        }
        let mut out = Vec::new();
        match expr {
            Expr::List(ast::ExprList { elts, .. }) | Expr::Tuple(ast::ExprTuple { elts, .. }) => {
                for elt in elts {
                    if let Some(id) = self.resolve_to_task_id(elt) {
                        out.push(id);
                    }
                }
            }
            other => {
                if let Some(id) = self.resolve_to_task_id(other) {
                    out.push(id);
                }
            }
        }
        out
    }

    fn resolve_to_task_id(&self, expr: &Expr) -> Option<TaskId> {
        match expr {
            Expr::Name(ExprName { id, .. }) => self
                .task_aliases
                .iter()
                .rev()
                .find(|(n, _)| n == id.as_str())
                .map(|(_, t)| t.clone()),
            // `add_one.expand(...)` or `step_one()` — recurse into the
            // receiver / callee. Resolving a bare `Name` callee picks
            // up `@task`-decorated functions registered as aliases of
            // themselves.
            Expr::Call(call) => self.resolve_to_task_id(&call.func),
            Expr::Attribute(ExprAttribute { value, .. }) => self.resolve_to_task_id(value),
            _ => None,
        }
    }
}

fn parse_dag_call(call: &ExprCall) -> Result<Option<ExtractedDag>, ParseError> {
    if !is_dag_callable(&call.func) {
        return Ok(None);
    }
    let mut dag = ExtractedDag::default();
    if let Some(first) = call.arguments.args.first()
        && let Some(s) = constant_str(first)
    {
        dag.dag_id = Some(make_dag_id(s)?);
    }
    merge_dag_kwargs(&mut dag, call)?;
    Ok(Some(dag))
}

fn merge_dag_kwargs(dag: &mut ExtractedDag, call: &ExprCall) -> Result<(), ParseError> {
    for kw in &call.arguments.keywords {
        let Some(arg) = kw.arg.as_ref() else { continue };
        match arg.as_str() {
            "dag_id" => {
                if let Some(s) = constant_str(&kw.value) {
                    dag.dag_id = Some(make_dag_id(s)?);
                }
            }
            "schedule" | "schedule_interval" | "timetable" => {
                dag.schedule = Some(stringify_expr(&kw.value));
            }
            "default_args" => {
                dag.has_default_args = true;
            }
            _ => {}
        }
    }
    Ok(())
}

fn is_dag_callable(expr: &Expr) -> bool {
    match expr {
        Expr::Name(ExprName { id, .. }) => DAG_NAMES.contains(&id.as_str()),
        Expr::Attribute(ExprAttribute { attr, .. }) => DAG_NAMES.contains(&attr.as_str()),
        _ => false,
    }
}

/// Result of looking for `@dag` / `@dag(...)` on a decorator slot.
#[derive(Debug)]
enum DagDecoratorMatch<'a> {
    /// Bare `@dag` (no parentheses).
    Bare,
    /// `@dag(...)` — kwargs come from the inner [`ExprCall`].
    Call(&'a ExprCall),
}

fn match_dag_decorator(expr: &Expr) -> Option<DagDecoratorMatch<'_>> {
    match expr {
        Expr::Name(ExprName { id, .. }) if DAG_DECORATOR_NAMES.contains(&id.as_str()) => {
            Some(DagDecoratorMatch::Bare)
        }
        Expr::Call(call) => match call.func.as_ref() {
            Expr::Name(ExprName { id, .. }) if DAG_DECORATOR_NAMES.contains(&id.as_str()) => {
                Some(DagDecoratorMatch::Call(call))
            }
            Expr::Attribute(ExprAttribute { attr, .. })
                if DAG_DECORATOR_NAMES.contains(&attr.as_str()) =>
            {
                Some(DagDecoratorMatch::Call(call))
            }
            _ => None,
        },
        _ => None,
    }
}

fn is_task_decorator(expr: &Expr) -> bool {
    fn inner_name(expr: &Expr) -> Option<&str> {
        match expr {
            Expr::Name(ExprName { id, .. }) => Some(id.as_str()),
            Expr::Attribute(ExprAttribute { attr, .. }) => Some(attr.as_str()),
            Expr::Call(call) => inner_name(&call.func),
            _ => None,
        }
    }
    matches!(
        inner_name(expr),
        Some("task" | "task_group" | "setup" | "teardown" | "sensor" | "short_circuit")
    )
}

fn call_task_id(call: &ExprCall) -> Option<String> {
    for kw in &call.arguments.keywords {
        if let Some(arg) = kw.arg.as_ref()
            && arg.as_str() == "task_id"
            && let Some(s) = constant_str(&kw.value)
        {
            return Some(s);
        }
    }
    None
}

fn constant_str(expr: &Expr) -> Option<String> {
    if let Expr::StringLiteral(ExprStringLiteral { value, .. }) = expr {
        return Some(value.to_str().to_string());
    }
    None
}

fn stringify_expr(expr: &Expr) -> String {
    match expr {
        Expr::StringLiteral(ExprStringLiteral { value, .. }) => value.to_str().to_string(),
        Expr::NoneLiteral(_) => "None".to_string(),
        Expr::BooleanLiteral(ast::ExprBooleanLiteral { value, .. }) => value.to_string(),
        Expr::NumberLiteral(ast::ExprNumberLiteral { value, .. }) => match value {
            ast::Number::Int(i) => i.to_string(),
            ast::Number::Float(f) => f.to_string(),
            ast::Number::Complex { real, imag } => format!("{real}+{imag}j"),
        },
        Expr::Name(ExprName { id, .. }) => id.as_str().to_string(),
        Expr::Attribute(ExprAttribute { value, attr, .. }) => {
            format!("{}.{}", stringify_expr(value), attr.as_str())
        }
        Expr::Call(call) => {
            let func = stringify_expr(&call.func);
            format!("{func}(...)")
        }
        _ => "<expr>".to_string(),
    }
}

fn single_target_name(targets: &[Expr]) -> Option<&str> {
    if targets.len() != 1 {
        return None;
    }
    if let Expr::Name(ExprName { id, .. }) = &targets[0] {
        return Some(id.as_str());
    }
    None
}

fn push_unique_edge(into: &mut Vec<(TaskId, TaskId)>, edge: (TaskId, TaskId)) {
    if !into
        .iter()
        .any(|e| e.0.as_str() == edge.0.as_str() && e.1.as_str() == edge.1.as_str())
    {
        into.push(edge);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_simple_with_dag() {
        let src = r#"
from airflow import DAG
from airflow.operators.bash import BashOperator

with DAG(dag_id="hello", schedule="@daily", default_args={"a": 1}) as dag:
    a = BashOperator(task_id="a", bash_command="echo a")
    b = BashOperator(task_id="b", bash_command="echo b")
    a >> b
"#;
        let dags = extract_all(src).expect("parse");
        assert_eq!(dags.len(), 1);
        let dag = &dags[0];
        assert_eq!(
            dag.dag_id.as_ref().map(crate::common::DagId::as_str),
            Some("hello")
        );
        assert_eq!(
            dag.task_ids.iter().map(TaskId::as_str).collect::<Vec<_>>(),
            vec!["a", "b"]
        );
        assert_eq!(dag.schedule.as_deref(), Some("@daily"));
        assert!(dag.has_default_args);
        let edges: Vec<(&str, &str)> = dag
            .deps_edges
            .iter()
            .map(|(a, b)| (a.as_str(), b.as_str()))
            .collect();
        assert_eq!(edges, vec![("a", "b")]);
        // The `with DAG(...) as dag:` block starts on line 5 (1-indexed).
        let span = dag.source_span.expect("span recorded");
        assert!(span.start_line >= 5 && span.end_line >= span.start_line);
    }

    #[test]
    fn extracts_dag_decorator() {
        let src = r#"
from airflow.sdk import dag, task

@dag(schedule="@daily")
def my_pipeline():
    @task
    def inner():
        pass
    inner()
"#;
        let dags = extract_all(src).expect("parse");
        assert_eq!(dags.len(), 1);
        assert_eq!(
            dags[0].dag_id.as_ref().map(crate::common::DagId::as_str),
            Some("my_pipeline")
        );
        assert_eq!(
            dags[0]
                .task_ids
                .iter()
                .map(TaskId::as_str)
                .collect::<Vec<_>>(),
            vec!["inner"]
        );
        assert_eq!(dags[0].schedule.as_deref(), Some("@daily"));
        assert!(dags[0].source_span.is_some());
    }

    #[test]
    fn parse_error_surfaces() {
        let err = extract_all("def !!!").unwrap_err();
        assert!(matches!(err, ParseError::Parse(_)));
    }

    #[test]
    fn invalid_dag_id_literal_surfaces_as_invalid_identifier() {
        // `dag_id` literal violates the safe charset (contains `/`).
        let src = r#"
from airflow import DAG

with DAG(dag_id="bad/id"):
    pass
"#;
        let err = extract_all(src).unwrap_err();
        assert!(matches!(err, ParseError::InvalidIdentifier { kind, .. } if kind == "dag_id"));
    }
}
