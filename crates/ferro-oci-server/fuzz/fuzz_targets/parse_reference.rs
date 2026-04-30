// SPDX-License-Identifier: Apache-2.0
//! Fuzz target: feed arbitrary UTF-8 input into the OCI Distribution
//! Spec parsers most likely to be reachable by an attacker via URL path
//! parameters:
//!
//! - `Reference::from_str` parses `{reference}` from
//!   `/v2/{name}/manifests/{reference}` and `/v2/{name}/blobs/{digest}`.
//!   It internally dispatches to either the `Digest` parser
//!   (`<algo>:<hex>`) or the tag grammar
//!   (`[a-zA-Z0-9_][a-zA-Z0-9._-]{0,127}`).
//! - `validate_name` parses `{name}` from the same URLs and enforces
//!   the OCI v1.1 §2 repository-name grammar (component-wise, with
//!   `.`, `_`, `__`, and `-+` separators).
//!
//! Neither must panic on any input. Successful parses additionally
//! round-trip through `Display` to catch wire-form drift.

#![no_main]

use ferro_oci_server::{validate_name, Reference, MAX_NAME_LENGTH, MAX_TAG_LENGTH};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(s) = std::str::from_utf8(data) else {
        return;
    };

    // 1) Reference parser (digest or tag).
    if let Ok(r) = s.parse::<Reference>() {
        let rendered = r.to_string();
        let reparsed: Reference = rendered.parse().expect("Reference round-trip");
        assert_eq!(r, reparsed, "Reference Display/FromStr drift");
        if let Some(t) = r.as_tag() {
            assert!(!t.is_empty());
            assert!(t.len() <= MAX_TAG_LENGTH);
        }
    }

    // 2) Repository-name validator.
    if validate_name(s).is_ok() {
        assert!(!s.is_empty());
        assert!(s.len() <= MAX_NAME_LENGTH);
        // No path-traversal segment can ever validate.
        for component in s.split('/') {
            assert_ne!(component, "..");
            assert_ne!(component, ".");
            assert!(!component.is_empty());
        }
    }
});
