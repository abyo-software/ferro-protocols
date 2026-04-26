// SPDX-License-Identifier: Apache-2.0
//! Manifest endpoints.
//!
//! Spec: OCI Distribution Spec v1.1 §3.2 "Pulling manifests", §4.4
//! "Pushing manifests", §4.9 "Deleting manifests".
//!
//! - `GET /v2/{name}/manifests/{reference}`  — fetch;
//! - `HEAD /v2/{name}/manifests/{reference}` — existence check;
//! - `PUT /v2/{name}/manifests/{reference}`  — push;
//! - `DELETE /v2/{name}/manifests/{reference}` — delete, BY DIGEST only.

use axum::body::{Body, Bytes};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use ferro_blob_store::Digest;
use serde_json::Value;

use crate::error::{OciError, OciErrorCode};
use crate::media_types::{ManifestKind, classify_manifest_media_type};
use crate::reference::{Reference, validate_name};
use crate::registry::ReferrerDescriptor;
use crate::router::AppState;

fn parse_reference(s: &str) -> Result<Reference, OciError> {
    s.parse::<Reference>()
}

fn manifest_response_headers(digest: &Digest, media_type: &str, size: usize) -> HeaderMap {
    let mut headers = HeaderMap::new();
    let digest_str = digest.to_string();
    if let Ok(v) = HeaderValue::from_str(&digest_str) {
        headers.insert("Docker-Content-Digest", v);
        if let Ok(etag) = HeaderValue::from_str(&format!("\"{digest_str}\"")) {
            headers.insert(header::ETAG, etag);
        }
    }
    if let Ok(v) = HeaderValue::from_str(media_type) {
        headers.insert(header::CONTENT_TYPE, v);
    }
    headers.insert(header::CONTENT_LENGTH, HeaderValue::from(size as u64));
    headers
}

/// Handle `GET /v2/{name}/manifests/{reference}`.
///
/// Spec: OCI Distribution Spec v1.1 §3.2.
///
/// **Range support (RFC 7233)**: when the request carries a
/// `Range: bytes=N-M` header that picks a valid sub-range of the
/// manifest body, the response is `206 Partial Content` with
/// `Content-Range: bytes N-M/total` and `Accept-Ranges: bytes`. A
/// syntactically valid range whose start is past the end of the body
/// produces `416 Range Not Satisfiable`. A missing or unsupported
/// `Range` header (multi-range, suffix-range, non-`bytes` unit) is
/// served as the full `200 OK` body — RFC 7233 §3.1 explicitly
/// permits ignoring ranges the server doesn't support.
pub async fn get_manifest(
    state: &AppState,
    name: &str,
    reference_str: &str,
    request_headers: &HeaderMap,
) -> Response {
    if let Err(e) = validate_name(name) {
        return e.into_response();
    }
    // A syntactically invalid reference on the GET/HEAD path is
    // treated as "manifest not found" — the OCI conformance suite
    // (`Pull GET nonexistent manifest should return 404`) requires
    // 404 here rather than the parser's 400 DigestInvalid /
    // ManifestInvalid. Push (PUT) and Delete keep stricter
    // 400-on-malformed-ref handling because there the client is
    // asserting an authoritative reference.
    let Ok(reference) = parse_reference(reference_str) else {
        return manifest_not_found(name, reference_str);
    };
    match state.registry.get_manifest(name, &reference).await {
        Ok(Some((digest, media_type, body))) => {
            range_or_full_response(request_headers, &digest, &media_type, &body)
        }
        Ok(None) => manifest_not_found(name, reference_str),
        Err(e) => OciError::from(e).into_response(),
    }
}

