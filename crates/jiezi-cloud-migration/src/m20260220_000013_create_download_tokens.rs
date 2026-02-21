//! Migration: create the `download_tokens` table.
//!
//! Supports time-limited and optional one-time-use download links (Stage 8).
//! A token is generated for an authenticated user and can be used to download
//! a file without a JWT — useful for direct browser links or share links.

use sea_orm_migration::prelude::*;

use super::m20260220_000001_create_users::Users;
use super::m20260220_000004_create_file_nodes::FileNodes;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(DownloadTokens::Table)
                    .if_not_exists()
                    // 32-char random hex string — the secret the client presents.
                    .col(
                        ColumnDef::new(DownloadTokens::Token)
                            .string()
                            .not_null()
                            .primary_key(),
                    )
                    // The file this token unlocks.
                    .col(
                        ColumnDef::new(DownloadTokens::FileNodeId)
                            .string()
                            .not_null(),
                    )
                    // The user who created the token (used for audit and revocation).
                    .col(
                        ColumnDef::new(DownloadTokens::UserId)
                            .string()
                            .not_null(),
                    )
                    // Hard expiry timestamp.
                    .col(
                        ColumnDef::new(DownloadTokens::ExpiresAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    // When `true` the token is invalidated immediately after the first
                    // successful download.
                    .col(
                        ColumnDef::new(DownloadTokens::OneTime)
                            .boolean()
                            .not_null()
                            .default(false),
                    )
                    // Set to `true` once a one-time token has been consumed.
                    .col(
                        ColumnDef::new(DownloadTokens::Used)
                            .boolean()
                            .not_null()
                            .default(false),
                    )
                    .col(
                        ColumnDef::new(DownloadTokens::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_download_tokens_file_node_id")
                            .from(DownloadTokens::Table, DownloadTokens::FileNodeId)
                            .to(FileNodes::Table, FileNodes::Id)
                            .on_delete(ForeignKeyAction::Cascade)
                            .on_update(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_download_tokens_user_id")
                            .from(DownloadTokens::Table, DownloadTokens::UserId)
                            .to(Users::Table, Users::Id)
                            .on_delete(ForeignKeyAction::Cascade)
                            .on_update(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        // Index on file_node_id — list or revoke all tokens for a file.
        manager
            .create_index(
                Index::create()
                    .name("idx_download_tokens_file_node_id")
                    .table(DownloadTokens::Table)
                    .col(DownloadTokens::FileNodeId)
                    .to_owned(),
            )
            .await?;

        // Index on expires_at — efficient GC sweep.
        manager
            .create_index(
                Index::create()
                    .name("idx_download_tokens_expires_at")
                    .table(DownloadTokens::Table)
                    .col(DownloadTokens::ExpiresAt)
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(DownloadTokens::Table).to_owned())
            .await
    }
}

// ─── Iden enum ────────────────────────────────────────────────────────────────

#[derive(Iden)]
pub enum DownloadTokens {
    Table,
    Token,
    FileNodeId,
    UserId,
    ExpiresAt,
    OneTime,
    Used,
    CreatedAt,
}
