//! Migration: create the `chunk_locations` table.
//!
//! Tracks where each content-addressed chunk is stored.  Because the storage
//! layer is content-addressed a single chunk (identified by its SHA-256 hash)
//! can exist on multiple backends simultaneously — that is the whole point of
//! multi-backend replication.  This table is the authoritative record of which
//! backends hold a given chunk, enabling:
//!
//! - Read-path routing: look up which backends have the chunk and try them in
//!   priority order.
//! - Replication verification: compare the set of `backend_id` rows for a
//!   `chunk_hash` against the replication policy to detect under-replication.
//! - Garbage collection: find chunks with a single backend row after a backend
//!   is disabled, and re-replicate before deleting.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(ChunkLocations::Table)
                    .if_not_exists()
                    // Composite PK: one row per (chunk, backend) pair.
                    .primary_key(
                        Index::create()
                            .col(ChunkLocations::ChunkHash)
                            .col(ChunkLocations::BackendId),
                    )
                    // Full SHA-256 hex digest (64 chars) of the chunk content.
                    .col(
                        ColumnDef::new(ChunkLocations::ChunkHash)
                            .string()
                            .not_null(),
                    )
                    // References `storage_backend_configs.id` (no FK constraint
                    // for DB portability; enforced at application layer).
                    .col(
                        ColumnDef::new(ChunkLocations::BackendId)
                            .string()
                            .not_null(),
                    )
                    // The backend-specific object key (e.g. `"ab/abcdef…"` for
                    // local/S3 or the full WebDAV path segment).
                    .col(
                        ColumnDef::new(ChunkLocations::StorageKey)
                            .string()
                            .not_null(),
                    )
                    // Size of the stored chunk in bytes.
                    .col(
                        ColumnDef::new(ChunkLocations::SizeBytes)
                            .big_integer()
                            .not_null(),
                    )
                    // Last time this copy was successfully read back and its
                    // hash verified.  NULL = never verified since upload.
                    .col(
                        ColumnDef::new(ChunkLocations::VerifiedAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .to_owned(),
            )
            .await?;

        // Find all chunks stored on a specific backend (used by GC and rebalancing).
        manager
            .create_index(
                Index::create()
                    .name("idx_chunk_locations_backend_id")
                    .table(ChunkLocations::Table)
                    .col(ChunkLocations::BackendId)
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(ChunkLocations::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum ChunkLocations {
    Table,
    ChunkHash,
    BackendId,
    StorageKey,
    SizeBytes,
    VerifiedAt,
}
