//! Migration 010 — add brute-force lockout columns to `users`.
//!
//! Three columns are added to the existing `users` table:
//!
//! | Column                | Type      | Purpose                                         |
//! |-----------------------|-----------|-------------------------------------------------|
//! | `failed_login_count`  | INTEGER   | Consecutive failed attempts since last success  |
//! | `locked_until`        | TIMESTAMP | NULL = not locked; non-NULL = locked until this |
//! | `last_failed_login_at`| TIMESTAMP | Timestamp of the most recent failed attempt     |
//!
//! The service layer uses these to implement an exponential-threshold lockout:
//! after `N` consecutive failures the account is locked for a configurable
//! duration.  A successful login resets all three columns to their default
//! state so the lockout is not permanent.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Consecutive failed login counter — reset to 0 on success.
        manager
            .alter_table(
                Table::alter()
                    .table(Users::Table)
                    .add_column(
                        ColumnDef::new(Users::FailedLoginCount)
                            .integer()
                            .not_null()
                            .default(0),
                    )
                    .to_owned(),
            )
            .await?;

        // Until when the account is locked.  NULL means "not locked".
        manager
            .alter_table(
                Table::alter()
                    .table(Users::Table)
                    .add_column(
                        ColumnDef::new(Users::LockedUntil)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .to_owned(),
            )
            .await?;

        // Timestamp of the most recent failed attempt (for audit logs /
        // adaptive lockout windows).
        manager
            .alter_table(
                Table::alter()
                    .table(Users::Table)
                    .add_column(
                        ColumnDef::new(Users::LastFailedLoginAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        for col in &[
            Users::FailedLoginCount,
            Users::LockedUntil,
            Users::LastFailedLoginAt,
        ] {
            manager
                .alter_table(
                    Table::alter()
                        .table(Users::Table)
                        .drop_column(*col)
                        .to_owned(),
                )
                .await?;
        }
        Ok(())
    }
}

#[derive(Copy, Clone, DeriveIden)]
enum Users {
    Table,
    FailedLoginCount,
    LockedUntil,
    LastFailedLoginAt,
}
