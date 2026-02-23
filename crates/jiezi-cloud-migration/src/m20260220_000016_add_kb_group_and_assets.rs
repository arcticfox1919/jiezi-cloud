//! Migration: add `group_name` to `kb_notes` and create the `kb_assets` table.
//!
//! # `group_name` column on `kb_notes`
//!
//! Stores the name of the VFS sub-directory (the "group" folder) that directly
//! contains the indexed Markdown file.  Nil for notes that sit outside the
//! standard `.knowledge-base/{group}/` layout.  Enables efficient SQL
//! filtering in the `list` API without a VFS join.
//!
//! # `kb_assets`
//!
//! Records every `jiezi://asset/<file_id>` URI that appears in a note's
//! Markdown source.  This composite table is the basis for orphaned-asset
//! garbage collection: when a note is deleted the database cascades the
//! deletion; a background patrol can then find file IDs that are no longer
//! referenced by any note and remove the underlying storage objects.

use sea_orm_migration::prelude::*;

use super::m20260220_000014_create_kb_notes::KbNotes;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // 1. Add nullable group_name column to kb_notes.
        manager
            .alter_table(
                Table::alter()
                    .table(KbNotes::Table)
                    .add_column(ColumnDef::new(KbNotesExt::GroupName).text().null())
                    .to_owned(),
            )
            .await?;

        // 2. Create kb_assets tracking table.
        manager
            .create_table(
                Table::create()
                    .table(KbAssets::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(KbAssets::NoteId).text().not_null())
                    .col(ColumnDef::new(KbAssets::FileId).text().not_null())
                    .primary_key(
                        Index::create()
                            .col(KbAssets::NoteId)
                            .col(KbAssets::FileId),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_kb_assets_note_id")
                            .from(KbAssets::Table, KbAssets::NoteId)
                            .to(KbNotes::Table, KbNotes::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        // 3. Reverse-lookup index: "which notes reference this file?" used by GC.
        manager
            .create_index(
                Index::create()
                    .name("idx_kb_assets_file_id")
                    .table(KbAssets::Table)
                    .col(KbAssets::FileId)
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(KbAssets::Table).to_owned())
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(KbNotes::Table)
                    .drop_column(KbNotesExt::GroupName)
                    .to_owned(),
            )
            .await
    }
}

/// Extension iden for the new column added to `kb_notes`.
///
/// Kept separate from [`KbNotes`] (imported from migration 000014) to avoid
/// re-declaring an existing identifier.
#[derive(DeriveIden)]
enum KbNotesExt {
    GroupName,
}

/// Column identifiers for the new `kb_assets` table.
#[derive(DeriveIden)]
enum KbAssets {
    Table,
    NoteId,
    FileId,
}
