// SPDX-License-Identifier: Apache-2.0
//! End-to-end round-trip against [`InMemoryBlobStore`].
//!
//! Demonstrates the canonical content-addressed-storage flow that every
//! consumer of this crate (ferro-oci-server, ferro-maven-layout,
//! ferro-cargo-registry-server) performs against a [`BlobStore`]:
//!
//! 1. Compute a [`Digest`] from the bytes.
//! 2. `put` the bytes under the digest — implementations verify the
//!    digest matches what they re-hash from the supplied bytes.
//! 3. `contains` / `get` / `list` to read back.
//! 4. `delete` and confirm absence.
//!
//! Run with:
//!
//! ```bash
//! cargo run --example in_memory_round_trip -p ferro-blob-store
//! ```

use bytes::Bytes;
use ferro_blob_store::{BlobStore, Digest, InMemoryBlobStore};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let store = InMemoryBlobStore::new();

    let body = Bytes::from_static(b"the quick brown fox jumps over the lazy dog");
    let digest = Digest::sha256_of(&body);

    println!("computed digest: {digest}");
    println!("body length:     {} bytes", body.len());

    store.put(&digest, body.clone()).await?;
    assert!(store.contains(&digest).await?);
    println!("after put: store has {} entries", store.len());

    let read_back = store.get(&digest).await?;
    assert_eq!(read_back, body);
    println!("read-back matches input: {} bytes", read_back.len());

    let listed = store.list().await?;
    assert_eq!(listed.len(), 1);
    println!("list returned: {}", listed[0]);

    store.delete(&digest).await?;
    assert!(!store.contains(&digest).await?);
    println!("after delete: store is_empty = {}", store.is_empty());

    Ok(())
}
