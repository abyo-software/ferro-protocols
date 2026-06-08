// SPDX-License-Identifier: Apache-2.0
//! Cargo crate name validator.
//!
//! Reference: `doc.rust-lang.org/cargo/reference/manifest.html#the-name-field`
//! and the registry API under "Crate name restrictions".
//!
//! Rules:
//! - 1–64 characters inclusive.
//! - First character must be an ASCII letter.
//! - Remaining characters must be ASCII alphanumeric, `-`, or `_`.
//! - Case-insensitive comparisons (the index uses lowercased names),
//!   though the canonical form from the manifest is preserved verbatim.

use crate::error::CargoError;

/// Maximum crate name length enforced by crates.io (registry spec).
pub const MAX_NAME_LEN: usize = 64;

/// Returns true if `name` satisfies the Cargo crate-name rules.
#[must_use]
pub fn is_valid_name(name: &str) -> bool {
    if name.is_empty() || name.len() > MAX_NAME_LEN {
        return false;
    }
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.is_ascii_alphabetic() {
        return false;
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Validate a crate name, returning a typed error on rejection.
///
/// # Errors
/// Returns [`CargoError::InvalidName`] when the name does not satisfy
/// [`is_valid_name`].
pub fn validate_name(name: &str) -> Result<(), CargoError> {
    if is_valid_name(name) {
        Ok(())
    } else {
        Err(CargoError::InvalidName(name.to_owned()))
    }
}

/// Canonicalize a crate name to its storage / index key.
///
/// Cargo treats crate names case-insensitively and considers `-` and `_`
/// interchangeable for *uniqueness* (a registry must not host both
/// `foo-bar` and `foo_bar`). The canonical key therefore lowercases the
/// name and folds `-` to `_`. This is the key under which records are
/// stored and the form the sparse-index path is derived from, so a
/// mixed-case publish (`MyCrate`) is retrievable at the lowercase path
/// (`my/cr/mycrate`) cargo actually requests.
///
/// The original display case is preserved separately in the
/// [`IndexEntry::name`](crate::index::IndexEntry) field.
#[must_use]
pub fn canonical_name(name: &str) -> String {
    name.to_ascii_lowercase().replace('-', "_")
}

/// Whether two crate names collide under cargo's uniqueness rules
/// (case-insensitive, `-`/`_`-insensitive).
#[must_use]
pub fn names_collide(a: &str, b: &str) -> bool {
    canonical_name(a) == canonical_name(b)
}

/// Compute the sparse-index path for `name`.
///
/// Reference: `doc.rust-lang.org/cargo/reference/registries.html#index-format`.
///
/// Layout rules:
/// - 1-char names → `1/{name}`
/// - 2-char names → `2/{name}`
/// - 3-char names → `3/{first-char}/{name}`
/// - 4+-char names → `{first-two}/{second-two}/{name}`
///
/// All prefixes are lowercased; the stored filename preserves the
/// original case because Cargo's `cksum` input uses the canonical name.
#[must_use]
pub fn index_path(name: &str) -> String {
    let lower = name.to_ascii_lowercase();
    match lower.len() {
        0 => String::new(),
        1 => format!("1/{lower}"),
        2 => format!("2/{lower}"),
        3 => format!("3/{}/{}", &lower[0..1], lower),
        _ => format!("{}/{}/{}", &lower[0..2], &lower[2..4], lower),
    }
}

#[cfg(test)]
mod tests {
    use super::{index_path, is_valid_name, validate_name};

    #[test]
    fn valid_crate_names() {
        assert!(is_valid_name("serde"));
        assert!(is_valid_name("tokio"));
        assert!(is_valid_name("abc-def"));
        assert!(is_valid_name("abc_def"));
        assert!(is_valid_name("A")); // single letter, 1 char.
    }

    #[test]
    fn invalid_crate_names() {
        assert!(!is_valid_name(""));
        assert!(!is_valid_name("-foo"));
        assert!(!is_valid_name("1abc"));
        assert!(!is_valid_name("foo bar"));
        assert!(!is_valid_name(&"a".repeat(65)));
    }

    #[test]
    fn validate_surface_error() {
        let e = validate_name("").unwrap_err();
        assert!(matches!(e, crate::error::CargoError::InvalidName(_)));
    }

    #[test]
    fn index_path_layout_by_length() {
        assert_eq!(index_path("a"), "1/a");
        assert_eq!(index_path("ab"), "2/ab");
        assert_eq!(index_path("abc"), "3/a/abc");
        assert_eq!(index_path("serde"), "se/rd/serde");
        // 4-char boundary.
        assert_eq!(index_path("tokio"), "to/ki/tokio");
        assert_eq!(index_path("abcd"), "ab/cd/abcd");
    }

    #[test]
    fn index_path_lowercases() {
        assert_eq!(index_path("Serde"), "se/rd/serde");
        assert_eq!(index_path("AB"), "2/ab");
    }

    /// F5: the canonical key lowercases and folds `-`→`_` so cargo's
    /// uniqueness rules are honoured.
    #[test]
    fn canonical_name_folds_case_and_hyphen() {
        use super::{canonical_name, names_collide};
        assert_eq!(canonical_name("MyCrate"), "mycrate");
        assert_eq!(canonical_name("Foo-Bar"), "foo_bar");
        assert_eq!(canonical_name("foo_bar"), "foo_bar");
        assert!(names_collide("foo-bar", "foo_bar"));
        assert!(names_collide("Foo", "foo"));
        assert!(names_collide("My-Crate", "my_crate"));
        assert!(!names_collide("foo", "bar"));
    }
}
