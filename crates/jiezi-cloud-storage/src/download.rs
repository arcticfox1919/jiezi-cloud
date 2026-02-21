//! File download pipeline.
//!
//! [`DownloadService`] reads `file_chunks` rows, resolves each chunk from the
//! [`StorageManager`], and reassembles the original byte stream.
//!
//! # Streaming
//!
//! [`DownloadService::stream_file`] returns a [`FileDownloadStream`] that
//! yields CDC chunks one at a time via an mpsc channel. Peak memory stays
//! O(CDC_chunk) ≈ 16 KiB rather than O(file_size).
//!
//! # Range reads
//!
//! [`DownloadService::read_range`] computes byte-range boundaries from the
//! stored `size_bytes` fields — no full-file read is needed; only the chunks
//! that overlap the requested range are fetched from backends.

use std::sync::Arc;

use bytes::{Bytes, BytesMut};
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder};
use tokio::sync::mpsc;

use jiezi_cloud_core::{
    error::{AppError, AppResult},
    types::FileId,
};

use crate::{
    entities::file_chunks,
    manager::StorageManager,
    upload::chunk_key,
};

// ─── DownloadService ──────────────────────────────────────────────────────────

/// Metadata needed to start a download session (e.g. `DOWNLOAD_INFO` frame)
/// before the first chunk arrives.
pub struct FileDownloadMeta {
    /// Total file size in bytes (sum of all CDC chunk sizes).
    pub total_size: u64,
    /// Number of CDC storage chunks that make up the file.
    pub chunk_count: u32,
}

/// A streaming handle to a file's content.
///
/// CDC chunks are yielded one at a time through a bounded mpsc channel.
/// The producer task runs in the background so the consumer can buffer or
/// re-chunk at its own pace.
pub struct FileDownloadStream {
    /// Metadata about the file (available immediately, before any I/O).
    pub meta: FileDownloadMeta,
    /// Receiver end — each message is one CDC chunk (`Bytes`).
    pub rx: mpsc::Receiver<AppResult<Bytes>>,
}

/// Reassembles file content from chunk records and storage backends.
#[derive(Clone)]
pub struct DownloadService {
    storage: Arc<StorageManager>,
    db: DatabaseConnection,
}

impl DownloadService {
    /// Create a new [`DownloadService`].
    pub fn new(storage: Arc<StorageManager>, db: DatabaseConnection) -> Self {
        Self { storage, db }
    }

    /// Read the complete content of `file_id` into memory.
    ///
    /// Chunks are fetched in sequence order and concatenated.
    ///
    /// # Errors
    ///
    /// - [`AppError::NotFound`] if no chunks exist for `file_id`.
    /// - [`AppError::Storage`] if a chunk cannot be retrieved from any backend.
    pub async fn read_file(&self, file_id: &FileId) -> AppResult<Bytes> {
        let chunks = self.load_chunk_list(file_id).await?;

        if chunks.is_empty() {
            return Err(AppError::NotFound(format!(
                "no chunks found for file {file_id}"
            )));
        }

        let total: usize = chunks.iter().map(|c| c.size_bytes as usize).sum();
        let mut buf = BytesMut::with_capacity(total);

        for chunk in &chunks {
            let key = chunk_key(&chunk.chunk_hash);
            let data = self.storage.get_from_any(&key).await?;
            buf.extend_from_slice(&data);
        }

        Ok(buf.freeze())
    }

