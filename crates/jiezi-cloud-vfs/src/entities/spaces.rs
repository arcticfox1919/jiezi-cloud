//! SeaORM entity for the `spaces` table.

use sea_orm::entity::prelude::*;

/// Row model — maps 1-to-1 onto columns in the `spaces` table.
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq)]
#[sea_orm(table_name = "spaces")]
pub struct Model {
    /// UUIDv7 serialised as a hyphenated string — primary key.
    #[sea_orm(primary_key, auto_increment = false, column_type = "Text")]
    pub id: String,

    /// Owner's user UUID (references `users.id`).
    #[sea_orm(column_type = "Text")]
    pub owner_id: String,

    #[sea_orm(column_type = "Text")]
    pub name: String,

    #[sea_orm(nullable)]
    pub description: Option<String>,

    /// Maximum bytes this space may consume; NULL = unlimited.
    #[sea_orm(nullable)]
    pub storage_quota: Option<i64>,

    /// Running total of bytes consumed by live files in this space.
    pub storage_used: i64,

    pub created_at: DateTimeUtc,
    pub updated_at: DateTimeUtc,
}

/// Outbound relation: one space owns many file nodes.
#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(has_many = "super::file_nodes::Entity")]
    FileNodes,
}

impl Related<super::file_nodes::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::FileNodes.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
