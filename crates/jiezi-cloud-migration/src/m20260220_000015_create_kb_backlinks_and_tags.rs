//! Migration: create the `kb_backlinks` and `kb_tags` tables.
//!
//! # `kb_backlinks`
//!
//! Stores directed `[[WikiLink]]` edges.  The `target_title` column holds the
//! **raw text** from the WikiLink (e.g. `"Rust ownership"`), unresolved.
//! Resolution to a concrete note ID is done lazily at query time by matching
//! `target_title` against VFS file names — this avoids write-time failures when
//! the target note hasn't been indexed yet and enables retroactive link
//! resolution.
//!
//! # `kb_tags`
//!
//! Many-to-one tags for KB notes.  Distinct from `file_metadata.tags` (raw
//! file metadata) — these are knowledge-layer semantic tags managed through
//! the KB API.

use sea_orm_migration::prelude::*;

use super::m20260220_000014_create_kb_notes::KbNotes;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // ── kb_backlinks ──────────────────────────────────────────────────────
        manager
            .create_table(
                Table::create()
                    .table(KbBacklinks::Table)
                    .if_not_exists()
                    // Composite PK: (from_note_id, target_title)
                    .col(
                        ColumnDef::new(KbBacklinks::FromNoteId)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(KbBacklinks::TargetTitle)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(KbBacklinks::AnchorText)
                            .string()
                            .not_null(),
                    )
                    .primary_key(
                        Index::create()
                            .col(KbBacklinks::FromNoteId)
                            .col(KbBacklinks::TargetTitle),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_kb_backlinks_from_note_id")
                            .from(KbBacklinks::Table, KbBacklinks::FromNoteId)
                            .to(KbNotes::Table, KbNotes::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        // Index for the "who links to title X?" query.
        manager
            .create_index(
                Index::create()
                    .name("idx_kb_backlinks_target_title")
                    .table(KbBacklinks::Table)
                    .col(KbBacklinks::TargetTitle)
                    .to_owned(),
            )
            .await?;

        // ── kb_tags ───────────────────────────────────────────────────────────
        manager
            .create_table(
                Table::create()
                    .table(KbTags::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(KbTags::NoteId).string().not_null())
                    .col(ColumnDef::new(KbTags::Tag).string().not_null())
                    .primary_key(
                        Index::create()
                            .col(KbTags::NoteId)
                            .col(KbTags::Tag),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_kb_tags_note_id")
                            .from(KbTags::Table, KbTags::NoteId)
                            .to(KbNotes::Table, KbNotes::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        // Fast tag filter: "give me all notes with tag = X".
        manager
            .create_index(
                Index::create()
                    .name("idx_kb_tags_tag")
                    .table(KbTags::Table)
                    .col(KbTags::Tag)
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(KbTags::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(KbBacklinks::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum KbBacklinks {
    Table,
    FromNoteId,
    TargetTitle,
    AnchorText,
}

#[derive(DeriveIden)]
enum KbTags {
    Table,
    NoteId,
    Tag,
}
