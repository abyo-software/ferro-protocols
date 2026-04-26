// SPDX-License-Identifier: Apache-2.0
//! `ferro-maven-layout`
//!
//! Maven 2/3 repository layout, POM (`pom.xml`) parsing, `maven-metadata.xml`
//! generation, and SNAPSHOT artifact timestamping for FerroRepo.
//!
//! ## Spec references
//!
//! - Maven Repository Layout —
//!   <https://maven.apache.org/repository/layout.html>
//! - Remote / Local Repository —
//!   <https://maven.apache.org/ref/3.9.6/maven-repository-metadata/>
//! - Snapshot metadata format —
//!   <https://maven.apache.org/ref/3.9.6/maven-repository-metadata/repository-metadata.html>
//!
//! Phase 1 scope: full Maven Central wire compatibility covering
//! `mvn deploy`, `mvn dependency:go-offline`, Gradle 8.x, and sbt clients, with
//! `groupId:artifactId:version` GAV coordinates, `maven-metadata.xml` index
//! files, checksum sidecars (`.md5`, `.sha1`, `.sha256`, `.sha512`), and POM
//! path validation on `PUT`. GPG detached signatures (`.asc`) and Maven
//! Central publisher staging validation land in Phase 2.

#![deny(missing_docs)]

pub mod checksum;
pub mod coordinate;
pub mod error;
pub mod handlers;
pub mod layout;
pub mod metadata;
pub mod pom;
pub mod router;
pub mod snapshot;

pub use checksum::{ChecksumAlgo, compute_checksum, parse_sidecar};
pub use coordinate::{Coordinate, CoordinateParseError};
pub use error::MavenError;
pub use layout::{LayoutPath, PathClass, parse_layout_path};
pub use metadata::{MavenMetadata, Snapshot, SnapshotVersion};
pub use pom::{Pom, PomParent, parse_pom};
pub use router::{MavenState, router};
pub use snapshot::{SnapshotTimestamp, is_snapshot_version};

/// Crate name, exposed for diagnostics and `/metrics` labelling.
pub const CRATE_NAME: &str = "ferro-maven-layout";

#[cfg(test)]
mod tests {
    use super::CRATE_NAME;

    #[test]
    fn crate_name_is_stable() {
        assert_eq!(CRATE_NAME, "ferro-maven-layout");
    }
}
