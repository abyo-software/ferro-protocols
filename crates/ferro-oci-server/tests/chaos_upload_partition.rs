// SPDX-License-Identifier: Apache-2.0
//! Chaos: OCI chunked-upload network partition.
//!
//! Spec: OCI Distribution Spec v1.1 §4.3 — a chunked upload is a
//! sequence of PATCH requests. If a client disconnects mid-stream,
//! subsequent GETs on the upload session MUST return the correct
//! offset, and the client MUST be able to resume with the next
//! PATCH.
//!
//! Threat model: OCI-D-01 session exhaustion / NPM-D-01 style zip
//! bombs don't apply here; the failure mode is "client drops on
//! Wi-Fi" + "client re-establishes". We simulate that with direct
//! `RegistryMeta` calls so we exercise the contract without spinning
//! an HTTP client.

use bytes::Bytes;
use ferro_blob_store::Digest;
use ferro_oci_server::registry::{InMemoryRegistryMeta, RegistryMeta};

#[tokio::test]
async fn mid_upload_disconnect_then_resume_succeeds() {
    let reg = InMemoryRegistryMeta::new();
    let uuid = reg.start_upload("lib/alpine").await.expect("start");

    // Client writes 3 chunks then "disconnects" (we just stop sending).
    reg.append_upload("lib/alpine", &uuid, 0, Bytes::from_static(b"aaa"))
        .await
        .expect("chunk 0");
    reg.append_upload("lib/alpine", &uuid, 3, Bytes::from_static(b"bbb"))
        .await
        .expect("chunk 1");
    reg.append_upload("lib/alpine", &uuid, 6, Bytes::from_static(b"ccc"))
        .await
        .expect("chunk 2");

    // GET /v2/.../blobs/uploads/{uuid} simulation — the state must
    // reflect the 9-byte offset.
    let state = reg
        .get_upload_state("lib/alpine", &uuid)
        .await
        .expect("get state")
        .expect("state present");
    assert_eq!(state.offset(), 9, "offset must equal cumulative bytes");

    // Client reconnects and resumes from offset 9 with the next chunk.
    let new_off = reg
        .append_upload("lib/alpine", &uuid, 9, Bytes::from_static(b"ddd"))
        .await
        .expect("resume chunk");
    assert_eq!(new_off, 12, "resume offset must extend the old buffer");

    // Client finalizes with a PUT.
    let combined: &[u8] = b"aaabbbcccddd";
    let digest = Digest::sha256_of(combined);
    let taken = reg
        .take_upload_bytes("lib/alpine", &uuid)
        .await
        .expect("take")
        .expect("bytes present");
    assert_eq!(&taken[..], combined, "buffered bytes must match");
    reg.complete_upload("lib/alpine", &uuid, &digest)
        .await
        .expect("complete");
}

#[tokio::test]
async fn out_of_order_resume_is_rejected() {
    // Spec §4.3: chunked uploads must be sequential. If a client tries
    // to resume from the wrong offset after a partition, the registry
    // MUST reject the PATCH.
    let reg = InMemoryRegistryMeta::new();
    let uuid = reg.start_upload("lib/alpine").await.expect("start");
    reg.append_upload("lib/alpine", &uuid, 0, Bytes::from_static(b"first"))
        .await
        .expect("first chunk");

    // Attacker / broken client tries to resume at a fabricated offset.
    let result = reg
        .append_upload("lib/alpine", &uuid, 100, Bytes::from_static(b"malicious"))
        .await;
    assert!(result.is_err(), "out-of-order chunk must be rejected");

    // State is still recoverable for an honest resume.
    let state = reg
        .get_upload_state("lib/alpine", &uuid)
        .await
        .expect("get state")
        .expect("state present");
    assert_eq!(state.offset(), 5, "rejected chunk must not advance offset");
}

#[tokio::test]
async fn cancel_during_partition_is_idempotent() {
    // When the client chooses to abort rather than resume, DELETE on
    // the upload must succeed and the UUID must stop existing.
    let reg = InMemoryRegistryMeta::new();
    let uuid = reg.start_upload("lib/alpine").await.expect("start");
    reg.append_upload("lib/alpine", &uuid, 0, Bytes::from_static(b"stuff"))
        .await
        .expect("chunk");
    let cancelled = reg
        .cancel_upload("lib/alpine", &uuid)
        .await
        .expect("cancel");
    assert!(cancelled, "first cancel reports existence");
    let cancelled_again = reg
        .cancel_upload("lib/alpine", &uuid)
        .await
        .expect("cancel again");
    assert!(!cancelled_again, "second cancel reports missing");
    let state = reg
        .get_upload_state("lib/alpine", &uuid)
        .await
        .expect("get state");
    assert!(state.is_none(), "session is gone");
}