/// Build the response body honouring an optional `Range` header.
///
/// Returns `200 OK` for the full body when the header is absent or
/// unparseable (per RFC 7233 §3.1, an unrecognised range unit MAY be
/// treated as "no range"), `206 Partial Content` for a satisfiable
/// `bytes=N-M` (or `bytes=N-` open-ended) range, or `416 Range Not
/// Satisfiable` when the start is past the end of the body.
fn range_or_full_response(
    request_headers: &HeaderMap,
    digest: &Digest,
    media_type: &str,
    body: &Bytes,
) -> Response {
    let total = body.len();
    let raw_range = request_headers
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok());
    let parse_outcome = raw_range.map(parse_byte_range);
    match parse_outcome {
        // No `Range` header — full body (existing 200 path).
        None => {
            let mut headers = manifest_response_headers(digest, media_type, total);
            // RFC 7233 §2.3 allows the server to advertise range
            // support on every response. We emit it on the full-body
            // path so clients know they can issue ranged GETs next
            // time.
            headers.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
            (StatusCode::OK, headers, Body::from(body.clone())).into_response()
        }
        // Header present but un-parseable → fall back to full body
        // (RFC 7233 §3.1 permits ignoring unknown range units).
        Some(ByteRangeOutcome::Ignore) => {
            let mut headers = manifest_response_headers(digest, media_type, total);
            headers.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
            (StatusCode::OK, headers, Body::from(body.clone())).into_response()
        }
        // Header present and clearly malformed for the `bytes` unit
        // → 416 per RFC 7233 §4.4.
        Some(ByteRangeOutcome::Unsatisfiable) => unsatisfiable_response(total),
        Some(ByteRangeOutcome::Range { start, end }) => {
            // Clamp end to the last byte index. RFC 7233 §2.1: an
            // end past the body is allowed and means "to the end".
            let last = total.saturating_sub(1);
            if total == 0 || start > last {
                return unsatisfiable_response(total);
            }
            let clamped_end = end.min(last);
            let slice_len = clamped_end - start + 1;
            let slice = body.slice(start..=clamped_end);

            let mut headers = HeaderMap::new();
            let digest_str = digest.to_string();
            if let Ok(v) = HeaderValue::from_str(&digest_str) {
                headers.insert("Docker-Content-Digest", v);
                if let Ok(etag) = HeaderValue::from_str(&format!("\"{digest_str}\"")) {
                    headers.insert(header::ETAG, etag);
                }
            }
            if let Ok(v) = HeaderValue::from_str(media_type) {
                headers.insert(header::CONTENT_TYPE, v);
            }
            headers.insert(header::CONTENT_LENGTH, HeaderValue::from(slice_len as u64));
            headers.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
            if let Ok(v) = HeaderValue::from_str(&format!("bytes {start}-{clamped_end}/{total}")) {
                headers.insert(header::CONTENT_RANGE, v);
            }
            (StatusCode::PARTIAL_CONTENT, headers, Body::from(slice)).into_response()
        }
    }
}

/// Build the canonical `416 Range Not Satisfiable` response.
fn unsatisfiable_response(total: usize) -> Response {
    let mut headers = HeaderMap::new();
    if let Ok(v) = HeaderValue::from_str(&format!("bytes */{total}")) {
        headers.insert(header::CONTENT_RANGE, v);
    }
    headers.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    (StatusCode::RANGE_NOT_SATISFIABLE, headers).into_response()
}

/// Outcome of parsing a `Range` header.
#[derive(Debug, PartialEq, Eq)]
enum ByteRangeOutcome {
    /// A satisfiable `bytes=N-M` (or `bytes=N-`) range.
    ///
    /// `end` is the end of the requested range, clamped or open;
    /// callers further clamp to the body length.
    Range { start: usize, end: usize },
    /// A syntactically valid `bytes=...` range that cannot be
    /// satisfied (e.g. start > end). Caller emits 416.
    Unsatisfiable,
    /// Header used a unit other than `bytes`, was multi-range, or
    /// was otherwise unparseable. RFC 7233 §3.1 permits ignoring it.
    Ignore,
}

/// Parse a `Range: bytes=...` header value.
///
/// Supported shapes:
///
/// - `bytes=N-M` — explicit range
/// - `bytes=N-`  — open-ended ("from N to end")
///
/// Returns `Ignore` for suffix ranges (`bytes=-N`), multi-range
/// (`bytes=0-99,200-299`), and any non-`bytes` unit. We deliberately
/// keep the parser narrow so future support for additional shapes is
/// an additive change.
fn parse_byte_range(raw: &str) -> ByteRangeOutcome {
    let Some(spec) = raw.strip_prefix("bytes=") else {
        return ByteRangeOutcome::Ignore;
    };
    // Multi-range — surface "Ignore" so we serve the full body.
    if spec.contains(',') {
        return ByteRangeOutcome::Ignore;
    }
    let Some((lhs, rhs)) = spec.split_once('-') else {
        return ByteRangeOutcome::Ignore;
    };
    if lhs.is_empty() {
        // Suffix-range (`bytes=-N`) is technically a separate
        // shape; we keep things simple and ignore it for now.
        return ByteRangeOutcome::Ignore;
    }
    let Ok(start) = lhs.parse::<usize>() else {
        return ByteRangeOutcome::Ignore;
    };
    let end = if rhs.is_empty() {
        usize::MAX
    } else {
        match rhs.parse::<usize>() {
            Ok(v) => v,
            Err(_) => return ByteRangeOutcome::Ignore,
        }
    };
    if start > end {
        return ByteRangeOutcome::Unsatisfiable;
    }
    ByteRangeOutcome::Range { start, end }
}

