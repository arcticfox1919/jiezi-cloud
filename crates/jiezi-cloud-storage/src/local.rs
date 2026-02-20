//! Local filesystem storage backend.
//!
//! # Directory layout
//!
//! ```text
//! {root_dir}/
//! ├── chunks/
//! │   ├── ab/                 ← first 2 hex chars of hash
//! │   │   ├── ab3f…a1         ← remaining 62 hex chars (full hash as filename)
//! │   │   └── ab7c…b2
//! │   └── cd/
//! │       └── cd12…f3
//! └── temp/                   ← in-flight writes; cleaned up on rename
//! ```
//!
//! # Key format
//!
//! Callers pass a key of the form `"{prefix}/{hash}"`, e.g.
//! `"ab/abcdef1234…"`.  Both the two-character prefix and the full hash appear
//! in the key, which means the on-disk path is simply
//! `{root_dir}/chunks/{key}`.
//!
//! # Atomic writes
//!
//! To prevent partial/corrupt files surviving a mid-write crash:
//!
//! 1. Write all bytes to `{root_dir}/temp/{uuid}`.
//! 2. Call `sync_all()` (fsync) to flush the kernel buffer to disk.
//! 3. `rename()` the temp file to the final path.  On POSIX-compatible
//!    filesystems (ext4, APFS, ZFS…) rename is atomic.  On Windows, NTFS
//!    rename is not POSIX-atomic, but the content-addressed key means that
//!    even a duplicate write is idempotent and safe.
//!
//! # Read-time integrity check
//!
//! `get_chunk` re-computes the SHA-256 of every read and compares it to the
//! hash embedded in the key.  Bit-rot, silent disk errors, or manual
//! tampering are detected immediately, returning [`AppError::Storage`] rather
//! than silently serving corrupt data.

use std::path::PathBuf;

use async_trait::async_trait;
use bytes::Bytes;
use tokio::io::AsyncWriteExt;
use tracing::{debug, warn};

use jiezi_cloud_core::{
    error::{AppError, AppResult},
    types::{BackendId, HealthStatus},
};

use crate::hashing::{sha256_hex, validate_key};

/// Local filesystem implementation of [`StorageBackend`].
///
/// Create with [`LocalFsBackend::new`]; all subdirectories are created
/// automatically on first use.
#[derive(Debug, Clone)]
pub struct LocalFsBackend {
    id: BackendId,
    root_dir: PathBuf,
    chunks_dir: PathBuf,
    temp_dir: PathBuf,
}

impl LocalFsBackend {
    /// Construct a new backend rooted at `root_dir`.
    ///
    /// The directory does not need to exist yet; it will be created lazily.
    pub fn new(id: impl Into<BackendId>, root_dir: impl Into<PathBuf>) -> Self {
        let root = root_dir.into();
        let chunks = root.join("chunks");
        let temp = root.join("temp");
        Self {
            id: id.into(),
            root_dir: root,
            chunks_dir: chunks,
            temp_dir: temp,
        }
    }

    /// Return the filesystem path where `key` is stored.
    fn chunk_path(&self, key: &str) -> PathBuf {
        self.chunks_dir.join(key)
    }

    /// Extract the expected SHA-256 hash from a key.
    ///
    /// For a key like `"ab/abcdef1234…"` the hash is the component after the
    /// last `/`.  For a flat key (no `/`) the key itself is the hash.
    fn expected_hash(key: &str) -> &str {
        key.rsplit('/').next().unwrap_or(key)
    }
}

#[async_trait]
impl jiezi_cloud_core::traits::storage::StorageBackend for LocalFsBackend {
    fn backend_id(&self) -> &BackendId {
        &self.id
    }

