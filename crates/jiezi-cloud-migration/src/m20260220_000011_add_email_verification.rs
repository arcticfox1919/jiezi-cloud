//! Migration 011 — Email OTP support
//!
//! Changes:
//! 1. Add `email_verified BOOLEAN NOT NULL DEFAULT FALSE` to `users`
//!    (kept for audit purposes — set to true after OTP-gated registration).
//! 2. Create `email_otps` table: stores short-lived 6-digit OTP codes used
//!    for registration, password reset, and account unlock flows.
//!
//! # Why `email_otps` instead of `email_verification_tokens`
//!
//! The previous design stored long random tokens for a post-registration
//! click-through link.  The new design issues a short numeric OTP code
//! *before* registration so the user proves email ownership as part of the
//! sign-up form rather than in a separate step.

use sea_orm_migration::prelude::*;

pub struct Migration;

impl MigrationName for Migration {
    fn name(&self) -> &str {
        "m20260220_000011_add_email_verification"
    }
}

// ─── Column identifiers ───────────────────────────────────────────────────────

#[derive(Iden)]
enum Users {
    Table,
    EmailVerified,
}

#[derive(Iden)]
enum EmailOtps {
    Table,
    Id,
    /// Target email address (NOT a FK — during registration no user exists yet).
    Email,
    /// 6-digit numeric code stored as text ("012345").
    Code,
    /// 'register' | 'reset_password' | 'unlock'
    Purpose,
    ExpiresAt,
    /// NULL = not yet consumed.
    UsedAt,
}

// ─── Up / Down ────────────────────────────────────────────────────────────────

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // 1. Add email_verified column to users.
        manager
            .alter_table(
                Table::alter()
                    .table(Users::Table)
                    .add_column(
                        ColumnDef::new(Users::EmailVerified)
                            .boolean()
                            .not_null()
                            .default(false),
                    )
                    .to_owned(),
            )
            .await?;

        // 2. Create email_otps table.
        //
        // Keyed by email (not user_id) so the 'register' flow can validate
        // the code *before* the user row is created.
        manager
            .create_table(
                Table::create()
                    .table(EmailOtps::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(EmailOtps::Id)
                            .text()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(EmailOtps::Email).text().not_null())
                    .col(ColumnDef::new(EmailOtps::Code).text().not_null())
                    .col(ColumnDef::new(EmailOtps::Purpose).text().not_null())
                    .col(
                        ColumnDef::new(EmailOtps::ExpiresAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(EmailOtps::UsedAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    // Index for fast lookup: find valid OTPs by email+purpose.
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(EmailOtps::Table).if_exists().to_owned())
            .await?;

        manager
            .alter_table(
                Table::alter()
                    .table(Users::Table)
                    .drop_column(Users::EmailVerified)
                    .to_owned(),
            )
            .await
    }
}
