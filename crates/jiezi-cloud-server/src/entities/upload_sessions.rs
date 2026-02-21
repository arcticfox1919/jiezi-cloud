//! SeaORM entity for the `upload_sessions` table.
//!
//! Each row represents one in-progress resumable upload session.
//! Raw chunk bytes are stored on the filesystem under
//! `{storage.local_root}/.upload_tmp/{id}/{index:06}`;
//! only session metadata lives in the database.

use sea_orm::entity::prelude::*;

/// Session status values stored in the `status` column.
pub const STATUS_PENDING: &str = "pending";
pub const STATUS_COMPLETE: &str = "complete";
pub const STATUS_CANCELLED: &str = "cancelled";

/// Row model — maps 1-to-1 onto columns in the `upload_sessions` table.
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "upload_sessions")]
pub struct Model {
    /// UUIDv4 string — the session identifier.
    #[sea_orm(primary_key, auto_increment = false, column_type = "Text")]
    pub id: String,

    /// Foreign key → `users.id`.
    #[sea_orm(column_type = "Text")]
    pub user_id: String,

    /// VFS parent directory that will hold the assembled file.
    #[sea_orm(column_type = "Text")]
    pub parent_id: String,

    /// Filename to record in the VFS when the upload completes.
    #[sea_orm(column_type = "Text")]
    pub file_name: String,

    /// Total file size in bytes (stored as i64 for cross-backend compat.).
    pub total_size: i64,

    /// SHA-256 hex digest of the complete file — used for dedup and integrity.
    #[sea_orm(column_type = "Text")]
    pub content_hash: String,

    /// Optional MIME type hint provided by the client.
    #[sea_orm(nullable)]
    pub mime_type: Option<String>,

    /// Negotiated HTTP-chunk size in bytes (typically 8 MiB).
    pub chunk_size: i64,

    /// Total number of HTTP chunks expected (`ceil(total_size / chunk_size)`).
    pub chunk_count: i32,

    /// JSON array of received chunk indices, e.g. `[0,1,3]`.
    ///
    /// Stored as TEXT to remain portable across SQLite / PostgreSQL / MySQL.
    /// The repository layer serialises/deserialises this field.
    #[sea_orm(column_type = "Text")]
    pub received_chunks: String,

    /// Session lifecycle: `"pending"` | `"complete"` | `"cancelled"`.
    #[sea_orm(column_type = "Text")]
    pub status: String,

    pub created_at: DateTimeUtc,

    /// Hard deadline — sessions at or past this time are eligible for GC.
    pub expires_at: DateTimeUtc,
}

/// No active SeaORM relations needed — FK constraints are handled by the DB.
#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
