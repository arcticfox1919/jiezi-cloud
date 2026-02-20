//! SeaORM entity for the `users` table.
//!
//! Only this module and the repository layer should reference these generated
//! types directly.  All other code works with the domain `User` struct from
//! `jiezi-cloud-core`.

use sea_orm::entity::prelude::*;

/// Row model — maps 1-to-1 onto columns in the `users` table.
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq)]
#[sea_orm(table_name = "users")]
pub struct Model {
    /// UUIDv7 serialised as a hyphenated string — primary key.
    #[sea_orm(primary_key, auto_increment = false, column_type = "Text")]
    pub id: String,

    #[sea_orm(unique, column_type = "Text")]
    pub username: String,

    #[sea_orm(unique, column_type = "Text")]
    pub email: String,

    /// Argon2id PHC string — never serialised to JSON.
    #[sea_orm(column_type = "Text")]
    pub password_hash: String,

    /// Stored as a text discriminant: 'owner' | 'admin' | 'member' | 'guest'.
    #[sea_orm(column_type = "Text")]
    pub role: String,

    pub is_active: bool,

    #[sea_orm(nullable)]
    pub display_name: Option<String>,

    #[sea_orm(nullable)]
    pub avatar_url: Option<String>,

    /// Maximum storage in bytes; NULL = unlimited.
    #[sea_orm(nullable)]
    pub storage_quota: Option<i64>,

    /// Running total of bytes consumed.
    pub storage_used: i64,

    /// Consecutive failed login attempts since the last successful login.
    pub failed_login_count: i32,

    /// When set, the account is locked until this timestamp.
    #[sea_orm(nullable)]
    pub locked_until: Option<DateTimeUtc>,

    /// Timestamp of the most recent failed login attempt.
    #[sea_orm(nullable)]
    pub last_failed_login_at: Option<DateTimeUtc>,

    /// Whether the user has clicked the link in the verification email.
    pub email_verified: bool,

    pub created_at: DateTimeUtc,
    pub updated_at: DateTimeUtc,
}

/// Outbound relation: one user has many refresh tokens.
#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(has_many = "super::refresh_tokens::Entity")]
    RefreshTokens,
}

impl Related<super::refresh_tokens::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::RefreshTokens.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
