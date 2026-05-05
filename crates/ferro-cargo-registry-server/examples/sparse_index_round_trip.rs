// SPDX-License-Identifier: Apache-2.0
//! Round-trip the sparse-index line format that Cargo fetches from
//! `/index/{prefix}/{name}` URLs.
//!
//! Each line is a complete JSON object representing one published
//! version. Cargo iterates the file line-by-line, oldest first.
//!
//! Demonstrates:
//!
//! - Building two [`IndexEntry`] values directly (the common case in
//!   tests and admin tooling).
//! - Serialising them to the wire format with [`render_lines`].
//! - Parsing the wire bytes back with [`parse_lines`] and confirming
//!   round-trip equality.
//! - Where the index file would live on disk for a name long enough to
//!   trigger the canonical sharded layout (via [`index_path`]).
//!
//! Run with:
//!
//! ```bash
//! cargo run --example sparse_index_round_trip -p ferro-cargo-registry-server
//! ```

use ferro_cargo_registry_server::{IndexDep, IndexEntry, index_path, parse_lines, render_lines};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let v1 = IndexEntry {
        name: "ferro-example".into(),
        vers: "0.1.0".into(),
        deps: vec![IndexDep {
            name: "serde".into(),
            req: "^1".into(),
            features: vec!["derive".into()],
            optional: false,
            default_features: true,
            target: None,
            kind: None,
            registry: None,
            package: None,
        }],
        cksum: "0".repeat(64),
        features: Default::default(),
        yanked: false,
        links: None,
        v: Some(2),
        features2: None,
        rust_version: Some("1.88".into()),
    };

    let v2 = IndexEntry {
        vers: "0.1.1".into(),
        cksum: "1".repeat(64),
        ..v1.clone()
    };

    let wire = render_lines(&[v1.clone(), v2.clone()]);
    println!("wire format ({} bytes):", wire.len());
    for line in wire.lines() {
        println!("  {line}");
    }

    let parsed = parse_lines(&wire)?;
    assert_eq!(parsed, vec![v1.clone(), v2]);
    println!("round-trip OK ({} entries)", parsed.len());

    // index_path returns the on-disk shard for the canonical sparse
    // layout. ferro-example is 13 chars so it lands under `fe/rr/`.
    let on_disk = index_path(&v1.name);
    println!("on-disk path: {on_disk}");

    Ok(())
}
