//! Migration: create the `spaces` table.
//!
//! A space is a logical storage container owned by a user.  Every user has at
//! least one personal space; additional collaborative spaces can be created
//! and shared with other members.

use sea_orm_migration::prelude::*;

/// Migration struct — the name doubles as the recorded identifier in the
/// `seaql_migrations` tracking table.
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(Spaces::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(Spaces::Id).string().not_null().primary_key())
                    .col(ColumnDef::new(Spaces::OwnerId).string().not_null())
                    .col(ColumnDef::new(Spaces::Name).string().not_null())
                    .col(ColumnDef::new(Spaces::Description).string().null())
                    // NULL = unlimited
                    .col(ColumnDef::new(Spaces::StorageQuota).big_integer().null())
                    .col(
                        ColumnDef::new(Spaces::StorageUsed)
                            .big_integer()
                            .not_null()
                            .default(0),
                    )
                    .col(
                        ColumnDef::new(Spaces::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(Spaces::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .to_owned(),
            )
            .await?;

        // Index: list all spaces belonging to an owner.
        manager
            .create_index(
                Index::create()
                    .name("idx_spaces_owner_id")
                    .table(Spaces::Table)
                    .col(Spaces::OwnerId)
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(Spaces::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum Spaces {
    Table,
    Id,
    OwnerId,
    Name,
    Description,
    StorageQuota,
    StorageUsed,
    CreatedAt,
    UpdatedAt,
}
