// SPDX-License-Identifier: Apache-2.0
//! Backend-agnostic types used by both the `ruff` and `rustpython`
//! parser backends.
//!
//! [`ExtractedDag`] is the unit produced by either backend on a single
//! Python source file; [`ParseError`] is the unified error surface;
//! [`DagId`] and [`TaskId`] are validated newtypes that reject
//! Airflow-incompatible identifiers up-front so downstream consumers
//! (a metadata DB, a UI, a metrics label) do not have to re-validate.
//!
//! The validation rule mirrors Apache Airflow™ exactly: at most 250
//! characters, drawn from `[a-zA-Z0-9_\-\.]`, non-empty.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Maximum length of any DAG / task identifier, in characters.
///
/// Matches
/// the upstream Airflow constraint (`AIRFLOW__CORE__MAX_DAG_ID_LENGTH`
/// and the equivalent task-id rule in `airflow.models.baseoperator`).
pub const MAX_IDENTIFIER_LEN: usize = 250;

/// Validation failure for [`DagId`] / [`TaskId`].
#[derive(Debug, Clone, Error, PartialEq, Eq)]
#[non_exhaustive]
pub enum IdentifierError {
    /// Empty input.
    #[error("{kind} must not be empty")]
    Empty {
        /// Identifier kind (`"dag_id"` / `"task_id"`).
        kind: &'static str,
    },
    /// Identifier exceeded the 250-character cap.
    #[error("{kind} must be at most {max_len} characters (got {len})")]
    TooLong {
        /// Identifier kind.
        kind: &'static str,
        /// Configured maximum (always [`MAX_IDENTIFIER_LEN`] in the
        /// public constructors).
        max_len: usize,
        /// Length actually supplied (in `char` units, not bytes).
        len: usize,
    },
    /// Identifier contained a character outside `[a-zA-Z0-9_\-\.]`.
    #[error("{kind} contains invalid character {bad:?}; allowed: [a-zA-Z0-9_\\-\\.]")]
    InvalidCharacter {
        /// Identifier kind.
        kind: &'static str,
        /// Offending character.
        bad: char,
    },
}

#[inline]
const fn is_safe_airflow_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.'
}

fn validate_safe_identifier(
    kind: &'static str,
    value: &str,
    max_len: usize,
) -> Result<(), IdentifierError> {
    if value.is_empty() {
        return Err(IdentifierError::Empty { kind });
    }
    let len = value.chars().count();
    if len > max_len {
        return Err(IdentifierError::TooLong { kind, max_len, len });
    }
    if let Some(bad) = value.chars().find(|c| !is_safe_airflow_char(*c)) {
        return Err(IdentifierError::InvalidCharacter { kind, bad });
    }
    Ok(())
}

macro_rules! define_safe_identifier {
    ($(#[$meta:meta])* $name:ident, $kind:literal) => {
        $(#[$meta])*
        #[derive(
            Debug,
            Clone,
            PartialEq,
            Eq,
            Hash,
            PartialOrd,
            Ord,
            Serialize,
            Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            /// Validate and construct from any type that converts into a `String`.
            pub fn new(value: impl Into<String>) -> Result<Self, IdentifierError> {
                let s = value.into();
                validate_safe_identifier($kind, &s, MAX_IDENTIFIER_LEN)?;
                Ok(Self(s))
            }

            /// Borrow the underlying string slice.
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }

            /// Consume and return the wrapped `String`.
            #[must_use]
            pub fn into_inner(self) -> String {
                self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }

        impl FromStr for $name {
            type Err = IdentifierError;
            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Self::new(s)
            }
        }

        impl TryFrom<String> for $name {
            type Error = IdentifierError;
            fn try_from(value: String) -> Result<Self, Self::Error> {
                Self::new(value)
            }
        }

        impl TryFrom<&str> for $name {
            type Error = IdentifierError;
            fn try_from(value: &str) -> Result<Self, Self::Error> {
                Self::new(value.to_owned())
            }
        }
    };
}

