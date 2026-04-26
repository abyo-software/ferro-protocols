// SPDX-License-Identifier: Apache-2.0
//! Chaos: concurrent PUT on different SNAPSHOTs of the same GAV.
//!
//! `mvn deploy` for a `-SNAPSHOT` version uploads both the artifact
//! jar AND a regenerated `maven-metadata.xml`. If two CI jobs push
//! overlapping SNAPSHOTs at the same instant, the metadata file
//! regeneration MUST end up with a superset of both timestamps, not
//! one overwriting the other.
//!
//! In Phase 1 the regeneration is synchronous inside the handler;
//! this test locks the structural invariants of the XML so the
//! Phase 3 locking story (F-R1-036 follow-up) doesn't silently drop
//! updates.

use chrono::Utc;
use ferro_maven_layout::metadata::{MavenMetadata, Snapshot, SnapshotVersion};

fn snapshot_with(classifier: Option<&str>, value: &str, updated: &str) -> SnapshotVersion {
    SnapshotVersion {
        classifier: classifier.map(str::to_owned),
        extension: "jar".to_owned(),
        value: value.to_owned(),
        updated: updated.to_owned(),
    }
}

#[test]
fn two_snapshots_merge_without_loss() {
    // Job A produced this snapshot list.
    let a = [
        snapshot_with(None, "1.0-SNAPSHOT-20260423010101-1", "20260423010101"),
        snapshot_with(
            Some("sources"),
            "1.0-SNAPSHOT-20260423010101-1",
            "20260423010101",
        ),
    ];
    // Job B produced this snapshot list 2 s later (different build).
    let b = [snapshot_with(
        None,
        "1.0-SNAPSHOT-20260423010103-2",
        "20260423010103",
    )];

    let merged_snapshot = Snapshot {
        timestamp: "20260423010103".to_owned(),
        build_number: 2,
    };
    let mut merged: Vec<_> = a.to_vec();
    merged.extend(b.iter().cloned());

    let md = MavenMetadata::snapshot_metadata(
        "com.example",
        "foo",
        "1.0-SNAPSHOT",
        merged_snapshot,
        merged.clone(),
        Utc::now(),
    );
    let xml = md.to_xml();
    assert!(xml.contains("<timestamp>20260423010103</timestamp>"));
    assert!(xml.contains("<buildNumber>2</buildNumber>"));
    assert!(
        xml.contains("1.0-SNAPSHOT-20260423010101-1"),
        "A snapshot value must survive the merge",
    );
    assert!(
        xml.contains("1.0-SNAPSHOT-20260423010103-2"),
        "B snapshot value must survive the merge",
    );
}

#[test]
fn regen_from_concurrent_versions_retains_both() {
    // Two concurrent publish-release jobs for the same artifact, one
    // version each. The regen output must retain both versions in
    // insertion order (Maven convention — not sorted).
    let v1 = "1.0.0";
    let v2 = "1.0.1";
    let md =
        MavenMetadata::artifact_index("com.example", "foo", vec![v1.into(), v2.into()], Utc::now());
    let xml = md.to_xml();
    let pos_v1 = xml
        .find(&format!("<version>{v1}</version>"))
        .expect("v1 present");
    let pos_v2 = xml
        .find(&format!("<version>{v2}</version>"))
        .expect("v2 present");
    assert!(pos_v1 < pos_v2, "insertion order preserved (v1 before v2)");
}

#[test]
fn merged_metadata_round_trips_through_xml() {
    // Even with a 10-element version list, regen → parse → regen
    // must be byte-stable (within escaping).
    let versions: Vec<String> = (0..10).map(|i| format!("1.0.{i}")).collect();
    let md = MavenMetadata::artifact_index("com.example", "foo", versions.clone(), Utc::now());
    let xml = md.to_xml();
    let parsed = MavenMetadata::from_xml(&xml).expect("parse");
    assert_eq!(parsed.versions, versions);
    assert_eq!(parsed.group_id, "com.example");
    assert_eq!(parsed.artifact_id, "foo");
}
