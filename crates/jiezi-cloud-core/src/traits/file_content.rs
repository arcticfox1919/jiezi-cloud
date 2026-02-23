//! Single-file content reader trait.
//!
//! Provides a minimal abstraction for reading the complete contents of a VFS
//! file as a byte buffer.  Implemented by `jiezi-cloud-storage`'s
//! `DownloadService` so that upper-layer crates (`jiezi-cloud-kb`, etc.) can
//! read file content **without** depending on storage-layer internals.

use async_trait::async_trait;
use bytes::Bytes;

use crate::error::AppResult;
use crate::types::FileId;

/// Contract for reading the full byte content of a VFS file.
///
/// Implementations reassemble chunk streams and return a single contiguous
/// buffer.  This is suitable for small-to-medium files (text documents,
/// Markdown notes).  Large binary files should be streamed instead.
#[async_trait]
pub trait FileContentReader: Send + Sync {
    /// Read the complete contents of the file identified by `file_id`.
    ///
    /// # Errors
    ///
    /// - [`AppError::NotFound`] if the file does not exist or has no chunks.
    /// - [`AppError::Storage`] on backend I/O failure.
    async fn read_file(&self, file_id: &FileId) -> AppResult<Bytes>;
}
