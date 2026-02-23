//! SeaORM entity for the `kb_notes` table.

use sea_orm::entity::prelude::*;

/// Row model for `kb_notes`.
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "kb_notes")]
pub struct Model {
    /// UUIDv7 — KB-layer primary key.
    #[sea_orm(primary_key, auto_increment = false, column_type = "Text")]
    pub id: String,

    /// FK → `file_nodes.id`.  `UNIQUE`: one file = one KB note at most.
    #[sea_orm(unique, column_type = "Text")]
    pub file_node_id: String,

    /// FK → `spaces.id`.
    #[sea_orm(column_type = "Text")]
    pub space_id: String,

    pub indexed_at: DateTimeUtc,

    /// URL-friendly slug for blog publishing.  `NULL` when not published.
    #[sea_orm(nullable)]
    pub slug: Option<String>,

    /// Timestamp of initial publication.  `NULL` = private/draft.
    #[sea_orm(nullable)]
    pub published_at: Option<DateTimeUtc>,

    /// Name of the VFS sub-directory ("group") that directly contains this
    /// note.  `NULL` for notes outside the `.knowledge-base/{group}/` layout.
    #[sea_orm(nullable)]
    pub group_name: Option<String>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(has_many = "super::kb_asset::Entity")]
    Assets,

    #[sea_orm(has_many = "super::kb_backlink::Entity")]
    Backlinks,

    #[sea_orm(has_many = "super::kb_tag::Entity")]
    Tags,
}

impl Related<super::kb_asset::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Assets.def()
    }
}

impl Related<super::kb_backlink::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Backlinks.def()
    }
}

impl Related<super::kb_tag::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Tags.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
