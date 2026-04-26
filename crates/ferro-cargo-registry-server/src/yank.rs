// SPDX-License-Identifier: Apache-2.0
//! Yank / unyank response shapes.
//!
//! Reference:
//! `doc.rust-lang.org/cargo/reference/registry-web-api.html#yank`.
//!
//! A yank transitions the published version to "not selectable by a
//! fresh resolve" but keeps the tarball downloadable so lock-files
//! continue to work. The index entry's `yanked` field flips between
//! `true` and `false`.

use serde::{Deserialize, Serialize};

/// JSON body returned by both `yank` and `unyank`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct YankResponse {
    /// Always `true` on success.
    pub ok: bool,
}

impl YankResponse {
    /// Return the fixed `{ "ok": true }` body.
    #[must_use]
    pub const fn ok() -> Self {
        Self { ok: true }
    }
}

#[cfg(test)]
mod tests {
    use super::YankResponse;

    #[test]
    fn ok_body_matches_spec() {
        let r = YankResponse::ok();
        let s = serde_json::to_string(&r).unwrap();
        assert_eq!(s, "{\"ok\":true}");
    }
}