/// Handle `HEAD /v2/{name}/manifests/{reference}`.
///
/// Spec: OCI Distribution Spec v1.1 §3.2.
pub async fn head_manifest(state: &AppState, name: &str, reference_str: &str) -> Response {
    if let Err(e) = validate_name(name) {
        return e.into_response();
    }
    // See `get_manifest` — invalid reference on the HEAD path is a
    // 404 (manifest not found), not a 400. This is the
    // `Pull HEAD nonexistent manifest should return 404` conformance
    // testcase contract.
    let Ok(reference) = parse_reference(reference_str) else {
        return manifest_not_found(name, reference_str);
    };
    match state.registry.get_manifest(name, &reference).await {
        Ok(Some((digest, media_type, body))) => {
            let headers = manifest_response_headers(&digest, &media_type, body.len());
            (StatusCode::OK, headers).into_response()
        }
        Ok(None) => manifest_not_found(name, reference_str),
        Err(e) => OciError::from(e).into_response(),
    }
}

/// Build the canonical 404 `MANIFEST_UNKNOWN` response body shared by
/// the GET / HEAD handlers above. Centralised so both call sites
/// (real lookup miss + parse-failure miss) emit the exact same JSON
/// envelope and Docker error code, which `containerd` / `crane` /
/// `nerdctl` clients all match on.
fn manifest_not_found(name: &str, reference_str: &str) -> Response {
    OciError::new(
        OciErrorCode::ManifestUnknown,
        format!("manifest {reference_str} not found in {name}"),
    )
    .into_response()
}

/// Handle `PUT /v2/{name}/manifests/{reference}`.
///
/// Spec: OCI Distribution Spec v1.1 §4.4 "Pushing manifests".
///
/// Validates the declared `Content-Type`, requires every referenced
/// blob to be present in the blob store, records the manifest, and
/// returns the canonical digest in both `Location` and
/// `Docker-Content-Digest`.
pub async fn put_manifest(
    state: &AppState,
    name: &str,
    reference_str: &str,
    headers: &HeaderMap,
    body: Bytes,
) -> Response {
    if let Err(e) = validate_name(name) {
        return e.into_response();
    }
    let reference = match parse_reference(reference_str) {
        Ok(r) => r,
        Err(e) => return e.into_response(),
    };

    let content_type = match headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
    {
        Some(s) => s.to_owned(),
        None => {
            return OciError::new(OciErrorCode::ManifestInvalid, "missing Content-Type")
                .into_response();
        }
    };
    let Some(kind) = classify_manifest_media_type(&content_type) else {
        return OciError::new(
            OciErrorCode::ManifestInvalid,
            format!("unsupported manifest media type `{content_type}`"),
        )
        .into_response();
    };

    // Parse the manifest as JSON. We validate the structure but also
    // keep the raw bytes because the canonical digest is over the
    // exact bytes the client sent.
    let parsed: Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => {
            return OciError::new(
                OciErrorCode::ManifestInvalid,
                format!("manifest is not valid JSON: {e}"),
            )
            .into_response();
        }
    };

    // Verify referenced blobs exist. For image manifests, check `config`
    // and every `layers[]` entry. For image indexes, check every
    // `manifests[]` entry's digest is present as a manifest body
    // (already registered) OR the digest exists as a blob.
    if let Err(e) = verify_referenced_blobs(state, &parsed, kind).await {
        return e.into_response();
    }

    let digest = Digest::sha256_of(&body);

    // Register any referrer (subject field).
    let subject_digest = parsed
        .get("subject")
        .and_then(|s| s.get("digest"))
        .and_then(Value::as_str)
        .and_then(|s| s.parse::<Digest>().ok());
    let artifact_type = parsed
        .get("artifactType")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let annotations = parsed
        .get("annotations")
        .and_then(Value::as_object)
        .map(|m| {
            m.iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_owned())))
                .collect::<std::collections::BTreeMap<_, _>>()
        });

    if let Err(e) = state
        .registry
        .put_manifest(name, &reference, &digest, &content_type, body.clone())
        .await
    {
        return OciError::from(e).into_response();
    }

    if let Some(subj) = subject_digest {
        let descriptor = ReferrerDescriptor {
            media_type: content_type.clone(),
            digest: digest.clone(),
            size: body.len() as u64,
            artifact_type,
            annotations,
        };
        if let Err(e) = state
            .registry
            .register_referrer(name, &subj, descriptor)
            .await
        {
            return OciError::from(e).into_response();
        }
    }

    let mut out = HeaderMap::new();
    let location = format!("/v2/{name}/manifests/{digest}");
    if let Ok(v) = HeaderValue::from_str(&location) {
        out.insert(header::LOCATION, v);
    }
    if let Ok(v) = HeaderValue::from_str(&digest.to_string()) {
        out.insert("Docker-Content-Digest", v);
    }
    // Per §3.3 the server MUST surface the subject header on manifests
    // that have one so clients can discover the referrers list.
    if let Some(subj) = parsed
        .get("subject")
        .and_then(|s| s.get("digest"))
        .and_then(Value::as_str)
        && let Ok(v) = HeaderValue::from_str(subj)
    {
        out.insert("OCI-Subject", v);
    }
    out.insert(header::CONTENT_LENGTH, HeaderValue::from(0u64));
    (StatusCode::CREATED, out).into_response()
}

