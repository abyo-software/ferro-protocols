// SPDX-License-Identifier: Apache-2.0
//! Filesystem-backed [`BlobStore`].

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;
use bytes::Bytes;

use crate::{BlobStore, BlobStoreError, Digest, DigestAlgo, Result};

static TMP_SEQ: AtomicU64 = AtomicU64::new(0);

/// Local-filesystem implementation of [`BlobStore`].
///
/// Files live at `<root>/<algo>/<2-char-prefix>/<rest-of-hex>`. The
/// 2-char prefix shards the directory so popular algorithms do not
/// produce a single directory with millions of entries.
///
/// Writes are atomic: bytes are written to a temporary file inside
/// `<root>/<algo>/.tmp/` and `tokio::fs::rename`'d into place. A
/// concurrent `get` either sees the previous version or the new one,
/// never a partial write.
#[derive(Debug, Clone)]
pub struct FsBlobStore {
    root: PathBuf,
}

impl FsBlobStore {
    /// Construct a store rooted at `root`. Creates the directory tree
    /// if it does not yet exist.
    pub fn new(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        std::fs::create_dir_all(&root)?;
        Ok(Self { root })
    }

    fn algo_dir(&self, algo: DigestAlgo) -> PathBuf {
        self.root.join(algo.prefix())
    }

    fn tmp_dir(&self, algo: DigestAlgo) -> PathBuf {
        self.algo_dir(algo).join(".tmp")
    }

    fn path_for(&self, digest: &Digest) -> PathBuf {
        let hex = digest.hex();
        // Sufficient hex length is enforced by Digest construction (>= 64).
        let (prefix, rest) = hex.split_at(2);
        self.algo_dir(digest.algo()).join(prefix).join(rest)
    }
}

#[async_trait]
impl BlobStore for FsBlobStore {
    async fn put(&self, digest: &Digest, bytes: Bytes) -> Result<()> {
        let computed = match digest.algo() {
            DigestAlgo::Sha256 => Digest::sha256_of(&bytes),
            DigestAlgo::Sha512 => Digest::sha512_of(&bytes),
        };
        if &computed != digest {
            return Err(BlobStoreError::DigestMismatch {
                expected: digest.to_string(),
                computed: computed.to_string(),
            });
        }

        let final_path = self.path_for(digest);
        if let Some(parent) = final_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let tmp_dir = self.tmp_dir(digest.algo());
        tokio::fs::create_dir_all(&tmp_dir).await?;
        // Per-call unique temp file name so concurrent puts of the same
        // digest never race on rename. PID + per-process atomic counter
        // is enough; the temp file is renamed away within milliseconds.
        let n = TMP_SEQ.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        let tmp_path = tmp_dir.join(format!("{}.{}.{}.tmp", digest.hex(), pid, n));
        tokio::fs::write(&tmp_path, &bytes).await?;
        match tokio::fs::rename(&tmp_path, &final_path).await {
            Ok(()) => Ok(()),
            Err(e) => {
                // If the destination already exists with the same content
                // (concurrent put of the same digest, the other writer won
                // the race), treat that as success and clean up our temp.
                let _ = tokio::fs::remove_file(&tmp_path).await;
                if final_path.is_file() {
                    Ok(())
                } else {
                    Err(e.into())
                }
            }
        }
    }

    async fn get(&self, digest: &Digest) -> Result<Bytes> {
        let path = self.path_for(digest);
        match tokio::fs::read(&path).await {
            Ok(bytes) => Ok(Bytes::from(bytes)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                Err(BlobStoreError::NotFound(digest.to_string()))
            }
            Err(e) => Err(e.into()),
        }
    }

    async fn contains(&self, digest: &Digest) -> Result<bool> {
        let path = self.path_for(digest);
        match tokio::fs::metadata(&path).await {
            Ok(_) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(e.into()),
        }
    }