define_safe_identifier!(
    /// Validated Apache Airflow DAG identifier.
    ///
    /// Constructed via [`DagId::new`]; refuses inputs that exceed
    /// [`MAX_IDENTIFIER_LEN`] characters or contain characters outside
    /// `[a-zA-Z0-9_\-\.]`.
    DagId,
    "dag_id"
);

define_safe_identifier!(
    /// Validated Apache Airflow task identifier.
    ///
    /// Same rule as [`DagId`]: 1–250 chars from `[a-zA-Z0-9_\-\.]`.
    TaskId,
    "task_id"
);

/// Parse-time output of either backend on a single Python source file.
///
/// Multiple DAGs in the same file flatten to multiple [`ExtractedDag`]
/// values (one per `with DAG(...)` block or `@dag`-decorated function).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExtractedDag {
    /// `dag_id="..."` on `DAG(...)` or function name under `@dag`.
    /// `None` means the literal could not be recovered statically and
    /// the caller must fall back to runtime evaluation (e.g. `PyO3`).
    pub dag_id: Option<DagId>,
    /// All `task_id="..."` operator kwargs and `@task`-decorated
    /// function names. De-duplicated, source order preserved.
    pub task_ids: Vec<TaskId>,
    /// `schedule=...` or legacy `schedule_interval=...` literal value.
    /// Non-string literals are best-effort stringified (e.g. `None`,
    /// `timedelta(days=1)`).
    pub schedule: Option<String>,
    /// `default_args={...}` keyword present at DAG construction.
    pub has_default_args: bool,
    /// `>>` / `<<` / `set_upstream` / `set_downstream` edges. Each
    /// tuple is `(upstream_task_id, downstream_task_id)`. When the
    /// referent is a chain helper or a list, the edge is omitted (those
    /// shapes need runtime fallback).
    pub deps_edges: Vec<(TaskId, TaskId)>,
    /// Source span of the DAG construct: the `with DAG(...)` block or
    /// the `@dag def fn():` definition. Useful for error messages and
    /// editor jump-to-DAG; `None` when the backend did not surface
    /// span info.
    pub source_span: Option<SourceSpan>,
}

/// Inclusive line range of a DAG construct in the source file.
/// Lines are 1-indexed, matching Python tracebacks and most editors.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct SourceSpan {
    /// First line of the DAG construct (1-indexed, inclusive).
    pub start_line: u32,
    /// Last line of the DAG construct (1-indexed, inclusive).
    pub end_line: u32,
}

/// Errors returned by either backend.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ParseError {
    /// Underlying Python parser rejected the source.
    #[error("python parse error: {0}")]
    Parse(String),
    /// Recovered identifier failed Airflow-safe validation.
    #[error("invalid {kind} {value:?}: {reason}")]
    InvalidIdentifier {
        /// Identifier kind (`"dag_id"` / `"task_id"`) — propagated as a
        /// static string so the caller can pattern-match without
        /// allocating.
        kind: &'static str,
        /// The raw literal that failed validation.
        value: String,
        /// Reason from the validator.
        reason: String,
    },
    /// Internal invariant violated; should never reach a caller.
    #[error("internal parser error: {0}")]
    Internal(String),
    /// I/O failure while loading a DAG file from disk.
    #[error("io error reading {path:?}: {source}")]
    Io {
        /// Path that failed to read.
        path: std::path::PathBuf,
        /// Underlying OS error.
        #[source]
        source: std::io::Error,
    },
    /// No backend feature was enabled at compile time.
    #[error("no parser backend enabled (compile with the `parser-ruff` feature)")]
    NoBackend,
}

impl ParseError {
    /// Build an [`InvalidIdentifier`](Self::InvalidIdentifier) variant
    /// from an [`IdentifierError`].
    pub(crate) fn from_id_error(kind: &'static str, value: String, err: &IdentifierError) -> Self {
        Self::InvalidIdentifier {
            kind,
            value,
            reason: err.to_string(),
        }
    }
}

/// Append `name` to `into` if it is not already present, preserving
/// first-seen order.
#[cfg(any(feature = "parser-ruff", test))]
pub(crate) fn push_unique_task(into: &mut Vec<TaskId>, name: TaskId) {
    if !into
        .iter()
        .any(|existing| existing.as_str() == name.as_str())
    {
        into.push(name);
    }
}

