//! Repository for the `download_tokens` table.
//!
//! Manages creation, consumption, and expiry of time-limited download tokens.
//!
//! # Token lifecycle
//!
//! 1. **Create**: authenticated user calls `POST /api/v1/download/{id}/token`;
//!    a random 32-char hex token is generated with a configurable TTL.
//! 2. **Consume**: the holder calls `GET /api/v1/download/t/{token}`;
//!    `validate_and_consume` checks expiry and one-time semantics.
//! 3. **GC**: `purge_expired` removes rows past their `expires_at`.

use chrono::Utc;
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter,
};
use tracing::{debug, warn};
use uuid::Uuid;

use jiezi_cloud_core::error::{AppError, AppResult};
use jiezi_cloud_core::types::{FileId, UserId};

use crate::entities::download_tokens::{self, ActiveModel, Column, Entity};

// ─── Domain type ──────────────────────────────────────────────────────────────

/// Default token TTL when the client does not specify one (1 hour).
pub const DEFAULT_TOKEN_TTL_SECS: u64 = 3600;

/// View of a download token row.
#[derive(Debug, Clone)]
pub struct DownloadToken {
    pub token: String,
    pub file_node_id: String,
    pub user_id: String,
    pub expires_at: chrono::DateTime<Utc>,
    pub one_time: bool,
    pub used: bool,
    pub created_at: chrono::DateTime<Utc>,
}

impl From<download_tokens::Model> for DownloadToken {
    fn from(m: download_tokens::Model) -> Self {
        Self {
            token: m.token,
            file_node_id: m.file_node_id,
            user_id: m.user_id,
            expires_at: m.expires_at,
            one_time: m.one_time,
            used: m.used,
            created_at: m.created_at,
        }
    }
}

// ─── Repository ───────────────────────────────────────────────────────────────

/// Repository for download tokens.
///
/// Cheap to clone: the inner [`DatabaseConnection`] is already `Arc`-backed.
#[derive(Clone)]
pub struct DownloadTokenRepository {
    db: DatabaseConnection,
}

impl DownloadTokenRepository {
    /// Create a new repository.
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }

    // ── Create ────────────────────────────────────────────────────────────────

    /// Issue a new download token for `file_id`.
    ///
    /// - `ttl_secs`: lifetime in seconds from now (capped at 7 days internally
    ///   to prevent unbounded tokens; callers may enforce stricter limits).
    /// - `one_time`: if `true`, the token is invalidated after one successful use.
    pub async fn create(
        &self,
        file_id: &FileId,
        user_id: &UserId,
        ttl_secs: u64,
        one_time: bool,
    ) -> AppResult<DownloadToken> {
        // Clamp to max 7 days to prevent indefinite tokens.
        let ttl_secs = ttl_secs.min(7 * 24 * 3600);
        let now = Utc::now();
        let expires_at = now + chrono::Duration::seconds(ttl_secs as i64);

        // 128-bit random token encoded as 32 lowercase hex chars.
        let token = Uuid::new_v4().to_string().replace('-', "");

        let am = ActiveModel {
            token: Set(token.clone()),
            file_node_id: Set(file_id.to_string()),
            user_id: Set(user_id.to_string()),
            expires_at: Set(expires_at),
            one_time: Set(one_time),
            used: Set(false),
            created_at: Set(now),
        };

        am.insert(&self.db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))?;

        self.find(&token)
            .await?
            .ok_or_else(|| AppError::Internal("token vanished after insert".into()))
    }

    // ── Validate ──────────────────────────────────────────────────────────────

    /// Validate a download token and return it.
    ///
    /// For one-time tokens this marks the token as `used = true` atomically
    /// so that a second call returns [`AppError::Gone`].
    ///
    /// # Errors
    ///
    /// - [`AppError::NotFound`] — token doesn't exist.
    /// - [`AppError::Gone`]    — token has expired or was already used.
    pub async fn validate_and_consume(&self, token: &str) -> AppResult<DownloadToken> {
        let model = Entity::find_by_id(token)
            .one(&self.db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))?
            .ok_or_else(|| AppError::NotFound("download token not found".into()))?;

        let now = Utc::now();

        if model.expires_at < now {
            return Err(AppError::Gone("download token has expired".into()));
        }

        if model.used {
            return Err(AppError::Gone("download token has already been used".into()));
        }

        // Mark as used for one-time tokens.
        if model.one_time {
            let mut am: ActiveModel = model.clone().into();
            am.used = Set(true);
            am.update(&self.db)
                .await
                .map_err(|e| AppError::Database(e.to_string()))?;
        }

        Ok(DownloadToken::from(model))
    }

    // ── Garbage collection ────────────────────────────────────────────────────

    /// Delete all expired tokens.  Returns the number of rows removed.
    pub async fn purge_expired(&self) -> AppResult<u64> {
        let now = Utc::now();
        let result = Entity::delete_many()
            .filter(Column::ExpiresAt.lt(now))
            .exec(&self.db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))?;

        let count = result.rows_affected;
        if count > 0 {
            debug!(purged = count, "download token GC: purged expired tokens");
        }
        Ok(count)
    }

    /// Spawn a background GC task that runs every `interval_minutes` minutes.
    pub fn spawn_gc(self, interval_minutes: u64) {
        tokio::spawn(async move {
            let mut ticker =
                tokio::time::interval(std::time::Duration::from_secs(interval_minutes * 60));
            loop {
                ticker.tick().await;
                match self.purge_expired().await {
                    Ok(n) if n > 0 => debug!(purged = n, "download token GC cycle"),
                    Err(e) => warn!(error = %e, "download token GC error"),
                    _ => {}
                }
            }
        });
    }

    // ── Private helpers ───────────────────────────────────────────────────────

    async fn find(&self, token: &str) -> AppResult<Option<DownloadToken>> {
        let model = Entity::find_by_id(token)
            .one(&self.db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(model.map(DownloadToken::from))
    }
}
