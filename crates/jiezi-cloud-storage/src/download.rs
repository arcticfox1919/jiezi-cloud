//! File download pipeline.
//!
//! [`DownloadService`] reads `file_chunks` rows, resolves each chunk from the
//! [`StorageManager`], and reassembles the original byte stream.
//!
//! # Range reads
//!
//! [`DownloadService::read_range`] computes byte-range boundaries from the
//! stored `size_bytes` fields — no full-file read is needed; only the chunks
//! that overlap the requested range are fetched from backends.

use std::sync::Arc;

use bytes::{Bytes, BytesMut};
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder};

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
