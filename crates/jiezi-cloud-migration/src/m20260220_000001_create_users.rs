//! Migration: create the `users` table.
//!
//! This Rust-based migration runs identically on SQLite, PostgreSQL and MySQL —
//! SeaORM's schema manager generates the appropriate DDL for each backend.

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
                    .table(Users::Table)
                    .if_not_exists()
                    // Primary key: UUIDv7 as text
                    .col(ColumnDef::new(Users::Id).string().not_null().primary_key())
                    .col(ColumnDef::new(Users::Username).string().not_null().unique_key())
                    .col(ColumnDef::new(Users::Email).string().not_null().unique_key())
                    .col(ColumnDef::new(Users::PasswordHash).string().not_null())
                    // 'owner' | 'admin' | 'member' | 'guest'
                    .col(ColumnDef::new(Users::Role).string().not_null().default("member"))
                    .col(ColumnDef::new(Users::IsActive).boolean().not_null().default(true))
                    .col(ColumnDef::new(Users::DisplayName).string().null())
                    .col(ColumnDef::new(Users::AvatarUrl).string().null())
                    // NULL means unlimited quota
                    .col(ColumnDef::new(Users::StorageQuota).big_integer().null())
                    .col(ColumnDef::new(Users::StorageUsed).big_integer().not_null().default(0))
                    .col(ColumnDef::new(Users::CreatedAt).timestamp_with_time_zone().not_null())
                    .col(ColumnDef::new(Users::UpdatedAt).timestamp_with_time_zone().not_null())
                    .to_owned(),
            )
            .await?;

        // Index on email (fast lookup by email credential)
        manager
            .create_index(
                Index::create()
                    .name("idx_users_email")
                    .table(Users::Table)
                    .col(Users::Email)
                    .to_owned(),
            )
            .await?;

        // Index on username (fast lookup by username credential)
        manager
            .create_index(
                Index::create()
                    .name("idx_users_username")
                    .table(Users::Table)
                    .col(Users::Username)
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(Users::Table).to_owned())
            .await
    }
}

/// Column identifiers — used by both this migration and the next one (FK).
#[derive(DeriveIden)]
pub enum Users {
    Table,
    Id,
    Username,
    Email,
    PasswordHash,
    Role,
    IsActive,
    DisplayName,
    AvatarUrl,
    StorageQuota,
    StorageUsed,
    CreatedAt,
    UpdatedAt,
}
