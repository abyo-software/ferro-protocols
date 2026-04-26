// SPDX-License-Identifier: Apache-2.0
//! Axum handlers for Maven 2/3 artifact GET / HEAD / PUT / DELETE.
//!
//! Conceptually each request is:
//!
//! 1. Parse the wildcard URL path into a [`LayoutPath`].
//! 2. Handle checksum sidecars, metadata documents, or the main artifact.
//! 3. Update the in-memory layout and metadata indices on mutations.
//!
//! Spec references are cited on individual branches. The implementation
//! follows the Maven repository layout closely so clients that target
//! Nexus / Artifactory require no configuration changes.

use axum::extract::{Path as AxumPath, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use bytes::Bytes;
use chrono::Utc;
use ferro_blob_store::Digest;
use tracing::debug;

use crate::checksum::{ChecksumAlgo, compute_checksum, parse_sidecar};
use crate::coordinate::Coordinate;
use crate::error::MavenError;
use crate::layout::{LayoutPath, PathClass, parse_layout_path};
use crate::metadata::{MavenMetadata, Snapshot, SnapshotVersion};
use crate::pom::parse_pom;
use crate::router::MavenState;
use crate::snapshot::{SnapshotTimestamp, base_version, is_snapshot_version};

fn build_key(repo: &str, path: &str) -> String {
    format!("{repo}/{path}")
}

fn snapshot_metadata_path(
    repo: &str,
    group_path: &str,
    artifact_id: &str,
    base_version: &str,
) -> String {
    format!("{repo}/{group_path}/{artifact_id}/{base_version}-SNAPSHOT/maven-metadata.xml")
}

/// GET a Maven resource.
///
/// Streams the blob identified by the URL path, computing any missing
/// checksum sidecar on the fly from the underlying artifact's bytes.
pub async fn handle_get(
    State(state): State<MavenState>,
    AxumPath((repo, path)): AxumPath<(String, String)>,
) -> Result<Response, MavenError> {
    serve(state, repo, path, true).await
}

/// HEAD a Maven resource.
///
/// Returns the same headers as `GET` but with an empty body so tooling
/// (`mvn dependency:get`, Gradle) can cheaply probe existence.
pub async fn handle_head(
    State(state): State<MavenState>,
    AxumPath((repo, path)): AxumPath<(String, String)>,
) -> Result<Response, MavenError> {
    serve(state, repo, path, false).await
}

async fn serve(
    state: MavenState,
    repo: String,
    path: String,
    with_body: bool,
) -> Result<Response, MavenError> {
    let layout = parse_layout_path(&path)?;
    let key = build_key(&repo, &path);

    match &layout.class {
        PathClass::Artifact => {
            let digest_opt = state.layout.read().await.get(&key).cloned();
            let Some(digest) = digest_opt else {
                return Err(MavenError::NotFound(path));
            };
            let bytes = state.blobs.get(&digest).await?;
            Ok(build_artifact_response(
                &layout.coordinate,
                &digest,
                bytes,
                with_body,
            ))
        }
        PathClass::Checksum(algo) => serve_checksum(&state, &repo, &path, *algo, with_body).await,
        PathClass::Metadata {
            version_level,
            checksum,
        } => {
            let meta_bytes = load_metadata_xml(&state, &repo, &layout, *version_level).await?;
            if let Some(algo) = checksum {
                let hex = compute_checksum(*algo, &meta_bytes).ok_or_else(|| {
                    MavenError::ChecksumMismatch(format!(
                        "cannot compute {algo:?} for maven-metadata.xml"
                    ))
                })?;
                Ok(build_sidecar_response(&hex, with_body))
            } else {
                Ok(build_raw_response(meta_bytes, with_body))
            }
        }
    }
}

async fn serve_checksum(
    state: &MavenState,
    repo: &str,
    path: &str,
    algo: ChecksumAlgo,
    with_body: bool,
) -> Result<Response, MavenError> {
    // Try the sidecar path literally first. If not found, recompute
    // from the underlying artifact on the fly. Spec: Maven clients
    // treat a missing `.sha1` sidecar as fatal, so synthesising it is
    // strictly a compatibility courtesy.
    let key = build_key(repo, path);
    if let Some(d) = state.layout.read().await.get(&key).cloned() {
        let bytes = state.blobs.get(&d).await?;
        return Ok(build_raw_response(bytes, with_body));
    }

    // Strip the trailing `.<algo>` extension and look up the main file.
    let main_path =
        path.strip_suffix(&format!(".{}", algo.extension()))
            .ok_or(MavenError::NotFound(format!(
                "sidecar path {path} has no algo suffix"
            )))?;
    let main_key = build_key(repo, main_path);
    let Some(digest) = state.layout.read().await.get(&main_key).cloned() else {
        return Err(MavenError::NotFound(path.to_string()));
    };
    let bytes = state.blobs.get(&digest).await?;
    let hex = compute_checksum(algo, &bytes).ok_or_else(|| {
        MavenError::ChecksumMismatch(format!("cannot compute {algo:?} on the fly"))
    })?;
    Ok(build_sidecar_response(&hex, with_body))
}

async fn load_metadata_xml(
    state: &MavenState,
    repo: &str,
    layout: &LayoutPath,
    version_level: bool,
) -> Result<Bytes, MavenError> {
    let group_path = layout.coordinate.group_path();
    let artifact_id = layout.coordinate.artifact_id.clone();
    let base = if version_level {
        Some(base_version(&layout.coordinate.version).to_string())
    } else {
        None
    };
    let cached = state
        .metadata
        .read()
        .await
        .get(&(
            repo.to_string(),
            group_path.clone(),
            artifact_id.clone(),
            base,
        ))
        .cloned();
    let md = cached.ok_or_else(|| {
        MavenError::NotFound(format!(
            "no maven-metadata.xml for {group_path}/{artifact_id}"
        ))
    })?;
    Ok(Bytes::from(md.to_xml()))
}

fn build_artifact_response(
    coordinate: &Coordinate,
    digest: &Digest,
    bytes: Bytes,
    with_body: bool,
) -> Response {
    let mut headers = HeaderMap::new();
    let ct = match coordinate.extension.as_str() {
        "pom" | "xml" => "application/xml",
        "jar" | "war" | "ear" => "application/java-archive",
        "tar.gz" | "tgz" => "application/gzip",
        "zip" => "application/zip",
        _ => "application/octet-stream",
    };
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(ct));
    let len_val = bytes.len().to_string();
    if let Ok(v) = HeaderValue::from_str(&len_val) {
        headers.insert(header::CONTENT_LENGTH, v);
    }
    if let Ok(v) = HeaderValue::from_str(&format!("\"sha256:{}\"", digest.hex())) {
        headers.insert(header::ETAG, v);
    }
    if let Ok(v) = HeaderValue::from_str(digest.hex()) {
        headers.insert("X-Checksum-Sha256", v);
    }
    if let Some(sha1) = compute_checksum(ChecksumAlgo::Sha1, &bytes)
        && let Ok(v) = HeaderValue::from_str(&sha1)
    {
        headers.insert("X-Checksum-Sha1", v);
    }

    let body = if with_body { bytes } else { Bytes::new() };
    (StatusCode::OK, headers, body).into_response()
}

