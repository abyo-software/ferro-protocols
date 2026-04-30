// SPDX-License-Identifier: Apache-2.0
//! Fuzz target: feed arbitrary bytes into `<Digest as FromStr>::from_str`
//! and confirm it never panics. Covers the `<algo>:<hex>` wire form
//! parser (separator detection, algorithm prefix lookup, hex length /
//! charset validation) shared by OCI manifests, Maven sidecars, and
//! Cargo cksums.
//!
//! As a defence-in-depth check, when the input parses successfully we
//! re-render via `Display` and round-trip parse it; the second parse
//! must succeed and yield an equal value. This catches Display/FromStr
//! drift on adversarial-but-valid inputs (e.g. mixed-case hex).

#![no_main]

use ferro_blob_store::Digest;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(s) = std::str::from_utf8(data) else {
        return;
    };
    if let Ok(d) = s.parse::<Digest>() {
        let rendered = d.to_string();
        let reparsed: Digest = rendered.parse().expect("Display output must reparse");
        assert_eq!(d, reparsed, "Digest round-trip drift");
    }
});
