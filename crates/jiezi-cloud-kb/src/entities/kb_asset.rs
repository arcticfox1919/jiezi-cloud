//! SeaORM entity for the `kb_assets` table.
//!
//! Tracks every `jiezi://asset/<file_id>` URI that appears in a KB note's
//! Markdown source.  The composite primary key `(note_id, file_id)` prevents
//! duplicates.  Rows are cascade-deleted when the owning note is removed, so
//! a background patrol can identify file IDs that are no longer referenced by
//! any note and schedule the underlying storage objects for removal.

use sea_orm::entity::prelude::*;

/// Row model for `kb_assets` (composite PK: `note_id + file_id`).
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "kb_assets")]
pub struct Model {
    /// FK → `kb_notes.id`, first part of the composite primary key.
    #[sea_orm(primary_key, auto_increment = false, column_type = "Text")]
    pub note_id: String,

    /// The VFS file ID of the embedded asset (the value after `jiezi://asset/`).
    #[sea_orm(primary_key, auto_increment = false, column_type = "Text")]
    pub file_id: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    /// Each asset row belongs to exactly one KB note.
    #[sea_orm(
        belongs_to = "super::kb_note::Entity",
        from = "Column::NoteId",
        to = "super::kb_note::Column::Id"
    )]
    Note,
}

impl Related<super::kb_note::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Note.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
