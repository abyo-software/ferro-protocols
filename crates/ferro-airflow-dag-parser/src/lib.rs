// SPDX-License-Identifier: Apache-2.0
#![doc = include_str!("../README.md")]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![cfg_attr(not(feature = "parser-ruff"), allow(dead_code, unused_imports))]
#![deny(missing_docs)]

/// Crate name, exposed for diagnostics and `/metrics` labelling.
pub const CRATE_NAME: &str = "ferro-airflow-dag-parser";

pub mod common;
pub mod line_index;

#[cfg(feature = "parser-ruff")]
mod panic_safe;

#[cfg(feature = "parser-ruff")]
#[cfg_attr(docsrs, doc(cfg(feature = "parser-ruff")))]
pub mod api;

#[cfg(feature = "parser-ruff")]
#[cfg_attr(docsrs, doc(cfg(feature = "parser-ruff")))]
pub mod cache;

#[cfg(feature = "parser-ruff")]
#[cfg_attr(docsrs, doc(cfg(feature = "parser-ruff")))]
pub mod dynamic_markers;

#[cfg(feature = "parser-ruff")]
#[cfg_attr(docsrs, doc(cfg(feature = "parser-ruff")))]
pub mod ruff_impl;

pub use common::{DagId, ExtractedDag, IdentifierError, ParseError, SourceSpan, TaskId};

#[cfg(feature = "parser-ruff")]
pub use api::{
    ParseOutcome, dynamic_markers_for, extract_all_static_dags, extract_static_dag, parse_dag_path,
};

#[cfg(feature = "parser-ruff")]
pub use cache::ParseCache;

#[cfg(feature = "parser-ruff")]
pub use dynamic_markers::{DynamicMarker, detect_dynamic_markers};
