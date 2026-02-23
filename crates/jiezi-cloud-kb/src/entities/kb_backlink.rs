//! SeaORM entity for the `kb_backlinks` table.
//!
//! Stores unresolved `[[WikiLink]]` targets as raw title strings.
//! Resolution to a `KbNoteId` is done lazily at query time by joining
//! with `kb_notes` + `file_nodes` via the file name.

use sea_orm::entity::prelude::*;

/// Row model for `kb_backlinks`.
///
/// Represents a directed edge: the note `from_note_id` contains a
/// `[[target_title]]` WikiLink.
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "kb_backlinks")]
pub struct Model {
    /// The note that contains the `[[WikiLink]]`.
    #[sea_orm(primary_key, auto_increment = false, column_type = "Text")]
    pub from_note_id: String,

    /// The raw title written inside `[[…]]`, e.g. `"Rust ownership"`.
    /// This is matched against VFS file names (`target_title + ".md"`) at
    /// query time to resolve the target note.
    #[sea_orm(primary_key, auto_increment = false, column_type = "Text")]
    pub target_title: String,

    /// Display alias: from `[[Target|Alias]]`.  Equals `target_title` when
    /// no alias is present.
    #[sea_orm(column_type = "Text")]
    pub anchor_text: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::kb_note::Entity",
        from = "Column::FromNoteId",
        to = "super::kb_note::Column::Id",
        on_update = "NoAction",
        on_delete = "Cascade"
    )]
    Note,
}

impl Related<super::kb_note::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Note.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
