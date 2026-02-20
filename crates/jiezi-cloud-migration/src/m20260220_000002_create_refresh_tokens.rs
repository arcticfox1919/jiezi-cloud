//! Migration: create the `refresh_tokens` table.
//!
//! See `repository.rs` for the token-hash strategy and rotation semantics.

use sea_orm_migration::prelude::*;

use super::m20260220_000001_create_users::Users;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(RefreshTokens::Table)
                    .if_not_exists()
                    // SHA-256 hex digest of the raw JWT — primary key
                    .col(
                        ColumnDef::new(RefreshTokens::TokenHash)
                            .string()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(RefreshTokens::UserId).string().not_null())
                    .col(ColumnDef::new(RefreshTokens::Family).string().not_null())
                    // Optional device label (e.g. "iPhone 15 (Safari)")
                    .col(ColumnDef::new(RefreshTokens::DeviceLabel).string().null())
                    .col(
                        ColumnDef::new(RefreshTokens::ExpiresAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(RefreshTokens::Revoked)
                            .boolean()
                            .not_null()
                            .default(false),
                    )
                    .col(
                        ColumnDef::new(RefreshTokens::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    // Foreign key: deleting a user cascades to their tokens
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_refresh_tokens_user_id")
                            .from(RefreshTokens::Table, RefreshTokens::UserId)
                            .to(Users::Table, Users::Id)
                            .on_delete(ForeignKeyAction::Cascade)
                            .on_update(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        // Index on user_id (list sessions for a user)
        manager
            .create_index(
                Index::create()
                    .name("idx_refresh_tokens_user_id")
                    .table(RefreshTokens::Table)
                    .col(RefreshTokens::UserId)
                    .to_owned(),
            )
            .await?;

        // Index on family (revoke-family and list-sessions queries)
        manager
            .create_index(
                Index::create()
                    .name("idx_refresh_tokens_family")
                    .table(RefreshTokens::Table)
                    .col(RefreshTokens::Family)
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(RefreshTokens::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
pub enum RefreshTokens {
    Table,
    TokenHash,
    UserId,
    Family,
    DeviceLabel,
    ExpiresAt,
    Revoked,
    CreatedAt,
}