/// Validate-and-wrap a recovered DAG-id literal. Surfaces
/// [`ParseError::InvalidIdentifier`] when the literal violates the
/// 250-char / safe-charset rule.
pub(crate) fn make_dag_id(value: String) -> Result<DagId, ParseError> {
    DagId::new(value.clone()).map_err(|e| ParseError::from_id_error("dag_id", value, &e))
}

/// Validate-and-wrap a recovered task-id literal. Surfaces
/// [`ParseError::InvalidIdentifier`] when the literal violates the
/// 250-char / safe-charset rule.
pub(crate) fn make_task_id(value: String) -> Result<TaskId, ParseError> {
    TaskId::new(value.clone()).map_err(|e| ParseError::from_id_error("task_id", value, &e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracted_dag_round_trips_through_json() {
        let dag = ExtractedDag {
            dag_id: Some(DagId::new("hello").unwrap()),
            task_ids: vec![TaskId::new("a").unwrap(), TaskId::new("b").unwrap()],
            schedule: Some("@daily".into()),
            has_default_args: true,
            deps_edges: vec![(TaskId::new("a").unwrap(), TaskId::new("b").unwrap())],
            source_span: Some(SourceSpan {
                start_line: 1,
                end_line: 8,
            }),
        };
        let json = serde_json::to_string(&dag).expect("serialise");
        let back: ExtractedDag = serde_json::from_str(&json).expect("deserialise");
        assert_eq!(dag, back);
    }

    #[test]
    fn dag_id_rejects_invalid_chars() {
        let e = DagId::new("has space").expect_err("must reject");
        assert!(matches!(
            e,
            IdentifierError::InvalidCharacter {
                kind: "dag_id",
                bad: ' '
            }
        ));
    }

    #[test]
    fn dag_id_rejects_too_long() {
        let long = "a".repeat(MAX_IDENTIFIER_LEN + 1);
        let e = DagId::new(long).expect_err("must reject");
        assert!(matches!(e, IdentifierError::TooLong { kind: "dag_id", .. }));
    }

    #[test]
    fn dag_id_rejects_empty() {
        let e = DagId::new("").expect_err("must reject");
        assert!(matches!(e, IdentifierError::Empty { kind: "dag_id" }));
    }

    #[test]
    fn task_id_accepts_dotted_dashed_underscored() {
        for ok in &["a", "a.b", "a-b", "a_b", "a.b-c_d.0"] {
            TaskId::new(*ok).unwrap_or_else(|_| panic!("must accept {ok:?}"));
        }
    }

    #[test]
    fn dag_id_displays_inner() {
        let id = DagId::new("hello").unwrap();
        assert_eq!(id.to_string(), "hello");
        assert_eq!(id.as_str(), "hello");
    }

    #[test]
    fn dag_id_from_str_round_trips() {
        let parsed: DagId = "ok".parse().unwrap();
        assert_eq!(parsed.as_str(), "ok");
    }

    #[test]
    fn task_id_try_from_string_round_trips() {
        let s = String::from("foo");
        let id = TaskId::try_from(s).unwrap();
        assert_eq!(id.as_str(), "foo");
    }

    #[test]
    fn make_dag_id_wraps_validation_error() {
        let e = make_dag_id("has space".into()).expect_err("must reject");
        match e {
            ParseError::InvalidIdentifier { kind, value, .. } => {
                assert_eq!(kind, "dag_id");
                assert_eq!(value, "has space");
            }
            other => panic!("expected InvalidIdentifier, got {other:?}"),
        }
    }

    #[test]
    fn push_unique_task_keeps_first_seen_order() {
        let mut v = Vec::<TaskId>::new();
        push_unique_task(&mut v, TaskId::new("x").unwrap());
        push_unique_task(&mut v, TaskId::new("y").unwrap());
        push_unique_task(&mut v, TaskId::new("x").unwrap());
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].as_str(), "x");
        assert_eq!(v[1].as_str(), "y");
    }
}