fn build_sidecar_response(hex: &str, with_body: bool) -> Response {
    let body = if with_body {
        Bytes::copy_from_slice(hex.as_bytes())
    } else {
        Bytes::new()
    };
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    (StatusCode::OK, headers, body).into_response()
}

fn build_raw_response(bytes: Bytes, with_body: bool) -> Response {
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/xml"),
    );
    if let Ok(v) = HeaderValue::from_str(&bytes.len().to_string()) {
        headers.insert(header::CONTENT_LENGTH, v);
    }
    let body = if with_body { bytes } else { Bytes::new() };
    (StatusCode::OK, headers, body).into_response()
}

/// PUT a Maven artifact, checksum sidecar, or metadata document.
pub async fn handle_put(
    State(state): State<MavenState>,
    AxumPath((repo, path)): AxumPath<(String, String)>,
    body: Bytes,
) -> Result<Response, MavenError> {
    let layout = parse_layout_path(&path)?;
    match layout.class.clone() {
        PathClass::Artifact => put_artifact(&state, &repo, &path, &layout, body).await,
        PathClass::Checksum(algo) => put_checksum(&state, &repo, &path, algo, &body).await,
        PathClass::Metadata { .. } => {
            // Metadata uploads from clients are accepted; we parse to
            // validate and cache the parsed form.
            let xml = std::str::from_utf8(&body)
                .map_err(|_| MavenError::InvalidMetadata("metadata body is not UTF-8".into()))?;
            let meta = MavenMetadata::from_xml(xml)?;
            let group_path = layout.coordinate.group_path();
            let base = if let PathClass::Metadata {
                version_level: true,
                ..
            } = layout.class
            {
                Some(base_version(&layout.coordinate.version).to_string())
            } else {
                None
            };
            let key = (
                repo.clone(),
                group_path,
                layout.coordinate.artifact_id.clone(),
                base,
            );
            state.metadata.write().await.insert(key, meta);
            Ok((StatusCode::CREATED, "stored").into_response())
        }
    }
}

