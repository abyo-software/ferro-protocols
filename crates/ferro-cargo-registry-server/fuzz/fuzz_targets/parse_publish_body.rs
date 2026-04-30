// SPDX-License-Identifier: Apache-2.0
//! Fuzz target: feed arbitrary bytes into the Cargo publish body
//! parser. Surface area covered:
//!
//! - the 4-byte little-endian metadata_len framing;
//! - bounds checks against truncated metadata / tarball bodies;
//! - `serde_json` parse of the embedded manifest;
//! - the second 4-byte crate_len frame.
//!
//! `parse_publish_body` (re-exported as `parse`) returns
//! `Result<PublishRequest, CargoError>` and must never panic for any
//! input. The sparse-index name validator and `index_path` builder are
//! also exercised when the manifest happens to carry a usable `name`
//! field — that exposes the second-most-adversarial parser surface in
//! the crate (length-bounded ASCII grammar + path-segment rendering)
//! through the same harness.

#![no_main]

use ferro_cargo_registry_server::{
    index_path, is_valid_name, parse_publish_body, MAX_NAME_LEN,
};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(req) = parse_publish_body(data) {
        if let Some(name) = req.manifest.get("name").and_then(|v| v.as_str()) {
            if is_valid_name(name) {
                assert!(name.len() <= MAX_NAME_LEN);
                let path = index_path(name);
                assert!(!path.is_empty());
                // The trailing path segment is always the lower-cased
                // name, regardless of which length bucket fired.
                assert!(path.ends_with(&name.to_ascii_lowercase()));
            } else {
                // Invalid names must produce a path that contains no
                // path-traversal escape characters.
                let path = index_path(name);
                assert!(!path.contains(".."));
            }
        }
    }
});
