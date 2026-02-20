//! Migration: create the `storage_backend_configs` table.
//!
//! Each row describes one user-configured storage backend (local directory,
//! S3 bucket, WebDAV server, …).  The actual connection parameters are stored
//! as JSON in `backend_type_json` so that adding new backend types never
//! requires a schema change.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(StorageBackendConfigs::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(StorageBackendConfigs::Id)
                            .string()
                            .not_null()
                            .primary_key(),
                    )
                    // NULL = system-wide / admin backend (not owned by a user).
                    .col(ColumnDef::new(StorageBackendConfigs::OwnerId).string().null())
                    .col(
                        ColumnDef::new(StorageBackendConfigs::DisplayName)
                            .string()
                            .not_null(),
                    )
                    // JSON-encoded BackendType (internally-tagged, see models/backend.rs).
                    .col(
                        ColumnDef::new(StorageBackendConfigs::BackendTypeJson)
                            .text()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(StorageBackendConfigs::IsEnabled)
                            .boolean()
                            .not_null()
                            .default(true),
                    )
                    // Lower value = higher priority in read-path fallback order.
                    .col(
                        ColumnDef::new(StorageBackendConfigs::Priority)
                            .integer()
                            .not_null()
                            .default(0),
                    )
                    .col(
                        ColumnDef::new(StorageBackendConfigs::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(StorageBackendConfigs::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .to_owned(),
            )
            .await?;

        // List all backends owned by a specific user (dashboard / settings page).
        manager
            .create_index(
                Index::create()
                    .name("idx_storage_backend_configs_owner_id")
                    .table(StorageBackendConfigs::Table)
                    .col(StorageBackendConfigs::OwnerId)
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(
                Table::drop()
                    .table(StorageBackendConfigs::Table)
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum StorageBackendConfigs {
    Table,
    Id,
    OwnerId,
    DisplayName,
    BackendTypeJson,
    IsEnabled,
    Priority,
    CreatedAt,
    UpdatedAt,
}
