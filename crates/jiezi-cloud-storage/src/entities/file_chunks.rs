//! SeaORM entity for the `file_chunks` table.
//!
//! Maps each (file, sequence-position) to its content-addressed chunk hash.
//! Composite primary key: `(file_id, sequence)`.

use sea_orm::entity::prelude::*;

/// Row model — maps 1-to-1 onto columns in the `file_chunks` table.
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq)]
#[sea_orm(table_name = "file_chunks")]
pub struct Model {
    /// The file node this chunk belongs to (references `file_nodes.id`).
    #[sea_orm(primary_key, auto_increment = false, column_type = "Text")]
    pub file_id: String,

    /// 0-based ordinal position of this chunk within the file.
    #[sea_orm(primary_key, auto_increment = false)]
    pub sequence: i32,

    /// SHA-256 hex digest of this chunk's content.
    /// Links to `chunk_locations.chunk_hash`.
    #[sea_orm(column_type = "Text")]
    pub chunk_hash: String,

    /// Number of bytes in this chunk (used for byte-range calculations
    /// without hitting the storage backend).
    pub size_bytes: i64,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
