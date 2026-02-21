//! SeaORM entity for the `download_tokens` table.
//!
//! Each row is a time-limited (optionally single-use) token that authorises
//! an unauthenticated download of a specific file node.

use sea_orm::entity::prelude::*;

/// Row model — maps 1-to-1 onto columns in the `download_tokens` table.
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "download_tokens")]
pub struct Model {
    /// 32-char random hex string — the secret the caller presents in the URL.
    #[sea_orm(primary_key, auto_increment = false, column_type = "Text")]
    pub token: String,

    /// Foreign key → `file_nodes.id`.
    #[sea_orm(column_type = "Text")]
    pub file_node_id: String,

    /// Foreign key → `users.id` — the user who issued the token.
    #[sea_orm(column_type = "Text")]
    pub user_id: String,

    /// Hard expiry.  Tokens past this time are rejected and eventually purged.
    pub expires_at: DateTimeUtc,

    /// When `true` the token is invalidated immediately after the first
    /// successful download (`used = true`).
    pub one_time: bool,

    /// Set to `true` once a one-time token has been consumed.
    /// Always `false` for reusable tokens.
    pub used: bool,

    pub created_at: DateTimeUtc,
}

/// No active SeaORM relations needed — FK constraints are handled by the DB.
#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
