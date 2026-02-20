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


#[cfg(test)]
#[path = "local_tests.rs"]
mod tests;