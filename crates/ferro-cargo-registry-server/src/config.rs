// SPDX-License-Identifier: Apache-2.0
//! Sparse-index `config.json` representation.
//!
//! Reference:
//! `doc.rust-lang.org/cargo/reference/registries.html#index-configuration`.
//!
//! Cargo fetches `config.json` at the root of the index and uses it to
//! discover the download and API URLs. FerroRepo serves the following
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
    /// Build the default Phase 1 configuration pinned to `api_host`.
    #[must_use]
    pub fn new(api_host: impl Into<String>) -> Self {
        Self {
            dl: "/api/v1/crates/{crate}/{version}/download".to_owned(),
            api: api_host.into(),
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
