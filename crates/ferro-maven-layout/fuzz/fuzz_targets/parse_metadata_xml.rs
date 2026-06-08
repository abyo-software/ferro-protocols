// SPDX-License-Identifier: Apache-2.0
//! Focused fuzz target for the Maven `maven-metadata.xml` parser.
//!
//! `MavenMetadata::from_xml`
//! (`crates/ferro-maven-layout/src/metadata.rs`) is exposed at the
//! Maven `maven-metadata.xml` `PUT`/`GET` handler boundary
//! (`handlers.rs:252`) — a malformed metadata body must never panic,
//! hang, or OOM the registry server. The parser delegates to
//! `quick_xml::de::from_str` (via the shared
//! `xml::from_str_panic_safe` shim) for the heavy lifting, so this
//! target also indirectly fuzzes our quick-xml dependency shape.
//!
//! This is the DoS-parity companion to the `maven_pom_parse` target:
//! both `pom.xml` and `maven-metadata.xml` are attacker-supplied XML
//! bodies routed through the same `quick_xml::de` deserialiser, and
//! both must survive its 0.39.2 `unreachable!()`
//! (`src/de/mod.rs:2903`) panic on malformed input by surfacing a
//! clean `MavenError` instead of aborting.
//!
//! ## Why we override the panic hook
//!
//! `libfuzzer-sys` installs a global panic hook in `initialize()` that
//! aborts on every panic, before any `catch_unwind` further up the
//! stack runs. The production `from_xml` shim catches quick-xml's
//! `unreachable!()` panics and surfaces them as
//! `MavenError::InvalidMetadata` — but in the fuzz binary that
//! `catch_unwind` never gets a chance because libfuzzer's hook aborts
//! first. Installing a no-op hook (Once-guarded) lets the production
//! shim run to completion just as it does for real callers. A genuine
//! escape (shim bypassed via SIGSEGV) still aborts via the non-panic
//! path.

#![no_main]

use std::sync::Once;

use libfuzzer_sys::fuzz_target;

use ferro_maven_layout::MavenMetadata;

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
    let _ = MavenMetadata::from_xml(s);
});
