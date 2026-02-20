//! SeaORM entity for the `chunk_locations` table.
//!
//! Tracks which backend(s) hold each content-addressed chunk.
//! Composite primary key: `(chunk_hash, backend_id)`.

use sea_orm::entity::prelude::*;

/// Row model — maps 1-to-1 onto columns in the `chunk_locations` table.
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq)]
#[sea_orm(table_name = "chunk_locations")]
pub struct Model {
    /// SHA-256 hex digest of the chunk content (dedup key).
    #[sea_orm(primary_key, auto_increment = false, column_type = "Text")]
    pub chunk_hash: String,

    /// The backend that holds this copy of the chunk.
    #[sea_orm(primary_key, auto_increment = false, column_type = "Text")]
    pub backend_id: String,

    /// Backend-specific storage key (e.g. `"ab/abcdef…"` for local/S3).
    #[sea_orm(column_type = "Text")]
    pub storage_key: String,

    /// Size of the stored chunk in bytes.
    pub size_bytes: i64,

    /// Last time this copy was successfully read back and its hash verified.
    /// `NULL` means never verified since upload.
    pub verified_at: Option<DateTimeWithTimeZone>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
