// SPDX-License-Identifier: Apache-2.0
//! Conformance tests against vendored real Maven Central artefacts.
//!
//! These exercise the POM and `maven-metadata.xml` parsers against the
//! shapes that `mvn`, Gradle, and sbt see when they fetch
//! `org.apache.commons:commons-lang3:3.14.0` from Maven Central.
//!
//! Source URLs and license attribution: see `tests/fixtures/README.md`.

use ferro_maven_layout::{MavenMetadata, parse_pom};

const COMMONS_LANG3_POM: &str = include_str!("fixtures/commons-lang3-3.14.0.pom.xml");
const COMMONS_LANG3_METADATA: &str = include_str!("fixtures/commons-lang3-maven-metadata.xml");

#[test]
fn upstream_commons_lang3_pom_parses() {
    let pom = parse_pom(COMMONS_LANG3_POM).expect("commons-lang3 POM parses");
    assert_eq!(pom.group_id, "org.apache.commons");
    assert_eq!(pom.artifact_id, "commons-lang3");
    assert_eq!(pom.version, "3.14.0");
    assert_eq!(pom.packaging, "jar");
    assert_eq!(pom.model_version.as_deref(), Some("4.0.0"));
}

#[test]
fn upstream_commons_lang3_pom_has_parent_block() {
    let pom = parse_pom(COMMONS_LANG3_POM).expect("parse");
    let parent = pom
        .parent
        .expect("commons-lang3 declares an apache-commons-parent");
    assert_eq!(parent.group_id, "org.apache.commons");
    assert_eq!(parent.artifact_id, "commons-parent");
    assert_eq!(parent.version, "69");
}

#[test]
fn upstream_commons_lang3_metadata_parses() {
    let m = MavenMetadata::from_xml(COMMONS_LANG3_METADATA).expect("commons-lang3 metadata parses");
    assert_eq!(m.group_id, "org.apache.commons");
    assert_eq!(m.artifact_id, "commons-lang3");
    assert_eq!(m.release.as_deref(), Some("3.14.0"));
    assert_eq!(m.latest.as_deref(), Some("3.14.0"));
    // The vendored excerpt covers 3.0 through 3.14.0 — the head and
    // tail of the live release history.
    assert!(m.versions.contains(&"3.0".to_owned()));
    assert!(m.versions.contains(&"3.14.0".to_owned()));
    assert!(m.versions.len() >= 15);
    assert_eq!(m.last_updated.as_deref(), Some("20231221152312"));
}

#[test]
fn upstream_metadata_round_trips_via_to_xml() {
    // Round-trip: parse → serialise → re-parse. A repository proxy
    // that round-trips an artifact-index metadata file must preserve
    // groupId, artifactId, release, latest, versions list, and
    // lastUpdated.
    let parsed_in = MavenMetadata::from_xml(COMMONS_LANG3_METADATA).expect("parse 1");
    let xml_out = parsed_in.to_xml();
    let parsed_out = MavenMetadata::from_xml(&xml_out).expect("parse 2");

    assert_eq!(parsed_in.group_id, parsed_out.group_id);
    assert_eq!(parsed_in.artifact_id, parsed_out.artifact_id);
    assert_eq!(parsed_in.release, parsed_out.release);
    assert_eq!(parsed_in.latest, parsed_out.latest);
    assert_eq!(parsed_in.versions, parsed_out.versions);
    assert_eq!(parsed_in.last_updated, parsed_out.last_updated);
}
