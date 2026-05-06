// SPDX-License-Identifier: Apache-2.0
//! Conformance tests against vendored real crates.io sparse-index entries.
//!
//! These exercise the index parser and the `config.json` deserialiser
//! against the live shapes Cargo 1.78+ pulls from
//! `https://index.crates.io/`.
//!
//! Source URLs and license attribution: see `tests/fixtures/README.md`.

use ferro_cargo_registry_server::{IndexConfig, parse_lines, render_lines};

const SERDE_INDEX: &str = include_str!("fixtures/serde-index.json");
const ANYHOW_INDEX: &str = include_str!("fixtures/anyhow-index.json");
const CONFIG_JSON: &str = include_str!("fixtures/config.json");

#[test]
fn upstream_serde_index_parses() {
    let entries = parse_lines(SERDE_INDEX).expect("serde index lines parse");
    assert_eq!(entries.len(), 2, "fixture exposes 1.0.0 + 1.0.219 lines");
    assert!(entries.iter().all(|e| e.name == "serde"));

    let v100 = &entries[0];
    let v_latest = &entries[1];
    assert_eq!(v100.vers, "1.0.0");
    assert_eq!(v_latest.vers, "1.0.219");
    // The 1.0.0 line predates the v2 feature-map marker; the latest
    // line carries `v: 2` plus a `features2` block keyed on `dep:`.
    assert_eq!(v100.v, None);
    assert_eq!(v_latest.v, Some(2));
    assert!(
        v_latest.features2.is_some(),
        "latest serde line has v2 features2 block",
    );
    // The optional `serde_derive` dependency edge is present in both
    // lines, gated on the `derive` feature.
    let dep = v100.deps.iter().find(|d| d.name == "serde_derive");
    assert!(
        dep.is_some(),
        "1.0.0 should have an optional serde_derive dep"
    );
    assert!(dep.expect("dep").optional);
}

#[test]
fn upstream_anyhow_index_parses_with_dev_deps() {
    let entries = parse_lines(ANYHOW_INDEX).expect("anyhow index lines parse");
    assert_eq!(entries.len(), 2);
    let latest = entries
        .iter()
        .find(|e| e.vers == "1.0.99")
        .expect("1.0.99 line present");

    // Dev-deps are encoded with `kind: "dev"` in the same `deps` array
    // as runtime deps; the parser must accept them transparently.
    let dev_count = latest
        .deps
        .iter()
        .filter(|d| d.kind.as_deref() == Some("dev"))
        .count();
    assert!(
        dev_count >= 4,
        "anyhow 1.0.99 has 4+ dev-deps (futures-core, syn, thiserror, trybuild, rustversion)",
    );

    // The `package` rename field — futures-core is pulled in *as*
    // `futures` via `package`. The parser must preserve that field.
    let renamed = latest.deps.iter().find(|d| d.package.is_some());
    assert!(
        renamed.is_some(),
        "anyhow 1.0.99 has at least one renamed dependency (futures → futures-core)",
    );
}

#[test]
fn upstream_index_lines_render_back_to_canonical_shape() {
    // Round-trip: parse → render → parse again. The conformance contract
    // is that an intermediary that forwards index lines must not
    // perturb their semantic content; line-order and cksums must
    // round-trip exactly.
    let entries_in = parse_lines(SERDE_INDEX).expect("parse 1");
    let rendered = render_lines(&entries_in);
    let entries_out = parse_lines(&rendered).expect("parse 2");
    assert_eq!(entries_in, entries_out);
}

#[test]
fn upstream_config_json_parses_into_index_config() {
    let cfg: IndexConfig = serde_json::from_str(CONFIG_JSON).expect("config.json parses");
    assert_eq!(cfg.dl, "https://crates.io/api/v1/crates");
    assert_eq!(cfg.api, "https://crates.io");
    // `auth-required` is absent from the public crates.io config; the
    // serde default must populate `false` rather than failing.
    assert!(!cfg.auth_required);
}

#[test]
fn upstream_index_lines_contain_no_embedded_newlines() {
    // Cargo iterates the sparse-index payload line-by-line; embedded
    // newlines inside JSON values would split a single entry across
    // multiple lines and corrupt the parse.
    for line in SERDE_INDEX.lines() {
        assert!(!line.contains('\r'), "no CR allowed: {line}");
    }
    for line in ANYHOW_INDEX.lines() {
        assert!(!line.contains('\r'), "no CR allowed: {line}");
    }
}
