//! Migration: create the `file_nodes` table.
//!
//! This table stores every file and directory node in the virtual file system.
//! Physical bytes are managed separately by the storage backend; this table
//! holds only node metadata.
//!
//! # Key design decisions
//!
//! - **Soft-delete** — `deleted_at IS NULL` means live; non-null means trashed.
//! - **Tree structure** — the parent-child relationship is captured here via
//!   `parent_id`; efficient subtree queries are handled separately by the
//!   `file_node_paths` closure table.
//! - **Content dedup** — `content_hash` (SHA-256 hex) is the deduplication key
//!   shared with the storage backend; `NULL` for directories.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(FileNodes::Table)
                    .if_not_exists()
                    // Primary key: UUIDv7 as text.
                    .col(
                        ColumnDef::new(FileNodes::Id)
                            .string()
                            .not_null()
                            .primary_key(),
                    )
                    // NULL only for space-root nodes.
                    .col(ColumnDef::new(FileNodes::ParentId).string().null())
                    .col(ColumnDef::new(FileNodes::SpaceId).string().not_null())
                    .col(ColumnDef::new(FileNodes::OwnerId).string().not_null())
                    .col(ColumnDef::new(FileNodes::Name).string().not_null())
                    // Discriminant: 'file' | 'directory' | 'symlink'
                    .col(ColumnDef::new(FileNodes::NodeType).string().not_null())
                    .col(
                        ColumnDef::new(FileNodes::Size)
                            .big_integer()
                            .not_null()
                            .default(0),
                    )
                    .col(ColumnDef::new(FileNodes::MimeType).string().null())
                    // SHA-256 hex; NULL for directories.
                    .col(ColumnDef::new(FileNodes::ContentHash).string().null())
                    // JSON-encoded FileMetadata (tags, extra, is_indexed, …).
                    .col(
                        ColumnDef::new(FileNodes::MetadataJson)
                            .text()
                            .not_null()
                            .default("{}"),
                    )
                    .col(
                        ColumnDef::new(FileNodes::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(FileNodes::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    // NULL = live; non-null = soft-deleted / trashed.
                    .col(
                        ColumnDef::new(FileNodes::DeletedAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .to_owned(),
            )
            .await?;

        // Fast child listings: WHERE parent_id = ? AND deleted_at IS NULL ORDER BY name
        manager
            .create_index(
                Index::create()
                    .name("idx_file_nodes_parent_id")
                    .table(FileNodes::Table)
                    .col(FileNodes::ParentId)
                    .to_owned(),
            )
            .await?;

        // Fast space-level queries: WHERE space_id = ?
        manager
            .create_index(
                Index::create()
                    .name("idx_file_nodes_space_id")
                    .table(FileNodes::Table)
                    .col(FileNodes::SpaceId)
                    .to_owned(),
            )
            .await?;

        // Dedup lookup: WHERE content_hash = ? (instant-upload check)
        manager
            .create_index(
                Index::create()
                    .name("idx_file_nodes_content_hash")
                    .table(FileNodes::Table)
                    .col(FileNodes::ContentHash)
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(FileNodes::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
pub enum FileNodes {
    Table,
    Id,
    ParentId,
    SpaceId,
    OwnerId,
    Name,
    NodeType,
    Size,
    MimeType,
    ContentHash,
    MetadataJson,
    CreatedAt,
    UpdatedAt,
    DeletedAt,
}
