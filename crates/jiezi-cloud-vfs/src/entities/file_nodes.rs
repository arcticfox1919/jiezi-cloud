//! SeaORM entity for the `file_nodes` table.
//!
//! Only the repository layer and entity tests should reference these types
//! directly.  All other code works with [`jiezi_cloud_core::models::file::FileNode`].

use sea_orm::entity::prelude::*;

/// Row model — maps 1-to-1 onto columns in the `file_nodes` table.
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq)]
#[sea_orm(table_name = "file_nodes")]
pub struct Model {
    /// UUIDv7 serialised as a hyphenated string — primary key.
    #[sea_orm(primary_key, auto_increment = false, column_type = "Text")]
    pub id: String,

    /// Parent directory UUID; NULL only for space-root nodes.
    #[sea_orm(nullable)]
    pub parent_id: Option<String>,

    /// The owning space UUID.
    #[sea_orm(column_type = "Text")]
    pub space_id: String,

    /// The user who created or owns this node.
    #[sea_orm(column_type = "Text")]
    pub owner_id: String,

    /// File or directory name (just the final component, not the full path).
    #[sea_orm(column_type = "Text")]
    pub name: String,

    /// Type discriminant: `"file"` | `"directory"` | `"symlink"`.
    #[sea_orm(column_type = "Text")]
    pub node_type: String,

    /// File size in bytes; 0 for directories.
    pub size: i64,

    /// Detected MIME type; NULL for directories.
    #[sea_orm(nullable)]
    pub mime_type: Option<String>,

    /// SHA-256 hex digest of the complete file content; NULL for directories.
    #[sea_orm(nullable)]
    pub content_hash: Option<String>,

    /// JSON-encoded [`jiezi_cloud_core::models::file::FileMetadata`].
    #[sea_orm(column_type = "Text")]
    pub metadata_json: String,

    pub created_at: DateTimeUtc,
    pub updated_at: DateTimeUtc,

    /// Non-null when the node has been soft-deleted (moved to trash).
    #[sea_orm(nullable)]
    pub deleted_at: Option<DateTimeUtc>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    /// The space that contains this node.
    #[sea_orm(
        belongs_to = "super::spaces::Entity",
        from = "Column::SpaceId",
        to = "super::spaces::Column::Id"
    )]
    Space,
}

impl Related<super::spaces::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Space.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