    /// Read a byte range `[start, end)` of `file_id`.
    ///
    /// Uses stored chunk sizes to fetch only the chunks that overlap the
    /// requested range.  Only the overlapping slices are buffered in memory.
    ///
    /// # Errors
    ///
    /// - [`AppError::NotFound`] if `file_id` has no chunks or the range is OOB.
    /// - [`AppError::Storage`] if a required chunk cannot be retrieved.
    pub async fn read_range(
        &self,
        file_id: &FileId,
        start: u64,
        end: u64,
    ) -> AppResult<Bytes> {
        if start >= end {
            return Ok(Bytes::new());
        }

        let chunks = self.load_chunk_list(file_id).await?;

        if chunks.is_empty() {
            return Err(AppError::NotFound(format!(
                "no chunks found for file {file_id}"
            )));
        }

        let mut buf = BytesMut::new();
        let mut chunk_start: u64 = 0;

        for chunk in &chunks {
            let chunk_end = chunk_start + chunk.size_bytes as u64;

            // Skip chunks entirely before the range.
            if chunk_end <= start {
                chunk_start = chunk_end;
                continue;
            }
            // Stop once we're past the end of the range.
            if chunk_start >= end {
                break;
            }

            let key = chunk_key(&chunk.chunk_hash);

            // Compute the slice within this chunk that we need.
            let slice_start = if start > chunk_start { start - chunk_start } else { 0 };
            let slice_end = (end - chunk_start).min(chunk.size_bytes as u64);

            if slice_start == 0 && slice_end == chunk.size_bytes as u64 {
                // Entire chunk is needed.
                let data = self.storage.get_from_any(&key).await?;
                buf.extend_from_slice(&data);
            } else {
                // Partial chunk: fetch the exact range from the backend.
                let data = self
                    .storage
                    .get_range_from_any(&key, slice_start..slice_end)
                    .await?;
                buf.extend_from_slice(&data);
            }

            chunk_start = chunk_end;
        }

        if buf.is_empty() {
            return Err(AppError::NotFound(format!(
                "byte range {start}..{end} is out of bounds for file {file_id}"
            )));
        }

        Ok(buf.freeze())
    }

    /// Return file metadata (total_size and chunk_count) without loading any
    /// chunk data.
    ///
    /// Useful when sending `DOWNLOAD_INFO` frames where the caller only needs
    /// size/count but will stream the actual data separately.
    pub async fn file_metadata(&self, file_id: &FileId) -> AppResult<FileDownloadMeta> {
        let chunks = self.load_chunk_list(file_id).await?;
        if chunks.is_empty() {
            return Err(AppError::NotFound(format!(
                "no chunks found for file {file_id}"
            )));
        }
        let total_size: u64 = chunks.iter().map(|c| c.size_bytes as u64).sum();
        Ok(FileDownloadMeta {
            total_size,
            chunk_count: chunks.len() as u32,
        })
    }

    /// Stream the file content as a series of CDC chunks.
    ///
    /// Returns immediately with a [`FileDownloadStream`] containing metadata
    /// and a receiver.  A background task sequentially loads each CDC chunk
    /// from storage and sends it through the channel.
    ///
    /// The channel buffer of 4 keeps memory usage bounded to approximately
    /// 4 × max_CDC_size ≈ 64 KiB regardless of file size.
    ///
    /// # Errors
    ///
    /// - [`AppError::NotFound`] if `file_id` has no chunks.
    pub async fn stream_file(&self, file_id: &FileId) -> AppResult<FileDownloadStream> {
        let chunks = self.load_chunk_list(file_id).await?;
        if chunks.is_empty() {
            return Err(AppError::NotFound(format!(
                "no chunks found for file {file_id}"
            )));
        }

        let total_size: u64 = chunks.iter().map(|c| c.size_bytes as u64).sum();
        let chunk_count = chunks.len() as u32;

        let (tx, rx) = mpsc::channel(4);
        let storage = Arc::clone(&self.storage);

        tokio::spawn(async move {
            for chunk in chunks {
                let key = chunk_key(&chunk.chunk_hash);
                let result = storage.get_from_any(&key).await;
                if tx.send(result).await.is_err() {
                    break; // Receiver dropped — stop producing.
                }
            }
        });

        Ok(FileDownloadStream {
            meta: FileDownloadMeta { total_size, chunk_count },
            rx,
        })
    }

    // ── Helpers ───────────────────────────────────────────────────────────────

    /// Load all `file_chunks` rows for `file_id`, ordered by `sequence`.
    async fn load_chunk_list(&self, file_id: &FileId) -> AppResult<Vec<file_chunks::Model>> {
        file_chunks::Entity::find()
            .filter(file_chunks::Column::FileId.eq(file_id.to_string()))
            .order_by_asc(file_chunks::Column::Sequence)
            .all(&self.db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))
    }
}