/// Handle `DELETE /v2/{name}/manifests/{reference}`.
///
/// Spec: OCI Distribution Spec v1.1 §4.9 — a DELETE by tag is NOT
/// allowed; the server MUST respond `405 Method Not Allowed` in that
/// case.
pub async fn delete_manifest(state: &AppState, name: &str, reference_str: &str) -> Response {
    if let Err(e) = validate_name(name) {
        return e.into_response();
    }
    let reference = match parse_reference(reference_str) {
        Ok(r) => r,
        Err(e) => return e.into_response(),
    };
    if reference.is_tag() {
        return OciError::new(
            OciErrorCode::Unsupported,
            "DELETE manifest by tag is not supported; use digest",
        )
        .with_status(StatusCode::METHOD_NOT_ALLOWED)
        .into_response();
    }
    match state.registry.delete_manifest(name, &reference).await {
        Ok(true) => (StatusCode::ACCEPTED, HeaderMap::new()).into_response(),
        Ok(false) => OciError::new(
            OciErrorCode::ManifestUnknown,
            format!("manifest {reference_str} not found in {name}"),
        )
        .into_response(),
        Err(e) => OciError::from(e).into_response(),
    }
}

async fn verify_referenced_blobs(
    state: &AppState,
    parsed: &Value,
    kind: ManifestKind,
) -> Result<(), OciError> {
    match kind {
        ManifestKind::ImageManifest | ManifestKind::Artifact => {
            if let Some(config) = parsed.get("config").and_then(Value::as_object)
                && let Some(d) = config.get("digest").and_then(Value::as_str)
            {
                check_blob_present(state, d).await?;
            }
            if let Some(layers) = parsed.get("layers").and_then(Value::as_array) {
                for layer in layers {
                    if let Some(d) = layer.get("digest").and_then(Value::as_str) {
                        check_blob_present(state, d).await?;
                    }
                }
            }
        }
        ManifestKind::ImageIndex => {
            // Per spec §4.4, each manifest in the index must already
            // be registered as a manifest body. We check the blob
            // store first (covers the case where the index manifests
            // were uploaded as raw blobs) and fall through silently
            // if the registry records them — either is spec-valid.
            if let Some(manifests) = parsed.get("manifests").and_then(Value::as_array) {
                for manifest in manifests {
                    if let Some(d) = manifest.get("digest").and_then(Value::as_str) {
                        // For indexes we accept either a registered
                        // manifest OR a stored blob. Blob check first.
                        let digest = d.parse::<Digest>().map_err(|e| {
                            OciError::new(
                                OciErrorCode::ManifestInvalid,
                                format!("invalid digest in manifests[]: {e}"),
                            )
                        })?;
                        let present = state
                            .blob_store
                            .contains(&digest)
                            .await
                            .map_err(OciError::from)?;
                        if !present {
                            // Also accept if the digest resolves as a
                            // manifest in any known name — we do a
                            // lightweight check by looking up the
                            // digest as a reference in the same name
                            // is left for Phase 2; reject for now.
                            return Err(OciError::new(
                                OciErrorCode::ManifestBlobUnknown,
                                format!("referenced manifest digest {d} not present"),
                            ));
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

/// SHA-256 digest of the OCI empty descriptor payload (`{}`, 2 bytes).
///
/// OCI Image Spec v1.1 §3 designates this digest as a well-known,
/// always-supported payload. Registries accept manifests that
/// reference it via `config` or `layers[]` without requiring an
/// explicit blob upload — the conformance suite relies on this when
/// pushing referrer manifests for the Content Discovery workflow.
const OCI_EMPTY_DESCRIPTOR_DIGEST: &str =
    "sha256:44136fa355b3678a1146ad16f7e8649e94fb4fc21fe77e8310c060f61caaff8a";

async fn check_blob_present(state: &AppState, digest_str: &str) -> Result<(), OciError> {
    // OCI Image Spec v1.1 §3 — empty descriptor is always-present
    // by spec; do not require operators to upload it explicitly.
    if digest_str == OCI_EMPTY_DESCRIPTOR_DIGEST {
        return Ok(());
    }
    let digest = digest_str.parse::<Digest>().map_err(|e| {
        OciError::new(
            OciErrorCode::ManifestInvalid,
            format!("invalid digest `{digest_str}`: {e}"),
        )
    })?;
    let present = state
        .blob_store
        .contains(&digest)
        .await
        .map_err(OciError::from)?;
    if !present {
        return Err(OciError::new(
            OciErrorCode::ManifestBlobUnknown,
            format!("referenced blob {digest_str} not present"),
        ));
    }
    Ok(())
}
