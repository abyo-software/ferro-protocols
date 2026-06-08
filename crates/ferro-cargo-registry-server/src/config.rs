// SPDX-License-Identifier: Apache-2.0
//! Sparse-index `config.json` representation.
//!
//! Reference:
//! `doc.rust-lang.org/cargo/reference/registries.html#index-configuration`.
//!
//! Cargo fetches `config.json` at the root of the index and uses it to
//! discover the download and API URLs. `FerroRepo` serves the following
//! fields:
//!
//! ```json
//! {
//!   "dl":   "/api/v1/crates/{crate}/{version}/download",
//!   "api":  "<host>",
//!   "auth-required": false
//! }
//! ```

use serde::{Deserialize, Serialize};

/// Root-level sparse index configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexConfig {
    /// Download URL template. `{crate}` and `{version}` are substituted
    /// by the client.
    pub dl: String,
    /// Base URL for the Registry Web API.
    pub api: String,
    /// Whether all requests must carry an `Authorization` header.
    #[serde(rename = "auth-required", default)]
    pub auth_required: bool,
}

impl IndexConfig {
    /// Build the default configuration pinned to `api_host`.
    ///
    /// The `dl` download template is rendered as an absolute URL rooted
    /// at `api_host`. Cargo requires `dl` to resolve to a fetchable URL;
    /// when `api_host` is a real origin (for example
    /// `http://127.0.0.1:8081`) the resulting template
    /// `http://127.0.0.1:8081/api/v1/crates/{crate}/{version}/download`
    /// is what a stock `cargo fetch` downloads from. If `api_host` is
    /// empty the template degrades to the bare server-relative path.
    #[must_use]
    pub fn new(api_host: impl Into<String>) -> Self {
        let api = api_host.into();
        let base = api.trim_end_matches('/');
        let dl = format!("{base}/api/v1/crates/{{crate}}/{{version}}/download");
        Self {
            dl,
            api,
            auth_required: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::IndexConfig;

    #[test]
    fn config_json_round_trip() {
        let c = IndexConfig::new("http://localhost:8080");
        let s = serde_json::to_string(&c).unwrap();
        let c2: IndexConfig = serde_json::from_str(&s).unwrap();
        assert_eq!(c, c2);
    }

    #[test]
    fn dl_template_matches_spec() {
        let c = IndexConfig::new("x");
        assert!(c.dl.contains("{crate}"));
        assert!(c.dl.contains("{version}"));
    }
}