    async fn put_chunk(&self, key: &str, data: Bytes) -> AppResult<()> {
        validate_key(key)?;

        let final_path = self.chunk_path(key);

        // Idempotent: content-addressed means same key → same bytes.
        // If the file already exists we can safely skip the write.
        if final_path.exists() {
            debug!(%key, "chunk already exists, skipping write");
            return Ok(());
        }

        // Ensure base directories exist
        if let Some(parent) = final_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::create_dir_all(&self.temp_dir).await?;

        // Step 1: write to a uniquely-named temp file
        let temp_path = self.temp_dir.join(uuid::Uuid::now_v7().to_string());
        {
            let mut file = tokio::fs::File::create(&temp_path).await?;
            file.write_all(&data).await?;
            file.flush().await?;
            file.sync_all().await?; // fsync — flush kernel buffers to disk
        }

        // Step 2: atomic rename to final location
        if let Err(e) = tokio::fs::rename(&temp_path, &final_path).await {
            // Clean up the temp file on rename failure; ignore secondary errors
            let _ = tokio::fs::remove_file(&temp_path).await;
            return Err(AppError::Storage(format!(
                "failed to commit chunk {key}: {e}"
            )));
        }

        debug!(%key, bytes = data.len(), "chunk written");
        Ok(())
    }

