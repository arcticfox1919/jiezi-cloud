//! Repository for the `upload_sessions` table.
//!
//! Manages the lifecycle of resumable upload sessions: creation, chunk
//! tracking, finalization, cancellation, and expiry-based garbage collection.
//!
//! # Temporary chunk files
//!
//! Raw chunk bytes are stored on the local filesystem under
//! `{tmp_dir}/{session_id}/{chunk_index:06}`.
//! The repository is responsible for cleaning up these files when a session
//! completes or is cancelled.

use std::path::{Path, PathBuf};

use chrono::Utc;
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, DatabaseConnection, EntityTrait,
    QueryFilter, TransactionTrait,
};
use tracing::{debug, warn};

use jiezi_cloud_core::error::{AppError, AppResult};
use jiezi_cloud_core::types::UserId;

use crate::entities::upload_sessions::{
    self, ActiveModel, Column, Entity, STATUS_CANCELLED, STATUS_COMPLETE, STATUS_PENDING,
};

// ─── Domain types ─────────────────────────────────────────────────────────────

/// Default HTTP-chunk size for resumable uploads (8 MiB).
pub const DEFAULT_CHUNK_SIZE: u64 = 8 * 1024 * 1024;

/// How long (seconds) a session lives before expiring (24 hours).
pub const SESSION_TTL_SECS: i64 = 24 * 3600;

/// Complete view of an upload session including the decoded chunk list.
#[derive(Debug, Clone)]
pub struct UploadSession {
    pub id: String,
    pub user_id: String,
    pub parent_id: String,
    pub file_name: String,
    /// Total file size in bytes.
    pub total_size: u64,
    pub content_hash: String,
    pub mime_type: Option<String>,
    pub chunk_size: u64,
    pub chunk_count: u32,
    /// Sorted list of received chunk indices.
    pub received_chunks: Vec<u32>,
    pub status: String,
    pub created_at: chrono::DateTime<Utc>,
    pub expires_at: chrono::DateTime<Utc>,
}

impl TryFrom<upload_sessions::Model> for UploadSession {
    type Error = AppError;

    fn try_from(m: upload_sessions::Model) -> Result<Self, Self::Error> {
        let received_chunks: Vec<u32> =
            serde_json::from_str(&m.received_chunks).map_err(|e| {
                AppError::Serialization(format!(
                    "upload_session received_chunks parse error: {e}"
                ))
            })?;
        Ok(Self {
            id: m.id,
            user_id: m.user_id,
            parent_id: m.parent_id,
            file_name: m.file_name,
            total_size: m.total_size as u64,
            content_hash: m.content_hash,
            mime_type: m.mime_type,
            chunk_size: m.chunk_size as u64,
            chunk_count: m.chunk_count as u32,
            received_chunks,
            status: m.status,
            created_at: m.created_at,
            expires_at: m.expires_at,
        })
    }
}

impl UploadSession {
    /// Returns the indices of chunks that have not yet been received.
    pub fn missing_chunks(&self) -> Vec<u32> {
        let received: std::collections::HashSet<u32> =
            self.received_chunks.iter().copied().collect();
        (0..self.chunk_count)
            .filter(|i| !received.contains(i))
            .collect()
    }

    /// Returns `true` when all chunks have been received.
    pub fn is_complete(&self) -> bool {
        self.received_chunks.len() == self.chunk_count as usize
    }
}

// ─── Repository ───────────────────────────────────────────────────────────────

/// Repository for resumable upload sessions.
///
/// Cheap to clone: the inner [`DatabaseConnection`] is already `Arc`-backed.
#[derive(Clone)]
pub struct UploadSessionRepository {
    db: DatabaseConnection,
    /// Root directory for temporary chunk files.
    /// Layout: `{tmp_dir}/{session_id}/{chunk_index:06}`.
    tmp_dir: PathBuf,
}

impl UploadSessionRepository {
    /// Create a new repository targeting `tmp_dir` for chunk storage.
    ///
    /// `tmp_dir` is created lazily at the first `create` call.
    pub fn new(db: DatabaseConnection, tmp_dir: PathBuf) -> Self {
        Self { db, tmp_dir }
    }

    // ── Read ──────────────────────────────────────────────────────────────────

