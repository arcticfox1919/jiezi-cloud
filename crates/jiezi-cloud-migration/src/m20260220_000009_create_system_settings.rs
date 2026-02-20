//! Migration: create `system_settings` and seed the initial setup flag.
//!
//! `system_settings` is a flat key-value store for server-wide configuration
//! that does NOT belong in the config file (i.e. it changes at runtime through
//! the admin UI or the first-run setup wizard).
//!
//! # Schema
//!
//! | column       | type      | notes                          |
//! |--------------|-----------|--------------------------------|
//! | key          | TEXT PK   | well-known setting name        |
//! | value        | TEXT      | string-serialised value        |
//! | updated_at   | TIMESTAMP | last write time                |
//!
//! # Well-known keys (seeded by this migration)
//!
//! | key                   | initial value | description                   |
//! |-----------------------|---------------|-------------------------------|
//! | setup_completed       | false         | first-run wizard gate         |
//! | site_name             | Jiezi Cloud   | display name shown in UI      |
//! | site_description      | (empty)       | optional tagline              |
//! | registration_enabled  | false         | allow public self-registration|
//! | max_upload_size_mb    | 512           | per-file upload limit         |

use sea_orm::Statement;
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Create the table.
        manager
            .create_table(
                Table::create()
                    .table(SystemSettings::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(SystemSettings::Key)
                            .string()
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(SystemSettings::Value)
                            .text()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(SystemSettings::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .to_owned(),
            )
            .await?;

        // Seed initial rows.
        let db = manager.get_connection();
        let backend = manager.get_database_backend();

        for (key, value) in &[
            ("setup_completed",      "false"),
            ("site_name",            "Jiezi Cloud"),
            ("site_description",     ""),
            ("registration_enabled", "false"),
            ("max_upload_size_mb",   "512"),
        ] {
            db.execute(Statement::from_string(
                backend,
                format!(
                    "INSERT INTO system_settings (key, value, updated_at) \
                     VALUES ('{}', '{}', CURRENT_TIMESTAMP)",
                    key, value
                ),
            ))
            .await?;
        }

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(SystemSettings::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum SystemSettings {
    Table,
    Key,
    Value,
    UpdatedAt,
}