    async fn get_chunk(&self, key: &str) -> AppResult<Bytes> {
        validate_key(key)?;

        let path = self.chunk_path(key);
        let raw = tokio::fs::read(&path).await.map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                AppError::NotFound(format!("chunk not found: {key}"))
            } else {
                AppError::Storage(e.to_string())
            }
        })?;

        // Re-verify SHA-256 on every read to catch silent bit-rot
        let computed = sha256_hex(&raw);
        let expected = Self::expected_hash(key);
        if computed != expected {
            warn!(
                %key,
                expected,
                computed = %computed,
                "chunk integrity check FAILED — possible disk corruption"
            );
            return Err(AppError::Storage(format!(
                "chunk integrity check failed for {key}: \
                 expected hash {expected}, computed {computed}"
            )));
        }

        debug!(%key, bytes = raw.len(), "chunk read OK");
        Ok(Bytes::from(raw))
    }

    async fn get_chunk_range(&self, key: &str, range: std::ops::Range<u64>) -> AppResult<Bytes> {
        validate_key(key)?;

        // Read the entire chunk then slice.
        // For a future optimisation this could use positional I/O to avoid
        // loading the whole file, but correctness comes first.
        let full = self.get_chunk(key).await?;

        let start = range.start as usize;
        let end = range.end as usize;

        if end > full.len() {
            return Err(AppError::Validation(format!(
                "range {start}..{end} exceeds chunk length {} for key {key}",
                full.len()
            )));
        }
        if start > end {
            return Err(AppError::Validation(format!(
                "range start {start} is greater than end {end}"
            )));
        }

        Ok(full.slice(start..end))
    }

    async fn delete_chunk(&self, key: &str) -> AppResult<()> {
        validate_key(key)?;

        let path = self.chunk_path(key);
        match tokio::fs::remove_file(&path).await {
            Ok(()) => {
                debug!(%key, "chunk deleted");
                Ok(())
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // Idempotent: deleting a non-existent chunk is a no-op
                Ok(())
            }
            Err(e) => Err(AppError::Storage(e.to_string())),
        }
    }

    async fn exists(&self, key: &str) -> AppResult<bool> {
        validate_key(key)?;
        Ok(self.chunk_path(key).exists())
    }

    async fn health_check(&self) -> AppResult<HealthStatus> {
        // Ensure the root directory is accessible
        if !self.root_dir.exists() {
            return Ok(HealthStatus::Unhealthy {
                reason: format!(
                    "storage root directory does not exist: {}",
                    self.root_dir.display()
                ),
            });
        }

        // Check write permission by attempting to create a probe file
        let probe = self.temp_dir.join(".health_probe");
        if let Err(e) = tokio::fs::create_dir_all(&self.temp_dir).await {
            return Ok(HealthStatus::Unhealthy {
                reason: format!("cannot create temp dir: {e}"),
            });
        }
        match tokio::fs::write(&probe, b"ok").await {
            Ok(()) => {
                let _ = tokio::fs::remove_file(&probe).await;
            }
            Err(e) => {
                return Ok(HealthStatus::Unhealthy {
                    reason: format!("storage directory not writable: {e}"),
                });
            }
        }

        Ok(HealthStatus::Healthy)
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use jiezi_cloud_core::traits::storage::StorageBackend;

    /// Create a backend in a temporary directory that is removed after the test.
    fn make_backend(tmp: &tempfile::TempDir) -> LocalFsBackend {
        LocalFsBackend::new("test-backend", tmp.path())
    }

    /// Helper: derive the canonical key for a byte slice.
    fn key_for(data: &[u8]) -> String {
        let hash = sha256_hex(data);
        let prefix = &hash[..2];
        format!("{prefix}/{hash}")
    }

    // ── put_chunk / get_chunk ────────────────────────────────────────────────

    #[tokio::test]
    async fn test_put_and_get_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let backend = make_backend(&tmp);

        let data = Bytes::from_static(b"hello, local storage!");
        let key = key_for(&data);

        backend.put_chunk(&key, data.clone()).await.unwrap();
        let retrieved = backend.get_chunk(&key).await.unwrap();
        assert_eq!(retrieved, data);
    }

    #[tokio::test]
    async fn test_put_is_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let backend = make_backend(&tmp);

        let data = Bytes::from_static(b"idempotent write");
        let key = key_for(&data);

        backend.put_chunk(&key, data.clone()).await.unwrap();
        // Second write must succeed without error
        backend.put_chunk(&key, data.clone()).await.unwrap();

        let retrieved = backend.get_chunk(&key).await.unwrap();
        assert_eq!(retrieved, data);
    }

    #[tokio::test]
    async fn test_get_nonexistent_returns_not_found() {
        let tmp = tempfile::tempdir().unwrap();
        let backend = make_backend(&tmp);

        let hash = "a".repeat(64);
        let key = format!("aa/{hash}");
        let err = backend.get_chunk(&key).await.unwrap_err();
        assert!(
            matches!(err, AppError::NotFound(_)),
            "expected NotFound, got: {err:?}"
        );
    }

    // ── exists ───────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_exists_after_put() {
        let tmp = tempfile::tempdir().unwrap();
        let backend = make_backend(&tmp);

        let data = Bytes::from_static(b"existence check");
        let key = key_for(&data);

        assert!(!backend.exists(&key).await.unwrap());
        backend.put_chunk(&key, data).await.unwrap();
        assert!(backend.exists(&key).await.unwrap());
    }

    // ── delete_chunk ─────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_delete_removes_chunk() {
        let tmp = tempfile::tempdir().unwrap();
        let backend = make_backend(&tmp);

        let data = Bytes::from_static(b"to be deleted");
        let key = key_for(&data);

        backend.put_chunk(&key, data).await.unwrap();
        assert!(backend.exists(&key).await.unwrap());

        backend.delete_chunk(&key).await.unwrap();
        assert!(!backend.exists(&key).await.unwrap());
    }

    #[tokio::test]
    async fn test_delete_nonexistent_is_noop() {
        let tmp = tempfile::tempdir().unwrap();
        let backend = make_backend(&tmp);

        let hash = "b".repeat(64);
        let key = format!("bb/{hash}");
        // Must not return an error
        backend.delete_chunk(&key).await.unwrap();
    }

    // ── get_chunk_range ──────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_range_read_correct_slice() {
        let tmp = tempfile::tempdir().unwrap();
        let backend = make_backend(&tmp);

        let data = Bytes::from_static(b"0123456789abcdef");
        let key = key_for(&data);
        backend.put_chunk(&key, data).await.unwrap();

        let slice = backend.get_chunk_range(&key, 4..8).await.unwrap();
        assert_eq!(&slice[..], b"4567");
    }

    #[tokio::test]
    async fn test_range_full_file() {
        let tmp = tempfile::tempdir().unwrap();
        let backend = make_backend(&tmp);

        let data = Bytes::from_static(b"full range");
        let key = key_for(&data);
        backend.put_chunk(&key, data.clone()).await.unwrap();

        let slice = backend.get_chunk_range(&key, 0..data.len() as u64).await.unwrap();
        assert_eq!(slice, data);
    }

    #[tokio::test]
    async fn test_range_out_of_bounds_returns_error() {
        let tmp = tempfile::tempdir().unwrap();
        let backend = make_backend(&tmp);

        let data = Bytes::from_static(b"short");
        let key = key_for(&data);
        backend.put_chunk(&key, data).await.unwrap();

        let err = backend.get_chunk_range(&key, 0..100).await.unwrap_err();
        assert!(
            matches!(err, AppError::Validation(_)),
            "out-of-bounds range must return Validation error, got: {err:?}"
        );
    }

    // ── path validation ──────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_key_with_path_traversal_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let backend = make_backend(&tmp);

        let bad_keys = [
            "../escape",
            "ab/../etc/passwd",
            "/absolute",
            "C:\\windows",
            "sub/./file",
        ];

        for key in bad_keys {
            let err = backend.put_chunk(key, Bytes::from_static(b"x")).await.unwrap_err();
            assert!(
                matches!(err, AppError::Validation(_)),
                "key '{key}' must be rejected with Validation error, got: {err:?}"
            );
        }
    }

    #[tokio::test]
    async fn test_empty_key_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let backend = make_backend(&tmp);
        let err = backend.put_chunk("", Bytes::from_static(b"x")).await.unwrap_err();
        assert!(matches!(err, AppError::Validation(_)));
    }

    // ── integrity check ──────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_get_detects_corrupted_file() {
        let tmp = tempfile::tempdir().unwrap();
        let backend = make_backend(&tmp);

        let data = Bytes::from_static(b"original content");
        let key = key_for(&data);
        backend.put_chunk(&key, data.clone()).await.unwrap();

        // Corrupt the file on disk by overwriting with different content
        let on_disk = backend.chunk_path(&key);
        tokio::fs::write(&on_disk, b"CORRUPTED DATA!!!").await.unwrap();

        let err = backend.get_chunk(&key).await.unwrap_err();
        assert!(
            matches!(err, AppError::Storage(_)),
            "corrupted chunk must return Storage error, got: {err:?}"
        );
    }

    // ── health_check ─────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_health_check_new_backend() {
        let tmp = tempfile::tempdir().unwrap();
        // Pre-create root so health_check can find it
        tokio::fs::create_dir_all(tmp.path()).await.unwrap();
        let backend = make_backend(&tmp);
        // health_check should succeed on a valid directory
        let status = backend.health_check().await.unwrap();
        assert!(status.is_healthy(), "new backend must be healthy");
    }

    #[tokio::test]
    async fn test_health_check_missing_root() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("does_not_exist");
        let backend = LocalFsBackend::new("ghost", root);
        let status = backend.health_check().await.unwrap();
        assert!(
            !status.is_healthy(),
            "backend with missing root must not be healthy"
        );
    }

    // ── concurrent write ─────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_concurrent_writes_same_key() {
        use std::sync::Arc;

        let tmp = tempfile::tempdir().unwrap();
        let backend = Arc::new(make_backend(&tmp));

        let data = Bytes::from_static(b"concurrent");
        let key = key_for(&data);

        let mut handles = Vec::new();
        for _ in 0..8 {
            let b = Arc::clone(&backend);
            let k = key.clone();
            let d = data.clone();
            handles.push(tokio::spawn(async move { b.put_chunk(&k, d).await }));
        }

        for h in handles {
            h.await.unwrap().unwrap();
        }

        // Final state must be correct, readable, and untampered
        let retrieved = backend.get_chunk(&key).await.unwrap();
        assert_eq!(retrieved, data);
    }
}
