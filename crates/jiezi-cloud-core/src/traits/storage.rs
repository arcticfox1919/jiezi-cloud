//! Storage backend trait.

use async_trait::async_trait;
use bytes::Bytes;

use crate::error::AppResult;
use crate::types::{BackendId, HealthStatus};

/// Abstraction over a physical binary storage backend.
///
/// Callers address chunks by an opaque string `key` (typically the SHA-256
/// hex digest of the content with a two-character directory prefix for sharding,
/// e.g. `"ab/abcdef1234…"`).
///
/// Concrete implementations provided in `jiezi-cloud-storage`:
///
/// - `LocalFsBackend` — POSIX/NTFS local filesystem.
/// - `S3Backend` — Amazon S3 or any S3-compatible service (MinIO, Backblaze).
/// - `WebDavBackend` — WebDAV endpoint (e.g. Nextcloud, OneDrive).
///
/// # Deduplication
///
/// All operations are **content-addressed**.  Because the key embeds the hash
/// of the content, writing the same data twice is a no-op (idempotent writes).
///
/// # Safety
///
/// Implementations MUST validate that `key` does not contain path traversal
/// sequences such as `..` or absolute path prefixes.
#[async_trait]
pub trait StorageBackend: Send + Sync {
    /// Return the unique, human-readable identifier of this backend instance.
    fn backend_id(&self) -> &BackendId;

    /// Persist a chunk under `key` (content-addressed, idempotent).
    ///
    /// If a chunk with `key` already exists its content MUST be identical
    /// (same hash), so overwriting is safe.
    ///
    /// # Errors
    ///
    /// - [`AppError::Validation`] if `key` contains a path traversal sequence.
    /// - [`AppError::Storage`] on I/O failure.
    async fn put_chunk(&self, key: &str, data: Bytes) -> AppResult<()>;

    /// Retrieve the full content of a chunk by key.
    ///
    /// # Errors
    ///
    /// - [`AppError::NotFound`] if no chunk with `key` exists.
    /// - [`AppError::Storage`] on I/O failure.
    async fn get_chunk(&self, key: &str) -> AppResult<Bytes>;

    /// Retrieve a byte sub-range of a chunk for range-request support.
    ///
    /// `range` is a half-open interval `[start, end)` in bytes relative to
    /// the start of the chunk.
    ///
    /// # Errors
    ///
    /// - [`AppError::NotFound`] if no chunk with `key` exists.
    /// - [`AppError::Validation`] if `range` exceeds the chunk length.
    /// - [`AppError::Storage`] on I/O failure.
    async fn get_chunk_range(&self, key: &str, range: std::ops::Range<u64>) -> AppResult<Bytes>;

    /// Delete a chunk.
    ///
    /// This is a no-op (not an error) if the chunk does not exist.
    ///
    /// # Errors
    ///
    /// - [`AppError::Storage`] on I/O failure.
    async fn delete_chunk(&self, key: &str) -> AppResult<()>;

    /// Return `true` if a chunk with `key` exists on this backend.
    ///
    /// # Errors
    ///
    /// - [`AppError::Storage`] on I/O failure.
    async fn exists(&self, key: &str) -> AppResult<bool>;

    /// Check the health of this backend and return a status report.
    async fn health_check(&self) -> AppResult<HealthStatus>;
}
