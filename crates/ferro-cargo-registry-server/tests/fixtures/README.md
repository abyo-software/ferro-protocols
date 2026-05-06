# Cargo registry conformance fixtures

Real upstream-derived sparse-index payloads used by `tests/conformance.rs`.

## Sources

- `serde-index.json` — extracted lines from the real crates.io
  sparse index entry for the `serde` crate.
  Source: <https://index.crates.io/se/rd/serde>
  Includes the `1.0.0` first-published line and the `1.0.219` latest
  line (a representative sample; the live index file currently has
  ~300 lines for `serde`).
- `anyhow-index.json` — extracted lines from the real crates.io
  sparse index entry for the `anyhow` crate.
  Source: <https://index.crates.io/an/yh/anyhow>
  Includes `1.0.0` and a current line pinned to `1.0.99` with the v2
  feature-map and `dep:` resolver markers.
- `config.json` — verbatim copy of the live crates.io
  `config.json` shape published at the root of the sparse index.
  Source: <https://index.crates.io/config.json>

License compliance: each line of the sparse index is metadata about a
public crate (name / version / dep edges / sha256), not the crate's
source code. crates.io's policy permits redistribution of registry
metadata. The crate fixtures themselves carry no copyrightable content
beyond identifiers and version requirements that are public facts.

## Hand-derivation note

The `cksum` values shown are real published sha256 fingerprints of the
upstream `.crate` tarballs. Line ordering, `v: 2` marker, `features2`
dep prefixes, and `rust_version` fields all match the live shape Cargo
1.78+ emits.