    /// Fetch a session by ID.  Returns `None` when not found.
    pub async fn find(&self, session_id: &str) -> AppResult<Option<UploadSession>> {
        let model = Entity::find_by_id(session_id)
            .one(&self.db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))?;
        model.map(UploadSession::try_from).transpose()
    }

    /// Fetch a session by ID and verify it belongs to `user_id`.
    ///
    /// Returns `NotFound` when the session doesn't exist or belongs to another user.
    pub async fn find_for_user(
        &self,
        session_id: &str,
        user_id: &UserId,
    ) -> AppResult<UploadSession> {
        let session = self
            .find(session_id)
            .await?
            .ok_or_else(|| AppError::NotFound(format!("upload session: {session_id}")))?;

        if session.user_id != user_id.to_string() {
            return Err(AppError::NotFound(format!(
                "upload session: {session_id}"
            )));
        }

        // Treat terminal sessions (cancelled) as gone so clients get 404 rather
        // than confusing stale data.
        if session.status == STATUS_CANCELLED {
            return Err(AppError::NotFound(format!(
                "upload session: {session_id}"
            )));
        }

        Ok(session)
    }

    // ── Write ─────────────────────────────────────────────────────────────────

    /// Create a new pending upload session and return it.
    ///
    /// Creates the temp directory for chunk files as a side-effect.
    pub async fn create(
        &self,
        user_id: &UserId,
        parent_id: &str,
        file_name: &str,
        total_size: u64,
        content_hash: &str,
        mime_type: Option<String>,
        chunk_size: u64,
    ) -> AppResult<UploadSession> {
        let id = uuid::Uuid::new_v4().to_string();
        let now = Utc::now();
        let expires_at = now + chrono::Duration::seconds(SESSION_TTL_SECS);

        // chunk_count = ceil(total_size / chunk_size), min 1
        let chunk_count = total_size
            .div_ceil(chunk_size)
            .max(1) as i32;

        // Create temp directory for this session's chunks.
        let session_tmp = self.session_tmp_dir(&id);
        tokio::fs::create_dir_all(&session_tmp)
            .await
            .map_err(|e| AppError::Storage(format!("create upload tmp dir: {e}")))?;

        let am = ActiveModel {
            id: Set(id.clone()),
            user_id: Set(user_id.to_string()),
            parent_id: Set(parent_id.to_owned()),
            file_name: Set(file_name.to_owned()),
            total_size: Set(total_size as i64),
            content_hash: Set(content_hash.to_owned()),
            mime_type: Set(mime_type),
            chunk_size: Set(chunk_size as i64),
            chunk_count: Set(chunk_count),
            received_chunks: Set("[]".to_owned()),
            status: Set(STATUS_PENDING.to_owned()),
            created_at: Set(now),
            expires_at: Set(expires_at),
        };

        am.insert(&self.db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))?;

        self.find(&id)
            .await?
            .ok_or_else(|| AppError::Internal("session vanished after insert".into()))
    }

    /// Record that chunk `index` has been received.
    ///
    /// Writes the chunk bytes to disk atomically (temp → rename) and then
    /// updates `received_chunks` in the database.
    ///
    /// Returns the updated session on success.
    pub async fn receive_chunk(
        &self,
        session: &UploadSession,
        index: u32,
        data: &[u8],
    ) -> AppResult<UploadSession> {
        let chunk_path = self.chunk_path(&session.id, index);

        // Write to a temp file then rename for atomicity.
        let tmp_path = chunk_path.with_extension("tmp");
        tokio::fs::write(&tmp_path, data)
            .await
            .map_err(|e| AppError::Storage(format!("write chunk {index}: {e}")))?;
        tokio::fs::rename(&tmp_path, &chunk_path)
            .await
            .map_err(|e| AppError::Storage(format!("rename chunk {index}: {e}")))?;

        // Atomically update received_chunks list.
        let txn = self
            .db
            .begin()
            .await
            .map_err(|e| AppError::Database(e.to_string()))?;

        let model = Entity::find_by_id(&session.id)
            .one(&txn)
            .await
            .map_err(|e| AppError::Database(e.to_string()))?
            .ok_or_else(|| AppError::NotFound(format!("upload session: {}", session.id)))?;

        let mut received: Vec<u32> =
            serde_json::from_str(&model.received_chunks).unwrap_or_default();
        if !received.contains(&index) {
            received.push(index);
            received.sort_unstable();
        }

        let updated_json = serde_json::to_string(&received)
            .map_err(|e| AppError::Serialization(e.to_string()))?;

        let mut am: ActiveModel = model.into();
        am.received_chunks = Set(updated_json);
        am.update(&txn)
            .await
            .map_err(|e| AppError::Database(e.to_string()))?;

        txn.commit()
            .await
            .map_err(|e| AppError::Database(e.to_string()))?;

        self.find(&session.id)
            .await?
            .ok_or_else(|| AppError::Internal("session vanished after chunk update".into()))
    }

    /// Mark a session as complete (called after VFS record is created).
    pub async fn mark_complete(&self, session_id: &str) -> AppResult<()> {
        self.set_status(session_id, STATUS_COMPLETE).await
    }

    /// Cancel a session and remove its temporary chunk files from disk.
    pub async fn cancel(&self, session_id: &str) -> AppResult<()> {
        self.set_status(session_id, STATUS_CANCELLED).await?;
        self.cleanup_tmp(session_id).await;
        Ok(())
    }

    // ── Garbage collection ────────────────────────────────────────────────────

    /// Delete all expired sessions and their temporary files.
    ///
    /// Returns the number of sessions purged.
    pub async fn purge_expired(&self) -> AppResult<u64> {
        let now = Utc::now();

        let expired: Vec<upload_sessions::Model> = Entity::find()
            .filter(Column::ExpiresAt.lt(now))
            .filter(Column::Status.eq(STATUS_PENDING))
            .all(&self.db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))?;

        let mut count = 0u64;
        for m in expired {
            let id = m.id.clone();
            self.cleanup_tmp(&id).await;
            Entity::delete_by_id(&id)
                .exec(&self.db)
                .await
                .map_err(|e| AppError::Database(e.to_string()))?;
            count += 1;
        }

        if count > 0 {
            debug!(purged = count, "upload session GC: purged expired sessions");
        }
        Ok(count)
    }

    /// Spawn a background GC task that runs every `interval` minutes.
    pub fn spawn_gc(self, interval_minutes: u64) {
        tokio::spawn(async move {
            let mut ticker =
                tokio::time::interval(std::time::Duration::from_secs(interval_minutes * 60));
            loop {
                ticker.tick().await;
                match self.purge_expired().await {
                    Ok(n) if n > 0 => debug!(purged = n, "upload session GC cycle"),
                    Err(e) => warn!(error = %e, "upload session GC error"),
                    _ => {}
                }
            }
        });
    }

    // ── Path helpers ──────────────────────────────────────────────────────────

    /// Path to the temp directory for a session's chunks.
    pub fn session_tmp_dir(&self, session_id: &str) -> PathBuf {
        self.tmp_dir.join(session_id)
    }

    /// Path to the file for chunk `index` of `session_id`.
    pub fn chunk_path(&self, session_id: &str, index: u32) -> PathBuf {
        self.session_tmp_dir(session_id)
            .join(format!("{index:06}"))
    }

    // ── Private helpers ───────────────────────────────────────────────────────

    async fn set_status(&self, session_id: &str, status: &str) -> AppResult<()> {
        let model = Entity::find_by_id(session_id)
            .one(&self.db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))?
            .ok_or_else(|| AppError::NotFound(format!("upload session: {session_id}")))?;

        let mut am: ActiveModel = model.into();
        am.status = Set(status.to_owned());
        am.update(&self.db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(())
    }

    /// Remove the temp directory for a session (best-effort, logs on error).
    async fn cleanup_tmp(&self, session_id: &str) {
        let dir = self.session_tmp_dir(session_id);
        if dir.exists() {
            if let Err(e) = tokio::fs::remove_dir_all(&dir).await {
                warn!(session_id, error = %e, "failed to remove upload tmp dir");
            }
        }
    }
}

/// Assemble all chunk files for a session in order into a single contiguous
/// byte vector.
///
/// This is called from the COMPLETE handler before handing bytes off to
/// [`jiezi_cloud_storage::UploadService`].
pub async fn assemble_chunks(
    session_tmp_dir: &Path,
    chunk_count: u32,
) -> AppResult<bytes::Bytes> {
    let mut buf = Vec::new();
    for index in 0..chunk_count {
        let path = session_tmp_dir.join(format!("{index:06}"));
        let data = tokio::fs::read(&path).await.map_err(|e| {
            AppError::Storage(format!("read chunk {index}: {e}"))
        })?;
        buf.extend_from_slice(&data);
    }
    Ok(bytes::Bytes::from(buf))
}
