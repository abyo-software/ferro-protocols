// SPDX-License-Identifier: Apache-2.0
//! Fuzz target: feed arbitrary UTF-8 strings into
//! `parse_layout_path`. This exercises:
//!
//! - segment splitting (with and without leading slashes);
//! - `maven-metadata.xml` detection plus optional `.{algo}` sidecar
//!   suffix matching;
//! - `{artifactId}-{version}[-{classifier}].{extension}` filename
//!   splitting, including the compound `tar.gz` / `tar.bz2` /
//!   `tar.xz` / `tar.zst` extension handling;
//! - `Coordinate::new` field validation (illegal-character rejection
//!   on slash / backslash / colon).
//!
//! The function must return `Ok` or `Err`, never panic. When parsing
//! succeeds we additionally re-render via `Coordinate::repository_path`
//! for `PathClass::Artifact` and require the result to be a non-empty
//! string with no `..` traversal segment.

#![no_main]

use ferro_maven_layout::{parse_layout_path, PathClass};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(s) = std::str::from_utf8(data) else {
        return;
    };
    if let Ok(parsed) = parse_layout_path(s) {
        if matches!(parsed.class, PathClass::Artifact | PathClass::Checksum(_)) {
            let rendered = parsed.coordinate.repository_path();
            assert!(!rendered.is_empty());
            // Defence in depth: parsed coordinates must not embed a
            // path-traversal segment in any of their components.
            for seg in rendered.split('/') {
                assert_ne!(seg, "..");
            }
        }
    }
});
