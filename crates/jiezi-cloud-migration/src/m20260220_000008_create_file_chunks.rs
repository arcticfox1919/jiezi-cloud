//! Migration: create the `file_chunks` table.
//!
//! A file in the VFS may be stored as one or more variable-length chunks
//! produced by the FastCDC content-defined-chunking algorithm.  This table
//! maps each (file, sequence-number) pair to the SHA-256 hash of that chunk,
//! which in turn links into `chunk_locations` to find the physical object on
//! each backend.
//!
//! # Why CDC chunking?
//!
//! Content-defined chunking ensures that small edits near the beginning of a
//! large file only invalidate a handful of chunks rather than the entire file,
//! enabling efficient delta-sync and deduplication across versions.
//!
//! # Deduplication
//!
//! Two files that share a common chunk (same `chunk_hash`) automatically
//! share physical storage — the chunk is not duplicated on any backend.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(FileChunks::Table)
                    .if_not_exists()
                    // Composite PK: unique (file, position) pair.
                    .primary_key(
                        Index::create()
                            .col(FileChunks::FileId)
                            .col(FileChunks::Sequence),
                    )
                    // References `file_nodes.id` (no FK for DB portability;
                    // enforced at application layer).
                    .col(
                        ColumnDef::new(FileChunks::FileId)
                            .string()
                            .not_null(),
                    )
                    // Full SHA-256 hex digest (64 chars) of this chunk's content.
                    // Links to `chunk_locations.chunk_hash`.
                    .col(
                        ColumnDef::new(FileChunks::ChunkHash)
                            .string()
                            .not_null(),
                    )
                    // 0-based ordinal position of this chunk within the file.
                    .col(
                        ColumnDef::new(FileChunks::Sequence)
                            .integer()
                            .not_null(),
                    )
                    // Number of bytes in this chunk (needed for byte-range
                    // calculations without hitting the storage backend).
                    .col(
                        ColumnDef::new(FileChunks::SizeBytes)
                            .big_integer()
                            .not_null(),
                    )
                    .to_owned(),
            )
            .await?;

        // Reverse lookup: which files reference a given chunk_hash?
        // Used during GC to determine if a chunk is still in use.
        manager
            .create_index(
                Index::create()
                    .name("idx_file_chunks_chunk_hash")
                    .table(FileChunks::Table)
                    .col(FileChunks::ChunkHash)
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(FileChunks::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum FileChunks {
    Table,
    FileId,
    ChunkHash,
    Sequence,
    SizeBytes,
}