async fn put_artifact(
    state: &MavenState,
    repo: &str,
    path: &str,
    layout: &LayoutPath,
    body: Bytes,
) -> Result<Response, MavenError> {
    // Validate POM coordinate against the URL when the extension is pom.
    if layout.coordinate.extension == "pom" {
        let text = std::str::from_utf8(&body)
            .map_err(|_| MavenError::InvalidPom("POM body is not UTF-8".into()))?;
        let pom = parse_pom(text)?;
        if pom.group_id != layout.coordinate.group_id
            || pom.artifact_id != layout.coordinate.artifact_id
            || pom.version != layout.coordinate.version
        {
            return Err(MavenError::CoordinateMismatch(format!(
                "POM says {}:{}:{}, URL says {}:{}:{}",
                pom.group_id,
                pom.artifact_id,
                pom.version,
                layout.coordinate.group_id,
                layout.coordinate.artifact_id,
                layout.coordinate.version,
            )));
        }
    }

    let digest = Digest::sha256_of(&body);
    state.blobs.put(&digest, body.clone()).await?;

    if is_snapshot_version(&layout.coordinate.version) {
        register_snapshot_timestamped(state, repo, layout, &digest).await?;
    }

    let key_path = build_key(repo, path);

    state.layout.write().await.insert(key_path, digest.clone());

    // Regenerate artifact-index metadata from the set of known
    // versions for this (repo, groupPath, artifactId).
    regenerate_artifact_index(state, repo, layout).await;

    let mut headers = HeaderMap::new();
    if let Ok(v) = HeaderValue::from_str(digest.hex()) {
        headers.insert("X-Checksum-Sha256", v);
    }
    Ok((StatusCode::CREATED, headers, "stored").into_response())
}

async fn register_snapshot_timestamped(
    state: &MavenState,
    repo: &str,
    layout: &LayoutPath,
    digest: &Digest,
) -> Result<(), MavenError> {
    let base = base_version(&layout.coordinate.version).to_string();
    let counter_key = (
        repo.to_string(),
        layout.coordinate.group_path(),
        layout.coordinate.artifact_id.clone(),
        base.clone(),
    );
    let build_number = {
        let mut w = state.snapshot_counter.write().await;
        let next = w.get(&counter_key).copied().unwrap_or(0) + 1;
        w.insert(counter_key, next);
        next
    };
    let ts = SnapshotTimestamp::now();
    let timestamped_version = ts.compose_version(&layout.coordinate.version, build_number);
    let timestamped_coord = Coordinate::new(
        layout.coordinate.group_id.clone(),
        layout.coordinate.artifact_id.clone(),
        timestamped_version.clone(),
        layout.coordinate.classifier.clone(),
        layout.coordinate.extension.clone(),
    )
    .map_err(|e| MavenError::InvalidPath(e.to_string()))?;
    let ts_path = format!(
        "{}/{}/{}/{}",
        layout.coordinate.group_path(),
        layout.coordinate.artifact_id,
        layout.coordinate.version,
        timestamped_coord.filename()
    );
    state
        .layout
        .write()
        .await
        .insert(build_key(repo, &ts_path), digest.clone());

    let sv = SnapshotVersion {
        classifier: layout.coordinate.classifier.clone(),
        extension: layout.coordinate.extension.clone(),
        value: timestamped_version,
        updated: Utc::now().format("%Y%m%d%H%M%S").to_string(),
    };
    let snap = Snapshot {
        timestamp: ts.format(),
        build_number,
    };
    let md = MavenMetadata::snapshot_metadata(
        layout.coordinate.group_id.clone(),
        layout.coordinate.artifact_id.clone(),
        layout.coordinate.version.clone(),
        snap,
        vec![sv],
        Utc::now(),
    );
    let md_path = snapshot_metadata_path(
        repo,
        &layout.coordinate.group_path(),
        &layout.coordinate.artifact_id,
        &base,
    );
    state.metadata.write().await.insert(
        (
            repo.to_string(),
            layout.coordinate.group_path(),
            layout.coordinate.artifact_id.clone(),
            Some(base),
        ),
        md,
    );
    debug!(%md_path, "snapshot metadata cached");
    Ok(())
}

