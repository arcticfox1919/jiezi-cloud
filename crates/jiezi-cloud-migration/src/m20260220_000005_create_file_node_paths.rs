//! Migration: create the `file_node_paths` closure table.
//!
//! A **closure table** stores every (ancestor, descendant, depth) triple,
//! including the self-reference row (node, node, 0).  This design enables:
//!
//! - O(1) "is X an ancestor/descendant of Y?" checks
//! - O(k) subtree/path retrieval where k is the result size
//! - Efficient tree restructuring on move operations
//!
//! # Trade-off
//!
//! Write amplification: inserting a node at depth d requires d+1 new rows.
//! For typical file system depths (< 20 levels) this is negligible.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(FileNodePaths::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(FileNodePaths::AncestorId)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(FileNodePaths::DescendantId)
                            .string()
                            .not_null(),
                    )
                    // 0 = self-reference; 1 = direct child; etc.
                    .col(
                        ColumnDef::new(FileNodePaths::Depth)
                            .integer()
                            .not_null(),
                    )
                    .primary_key(
                        Index::create()
                            .col(FileNodePaths::AncestorId)
                            .col(FileNodePaths::DescendantId),
                    )
                    .to_owned(),
            )
            .await?;

        // Fast "all ancestors of node X" queries.
        manager
            .create_index(
                Index::create()
                    .name("idx_file_node_paths_descendant")
                    .table(FileNodePaths::Table)
                    .col(FileNodePaths::DescendantId)
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(FileNodePaths::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum FileNodePaths {
    Table,
    AncestorId,
    DescendantId,
    Depth,
}
