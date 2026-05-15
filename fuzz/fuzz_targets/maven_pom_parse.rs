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
//!
//! ## Why we override the panic hook
//!
//! `libfuzzer-sys` installs a global panic hook in `initialize()` that
//! aborts on every panic, before any `catch_unwind` further up the
//! stack runs. The production `parse_pom` shim catches quick-xml's
//! `unreachable!()` panics (e.g. `quick-xml-0.39.2/src/de/mod.rs:2903`)
//! and surfaces them as `MavenError::InvalidPom` — but in the fuzz
//! binary that catch_unwind never gets a chance because libfuzzer's
//! hook aborts first. Installing a no-op hook (Once-guarded) lets the
//! production shim run to completion just as it does for real callers.
//! A genuine escape (shim bypassed via SIGSEGV) still aborts via the
//! non-panic path.

#![no_main]

use std::sync::Once;

use libfuzzer_sys::fuzz_target;

use ferro_maven_layout::parse_pom;

static INSTALL_HOOK: Once = Once::new();

fn install_silent_panic_hook() {
    INSTALL_HOOK.call_once(|| {
        std::panic::set_hook(Box::new(|_info| {}));
    });
}

fuzz_target!(|data: &[u8]| {
    install_silent_panic_hook();
    let Ok(s) = std::str::from_utf8(data) else {
        return;
    };
    let _ = parse_pom(s);
});
