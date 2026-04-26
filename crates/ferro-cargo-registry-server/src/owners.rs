// SPDX-License-Identifier: Apache-2.0
//! Owner-management response shapes.
//!
//! Reference:
//! `doc.rust-lang.org/cargo/reference/registry-web-api.html#owners`.

use serde::{Deserialize, Serialize};

/// A single registered crate owner.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Owner {
    /// Numeric owner id.
    pub id: u64,
    /// Human-readable login (e.g. `github-login`).
    pub login: String,
    /// Optional full name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// `GET /api/v1/crates/{crate}/owners` response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OwnersResponse {
    /// Current owner list.
    pub users: Vec<Owner>,
}

/// `PUT` / `DELETE` body for owner add / remove.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OwnersRequest {
    /// List of login strings to add / remove.
    pub users: Vec<String>,
}

/// `PUT` / `DELETE` response body — `ok` plus an optional message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OwnersMutationResponse {
    /// `true` on success.
    pub ok: bool,
    /// Message surfaced to the user on add.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub msg: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::{Owner, OwnersMutationResponse, OwnersRequest, OwnersResponse};

    #[test]
    fn owners_response_round_trips() {
        let r = OwnersResponse {
            users: vec![Owner {
                id: 1,
                login: "abyo".into(),
                name: Some("Youichi".into()),
            }],
        };
        let s = serde_json::to_string(&r).unwrap();
        let r2: OwnersResponse = serde_json::from_str(&s).unwrap();
        assert_eq!(r, r2);
    }

    #[test]
    fn owners_request_is_a_bare_users_list() {
        let r = OwnersRequest {
            users: vec!["alice".into(), "bob".into()],
        };
        let s = serde_json::to_string(&r).unwrap();
        assert!(s.contains("\"users\""));
    }

    #[test]
    fn mutation_response_carries_optional_msg() {
        let r = OwnersMutationResponse {
            ok: true,
            msg: Some("user added".into()),
        };
        let s = serde_json::to_string(&r).unwrap();
        assert!(s.contains("msg"));
    }
}
