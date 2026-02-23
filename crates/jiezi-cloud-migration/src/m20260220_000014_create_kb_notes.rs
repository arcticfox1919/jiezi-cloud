//! Migration: create the `kb_notes` table.
//!
//! Each row links a VFS `file_nodes` entry to KB-layer metadata.  The
//! `file_node_id` column carries a `UNIQUE` constraint — one file can be
//! registered as a KB note at most once.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(KbNotes::Table)
                    .if_not_exists()
                    // KB-layer primary key (UUIDv7 as text).
                    .col(
                        ColumnDef::new(KbNotes::Id)
                            .string()
                            .not_null()
                            .primary_key(),
                    )
                    // FK to file_nodes.id — UNIQUE: one file = one KB note.
                    .col(
                        ColumnDef::new(KbNotes::FileNodeId)
                            .string()
                            .not_null()
                            .unique_key(),
                    )
                    // Propagated from the file's space.
                    .col(ColumnDef::new(KbNotes::SpaceId).string().not_null())
                    // Timestamp of last indexing run.
                    .col(
                        ColumnDef::new(KbNotes::IndexedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    // Blog-publishing fields — NULL when not published.
                    .col(ColumnDef::new(KbNotes::Slug).string().null().unique_key())
                    .col(
                        ColumnDef::new(KbNotes::PublishedAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .to_owned(),
            )
            .await?;

        // Fast lookup by space for listing all notes in a space.
        manager
            .create_index(
                Index::create()
                    .name("idx_kb_notes_space_id")
                    .table(KbNotes::Table)
                    .col(KbNotes::SpaceId)
                    .to_owned(),
            )
            .await?;

        // Fast lookup by slug for blog-post URL resolution.
        manager
            .create_index(
                Index::create()
                    .name("idx_kb_notes_slug")
                    .table(KbNotes::Table)
                    .col(KbNotes::Slug)
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(KbNotes::Table).to_owned())
            .await
    }
}

/// Column identifiers — shared with downstream migrations.
#[derive(DeriveIden)]
pub enum KbNotes {
    Table,
    Id,
    FileNodeId,
    SpaceId,
    IndexedAt,
    Slug,
    PublishedAt,
}
