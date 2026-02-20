//! File upload pipeline.
//!
//! [`UploadService`] chains the FastCDC chunker → [`StorageManager`] →
//! persists `file_chunks` + `chunk_locations` rows in a single logical
//! operation.
//!
//! # Deduplication
//!
//! Before writing a chunk to any backend the pipeline checks whether at least
//! one `chunk_locations` row already exists for that `chunk_hash`.  If the
//! chunk is already present on some backend no backend writes are issued —
//! only the `file_chunks` mapping rows are inserted.
//!
//! # Error handling
//!
//! - A chunk write is considered successful if **at least one** backend
//!   accepted it.  Failed backends are logged as warnings; their IDs are not
//!   written to `chunk_locations`.
//! - If **all** backends reject a chunk the upload is aborted with
//!   [`AppError::Storage`].

use std::sync::Arc;

use bytes::Bytes;
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, DatabaseConnection, EntityTrait,
    PaginatorTrait, QueryFilter, QuerySelect,
};
use tracing::{debug, warn};

use jiezi_cloud_core::{
    error::{AppError, AppResult},
    models::backend::ReplicationPolicy,
    types::FileId,
};

use crate::{
    chunking::FastCdcChunker,
    entities::{chunk_locations, file_chunks},
    hashing::sha256_hex,
    manager::StorageManager,
};

// ─── StoredFileInfo ───────────────────────────────────────────────────────────

/// Result returned by [`UploadService::store_file`].
#[derive(Debug, Clone)]
pub struct StoredFileInfo {
    /// SHA-256 hex digest of the **complete** file content.
    pub content_hash: String,
    /// Total file size in bytes.
    pub total_size: u64,
    /// Number of CDC chunks stored.
    pub chunk_count: u32,
}

// ─── UploadService ────────────────────────────────────────────────────────────

/// Orchestrates the full upload pipeline: CDC → backends → DB records.
#[derive(Clone)]
pub struct UploadService {
    storage: Arc<StorageManager>,
    db: DatabaseConnection,
}

impl UploadService {
    /// Create a new [`UploadService`].
    pub fn new(storage: Arc<StorageManager>, db: DatabaseConnection) -> Self {
        Self { storage, db }
    }

    /// Store `data` associated with `file_id` using the given replication
    /// `policy`, then persist `file_chunks` and `chunk_locations` rows.
    ///
    /// Callers are expected to create the VFS [`FileNode`] record
    /// **after** this method succeeds, using the returned [`StoredFileInfo`].
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Storage`] if every backend rejected at least one chunk.
    /// Returns [`AppError::Database`] on any DB write failure.
    pub async fn store_file(
        &self,
        file_id: &FileId,
        data: Bytes,
        policy: &ReplicationPolicy,
    ) -> AppResult<StoredFileInfo> {
        let total_size = data.len() as u64;
        let content_hash = sha256_hex(&data);

        // ── 1. CDC chunking ───────────────────────────────────────────────────
        let chunker = FastCdcChunker::default();
        let chunks = if data.is_empty() {
            Vec::new()
        } else {
            chunker.chunk(&data)
        };
        let chunk_count = chunks.len() as u32;

        debug!(
            file_id = %file_id,
            total_size,
            chunk_count,
            "upload pipeline started"
        );

        // ── 2. Per-chunk: dedup check → backend write → chunk_locations ───────
        for (seq, chunk) in chunks.iter().enumerate() {
            let sequence = seq as u32;
            let storage_key = chunk_key(&chunk.hash);

            // Dedup: if any chunk_locations row exists, skip the backend write.
            let already_stored: bool = chunk_locations::Entity::find()
                .filter(chunk_locations::Column::ChunkHash.eq(&chunk.hash))
                .limit(1)
                .count(&self.db)
                .await
                .map_err(|e: sea_orm::DbErr| AppError::Database(e.to_string()))?
                > 0;

            if !already_stored {
                // Write to backends according to the replication policy.
                let results = self
                    .storage
                    .put_with_policy(&storage_key, chunk.data.clone(), policy)
                    .await;

                let mut any_success = false;
                for (backend_id, result) in &results {
                    match result {
                        Ok(()) => {
                            any_success = true;
                            // Persist a chunk_locations row for this backend.
                            let loc = chunk_locations::ActiveModel {
                                chunk_hash: Set(chunk.hash.clone()),
                                backend_id: Set(backend_id.as_str().to_owned()),
                                storage_key: Set(storage_key.clone()),
                                size_bytes: Set(chunk.size as i64),
                                verified_at: Set(None),
                            };
                            // INSERT OR IGNORE — another concurrent upload may
                            // have just written this row; ignore the conflict.
                            if let Err(e) = loc.insert(&self.db).await {
                                // Treat a duplicate-key error as a harmless race.
                                let msg = e.to_string();
                                if msg.contains("UNIQUE") || msg.contains("duplicate") {
                                    debug!(chunk_hash = %chunk.hash, backend = %backend_id, "chunk_locations row already exists (race)");
                                } else {
                                    return Err(AppError::Database(msg));
                                }
                            }
                        }
                        Err(e) => {
                            warn!(
                                chunk_hash = %chunk.hash,
                                backend = %backend_id,
                                error = %e,
                                "backend rejected chunk; skipping"
                            );
                        }
                    }
                }

                if !any_success {
                    return Err(AppError::Storage(format!(
                        "all backends rejected chunk {} (sequence {sequence})",
                        chunk.hash
                    )));
                }
            } else {
                debug!(chunk_hash = %chunk.hash, sequence, "chunk already stored — dedup hit");
            }

            // ── 3. Persist the file → chunk mapping ───────────────────────────
            let fc = file_chunks::ActiveModel {
                file_id: Set(file_id.to_string()),
                sequence: Set(sequence as i32),
                chunk_hash: Set(chunk.hash.clone()),
                size_bytes: Set(chunk.size as i64),
            };
            fc.insert(&self.db)
                .await
                .map_err(|e| AppError::Database(e.to_string()))?;
        }

        debug!(
            file_id = %file_id,
            content_hash,
            total_size,
            chunk_count,
            "upload pipeline complete"
        );

        Ok(StoredFileInfo { content_hash, total_size, chunk_count })
    }

    /// Delete all `file_chunks` rows for `file_id`.
    ///
    /// Does **not** remove `chunk_locations` rows — those are cleaned up by
    /// the garbage-collector once no `file_chunks` references remain.
    pub async fn delete_file_chunks(&self, file_id: &FileId) -> AppResult<()> {
        file_chunks::Entity::delete_many()
            .filter(file_chunks::Column::FileId.eq(file_id.to_string()))
            .exec(&self.db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(())
    }
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

/// Derive the backend storage key from a SHA-256 hex hash.
///
/// Format: `{first_2_hex_chars}/{full_64_char_hash}`.
/// The two-character prefix provides a shallow directory tree that keeps
/// per-directory entry counts manageable on local filesystems.
pub(crate) fn chunk_key(hash: &str) -> String {
    // Guaranteed to have at least 2 chars for a valid SHA-256 hex string.
    let prefix = &hash[..2.min(hash.len())];
    format!("{prefix}/{hash}")
}
