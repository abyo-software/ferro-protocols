// SPDX-License-Identifier: Apache-2.0
//! Panic-shielded `quick-xml` deserialisation helper.
//!
//! Every attacker-reachable XML body in this crate (the Maven `pom.xml`
//! upload boundary and the `maven-metadata.xml` PUT/GET boundary) is
//! deserialised through [`from_str_panic_safe`].
//!
//! # Why `catch_unwind`
//!
//! `quick_xml::de` 0.39.2 hits an `unreachable!()` macro at
//! `quick-xml-0.39.2/src/de/mod.rs:2903:37` (`internal error: entered
//! unreachable code`) on certain malformed inputs (e.g. mixed-token
//! `<><groupId\tp...\n<!DOCTYPe\t;:="0"1"...` shapes — see the
//! 2026-05-15 fuzz artifact `crash-1ceeadf1`). That panic propagates
//! past any `?`/`map_err` conversion and aborts the request thread.
//! Production callers (the `ferro-maven-server` registry PUT handlers)
//! must never abort on attacker-supplied bodies, so the panic is
//! converted into a caller-supplied error value just like every other
//! parse failure.

use std::panic::AssertUnwindSafe;

use serde::de::DeserializeOwned;

/// Deserialise `xml` into `T`, converting both ordinary parse errors
/// **and** `quick-xml` deserialiser panics into `E` via `on_err`.
///
/// `on_err` is called with a human-readable message:
///
/// - `"XML parse failed: {e}"` when `quick-xml` returns a parse error;
/// - `"XML parser panicked on malformed input: {msg}"` when `quick-xml`
///   hits its internal `unreachable!()` (the downcasted panic payload
///   is preserved for debugging).
///
/// # Errors
///
/// Returns `Err(on_err(..))` on any parse error or recovered panic.
pub(crate) fn from_str_panic_safe<T, E>(
    xml: &str,
    on_err: impl Fn(String) -> E,
) -> Result<T, E>
where
    T: DeserializeOwned,
{
    // `AssertUnwindSafe` is sound here: `from_str` borrows `xml`
    // immutably and produces an owned `T`; nothing observable is left
    // in a torn state if it unwinds.
    let parse_result =
        std::panic::catch_unwind(AssertUnwindSafe(|| quick_xml::de::from_str::<T>(xml)));
    match parse_result {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(e)) => Err(on_err(format!("XML parse failed: {e}"))),
        Err(panic_payload) => {
            let msg = if let Some(s) = panic_payload.downcast_ref::<&str>() {
                (*s).to_string()
            } else if let Some(s) = panic_payload.downcast_ref::<String>() {
                s.clone()
            } else {
                "non-string panic payload".to_string()
            };
            Err(on_err(format!(
                "XML parser panicked on malformed input: {msg}"
            )))
        }
    }
}