    async fn delete(&self, digest: &Digest) -> Result<()> {
        let path = self.path_for(digest);
        match tokio::fs::remove_file(&path).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    async fn list(&self) -> Result<Vec<Digest>> {
        let mut out = Vec::new();
        for algo in [DigestAlgo::Sha256, DigestAlgo::Sha512] {
            let algo_dir = self.algo_dir(algo);
            if !algo_dir.exists() {
                continue;
            }
            collect_algo(&algo_dir, algo, &mut out).await?;
        }
        Ok(out)
    }
}

async fn collect_algo(algo_dir: &Path, algo: DigestAlgo, out: &mut Vec<Digest>) -> Result<()> {
    let mut entries = tokio::fs::read_dir(algo_dir).await?;
    while let Some(prefix_entry) = entries.next_entry().await? {
        let prefix_path = prefix_entry.path();
        let Some(prefix_name) = prefix_path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        // Skip the temp directory and any non-2-char prefixes.
        if prefix_name.starts_with('.') || prefix_name.len() != 2 {
            continue;
        }
        if !prefix_path.is_dir() {
            continue;
        }
        let mut hex_files = tokio::fs::read_dir(&prefix_path).await?;
        while let Some(file_entry) = hex_files.next_entry().await? {
            let file_path = file_entry.path();
            let Some(file_name) = file_path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            let full_hex = format!("{prefix_name}{file_name}");
            if let Ok(digest) = Digest::new(algo, full_hex) {
                out.push(digest);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn fs_put_get_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let store = FsBlobStore::new(dir.path()).unwrap();
        let body = Bytes::from_static(b"persistent-bytes");
        let d = Digest::sha256_of(&body);
        store.put(&d, body.clone()).await.unwrap();
        assert!(store.contains(&d).await.unwrap());
        assert_eq!(store.get(&d).await.unwrap(), body);
    }

    #[tokio::test]
    async fn fs_layout_uses_two_char_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let store = FsBlobStore::new(dir.path()).unwrap();
        let body = Bytes::from_static(b"layout-check");
        let d = Digest::sha256_of(&body);
        store.put(&d, body).await.unwrap();
        let hex = d.hex();
        let prefix = &hex[..2];
        let rest = &hex[2..];
        let expected = dir.path().join("sha256").join(prefix).join(rest);
        assert!(expected.is_file());
    }

    #[tokio::test]
    async fn fs_get_missing_returns_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let store = FsBlobStore::new(dir.path()).unwrap();
        let d = Digest::sha256_of(b"absent");
        let err = store.get(&d).await.unwrap_err();
        assert!(matches!(err, BlobStoreError::NotFound(_)));
    }

    #[tokio::test]
    async fn fs_put_rejects_digest_mismatch() {
        let dir = tempfile::tempdir().unwrap();
        let store = FsBlobStore::new(dir.path()).unwrap();
        let lying = Digest::sha256_of(b"a");
        let real = Bytes::from_static(b"b");
        let err = store.put(&lying, real).await.unwrap_err();
        assert!(matches!(err, BlobStoreError::DigestMismatch { .. }));
    }

    #[tokio::test]
    async fn fs_delete_missing_ok() {
        let dir = tempfile::tempdir().unwrap();
        let store = FsBlobStore::new(dir.path()).unwrap();
        let d = Digest::sha256_of(b"never-stored");
        store.delete(&d).await.unwrap();
    }

    #[tokio::test]
    async fn fs_list_finds_all_entries() {
        let dir = tempfile::tempdir().unwrap();
        let store = FsBlobStore::new(dir.path()).unwrap();
        let bodies: &[&[u8]] = &[b"a", b"b", b"c"];
        let mut digests = Vec::new();
        for body in bodies {
            let d = Digest::sha256_of(body);
            store.put(&d, Bytes::copy_from_slice(body)).await.unwrap();
            digests.push(d);
        }
        let mut listed = store.list().await.unwrap();
        listed.sort_by_key(|d| d.hex().to_string());
        digests.sort_by_key(|d| d.hex().to_string());
        assert_eq!(listed, digests);
    }

    #[tokio::test]
    async fn fs_concurrent_put_same_digest_no_corruption() {
        let dir = tempfile::tempdir().unwrap();
        let store = FsBlobStore::new(dir.path()).unwrap();
        let body = Bytes::from_static(b"concurrent");
        let d = Digest::sha256_of(&body);

        // Spin up 8 puts of the same digest in parallel; each must
        // succeed and the final blob must be intact.
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let s = store.clone();
                let d = d.clone();
                let b = body.clone();
                tokio::spawn(async move { s.put(&d, b).await })
            })
            .collect();
        for h in handles {
            h.await.unwrap().unwrap();
        }
        assert_eq!(store.get(&d).await.unwrap(), body);
    }
}
