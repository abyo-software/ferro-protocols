// SPDX-License-Identifier: Apache-2.0
//! Demonstrates the codec-only types in `ferro-maven-layout`: layout-path
//! parsing, GAV coordinates, checksum sidecar parsing, and SNAPSHOT
//! timestamp formatting.
//!
//! No HTTP server, no Tokio — just the spec-mapped types. Run with the
//! default `http` feature off to prove the parsing layer is free of any
//! Axum / Tokio surface:
//!
//! ```bash
//! cargo run --example parse_layout -p ferro-maven-layout --no-default-features
//! ```

use ferro_maven_layout::{
    ChecksumAlgo, Coordinate, PathClass, SnapshotTimestamp, compute_checksum, is_snapshot_version,
    parse_layout_path, parse_sidecar,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // ----- Coordinate construction + parsing ------------------------------

    let coord = Coordinate::new_jar("com.example.foo", "bar-api", "1.2.3")?;
    println!("coord: {coord}");
    println!("  group_id    = {}", coord.group_id);
    println!("  artifact_id = {}", coord.artifact_id);
    println!("  version     = {}", coord.version);
    println!("  is_snapshot = {}", is_snapshot_version(&coord.version));

    // ----- Layout path parsing --------------------------------------------

    let paths = [
        "com/example/foo/bar-api/1.2.3/bar-api-1.2.3.jar",
        "com/example/foo/bar-api/1.2.3/bar-api-1.2.3.jar.sha256",
        "com/example/foo/bar-api/maven-metadata.xml",
        "com/example/foo/bar-api/1.2.3-SNAPSHOT/maven-metadata.xml",
    ];
    for p in paths {
        match parse_layout_path(p) {
            Ok(parsed) => match parsed.class {
                PathClass::Artifact => println!("ARTIFACT  {p} -> {}", parsed.coordinate),
                PathClass::Checksum(algo) => {
                    println!("SIDECAR   {p} -> {algo:?} for {}", parsed.coordinate);
                }
                PathClass::Metadata {
                    version_level,
                    checksum,
                } => {
                    println!(
                        "METADATA  {p} (version_level={version_level}, checksum={checksum:?})",
                    );
                }
            },
            Err(e) => println!("REJECT    {p} -> {e}"),
        }
    }

    // ----- Checksum sidecar round-trip ------------------------------------

    let body = b"Hello, Maven Central!";
    let hex = compute_checksum(ChecksumAlgo::Sha256, body).expect("sha-256 always produces a hex");
    let sidecar = format!("{hex}  bar-api-1.2.3.jar\n");
    let parsed_hex = parse_sidecar(ChecksumAlgo::Sha256, sidecar.as_bytes())?;
    assert_eq!(parsed_hex, hex);
    println!("checksum (SHA-256): {hex}");
    println!("sidecar parsed back: {parsed_hex}");

    // ----- SNAPSHOT timestamp composition ---------------------------------

    let ts = SnapshotTimestamp::now();
    let composed = ts.compose_version("1.2.3-SNAPSHOT", 1);
    println!("snapshot timestamp version: {composed}");

    Ok(())
}