async fn put_checksum(
    state: &MavenState,
    repo: &str,
    path: &str,
    algo: ChecksumAlgo,
    body: &Bytes,
) -> Result<Response, MavenError> {
    let declared = parse_sidecar(algo, body)?;

    // If the main artifact is already present, verify the sidecar
    // against its recomputed checksum. Otherwise accept the sidecar as
    // a staged write (it will be validated on the subsequent artifact
    // PUT if possible).
    let main_path = path
        .strip_suffix(&format!(".{}", algo.extension()))
        .ok_or_else(|| MavenError::InvalidPath("sidecar without algo suffix".into()))?;
    let main_key = build_key(repo, main_path);
    if let Some(d) = state.layout.read().await.get(&main_key).cloned() {
        let bytes = state.blobs.get(&d).await?;
        let actual = compute_checksum(algo, &bytes)
            .ok_or_else(|| MavenError::ChecksumMismatch(format!("cannot compute {algo:?}")))?;
        if actual != declared {
            return Err(MavenError::ChecksumMismatch(format!(
                "declared {declared}, actual {actual}"
            )));
        }
    }

    // Store the raw sidecar body so a subsequent GET returns the exact
    // bytes the client uploaded (matching `sha1sum` formatting).
    let digest = Digest::sha256_of(body);
    state.blobs.put(&digest, body.clone()).await?;
    state
        .layout
        .write()
        .await
        .insert(build_key(repo, path), digest);

    Ok((StatusCode::CREATED, "stored").into_response())
}

async fn regenerate_artifact_index(state: &MavenState, repo: &str, layout: &LayoutPath) {
    let group_path = layout.coordinate.group_path();
    let artifact_id = layout.coordinate.artifact_id.clone();
    let prefix = format!("{repo}/{group_path}/{artifact_id}/");

    let mut versions: Vec<String> = state
        .layout
        .read()
        .await
        .keys()
        .filter_map(|k| {
            let tail = k.strip_prefix(&prefix)?;
            let (ver, _) = tail.split_once('/')?;
            Some(ver.to_string())
        })
        .collect();
    versions.sort();
    versions.dedup();

    let md = MavenMetadata::artifact_index(
        &layout.coordinate.group_id,
        &artifact_id,
        versions,
        Utc::now(),
    );
    state
        .metadata
        .write()
        .await
        .insert((repo.to_string(), group_path, artifact_id, None), md);
}

/// DELETE a Maven resource.
///
/// Removes the layout entry and, if no other path references the
/// underlying blob, the blob itself.
pub async fn handle_delete(
    State(state): State<MavenState>,
    AxumPath((repo, path)): AxumPath<(String, String)>,
) -> Result<Response, MavenError> {
    let key = build_key(&repo, &path);
    let removed = state.layout.write().await.remove(&key);
    if let Some(digest) = removed {
        // Keep blob if referenced elsewhere; otherwise delete.
        let still_referenced = state.layout.read().await.values().any(|d| d == &digest);
        if !still_referenced {
            state.blobs.delete(&digest).await?;
        }
        Ok((StatusCode::NO_CONTENT, "").into_response())
    } else {
        Err(MavenError::NotFound(path))
    }
}
