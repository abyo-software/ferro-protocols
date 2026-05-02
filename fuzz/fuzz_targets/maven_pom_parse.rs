// SPDX-License-Identifier: Apache-2.0
//! Focused fuzz target for the Maven `pom.xml` parser.
//!
//! `parse_pom` (`crates/ferro-maven-layout/src/pom.rs:55`) is exposed
//! at every Maven artifact `PUT` upload boundary — a malformed POM
//! must never panic, hang, or OOM the registry server. The parser
//! delegates to `quick_xml::de::from_str` for the heavy lifting
//! (so this target also indirectly fuzzes our quick-xml dependency
//! shape), then re-validates `groupId` / `artifactId` / `version`
//! after applying the parent-fallback rule.
//!
//! Watching for: serde+quick-xml deserialiser panics on malformed
//! XML, billion-laughs / external-entity DoS shapes (quick-xml is
//! supposed to be entity-disabled by default; this fuzz catches a
//! regression there), Vec/String OOM from huge inner element counts,
//! and any panic on the `unwrap_or_default()` fallback path when
//! `<parent>` is partially populated.

#![no_main]

use libfuzzer_sys::fuzz_target;

use ferro_maven_layout::parse_pom;

fuzz_target!(|data: &[u8]| {
    let Ok(s) = std::str::from_utf8(data) else {
        return;
    };
    let _ = parse_pom(s);
});
