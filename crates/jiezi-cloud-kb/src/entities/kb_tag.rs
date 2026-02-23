//! SeaORM entity for the `kb_tags` table.

use sea_orm::entity::prelude::*;

/// Row model for `kb_tags`.
///
/// Many-to-one relation: `kb_tags.note_id` → `kb_notes.id`.
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "kb_tags")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false, column_type = "Text")]
    pub note_id: String,

    #[sea_orm(primary_key, auto_increment = false, column_type = "Text")]
    pub tag: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::kb_note::Entity",
        from = "Column::NoteId",
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
