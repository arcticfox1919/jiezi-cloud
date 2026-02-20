//! SeaORM entity for the `refresh_tokens` table.
//!
//! Raw JWT strings are **never** stored.  Only the SHA-256 hex digest of the
//! raw JWT is persisted as the primary key.
//!
//! A `family` UUID groups all rotation descendants of a single login session,
//! enabling per-device logout and reuse-attack detection.

use sea_orm::entity::prelude::*;

/// Row model — maps 1-to-1 onto columns in the `refresh_tokens` table.
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq)]
#[sea_orm(table_name = "refresh_tokens")]
pub struct Model {
    /// SHA-256 hex digest of the raw JWT string — primary key.
    /// Changing on every rotation; the `family` column is the stable session key.
    #[sea_orm(primary_key, auto_increment = false, column_type = "Text")]
    pub token_hash: String,

    /// Foreign key → `users.id`.
    #[sea_orm(column_type = "Text")]
    pub user_id: String,

    /// Rotation family UUID.  All tokens from the same login session share this
    /// value.  Pass it to revoke all tokens for a specific device (logout).
    #[sea_orm(column_type = "Text")]
    pub family: String,

    /// Optional human-readable label supplied by the client at login time,
    /// e.g. "iPhone 15 (Safari)", "Home PC (Firefox)".
    ///
    /// Copied verbatim to every rotation token within the same family so that
    /// `list_sessions` can always show the label without a join.
    #[sea_orm(nullable)]
    pub device_label: Option<String>,

    /// When this specific token expires.
    pub expires_at: DateTimeUtc,

    /// `true` once this token has been consumed by a rotation or explicit logout.
    /// Revoked tokens are kept (not deleted) so reuse-attack detection works.
    pub revoked: bool,

    pub created_at: DateTimeUtc,
}

/// Inbound relation: belongs to one user.
#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::users::Entity",
        from = "Column::UserId",
        to = "super::users::Column::Id",
        on_update = "Cascade",
        on_delete = "Cascade"
    )]
    User,
}

impl Related<super::users::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::User.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
