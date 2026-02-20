//! SeaORM entity for the `email_verification_tokens` table.

use sea_orm::entity::prelude::*;

/// Row model — one row per pending (or used) verification token.
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq)]
#[sea_orm(table_name = "email_verification_tokens")]
pub struct Model {
    /// UUIDv4 string — primary key.
    #[sea_orm(primary_key, auto_increment = false, column_type = "Text")]
    pub id: String,

    /// ID of the user this token belongs to.
    #[sea_orm(column_type = "Text")]
    pub user_id: String,

    /// SHA-256 hex digest of the raw token sent in the email link.
    /// The raw token is never stored in the database.
    #[sea_orm(column_type = "Text", unique)]
    pub token_hash: String,

    /// When the token expires and becomes unusable.
    pub expires_at: DateTimeUtc,

    /// When the token was consumed by the user clicking the link.
    /// `None` means the token has not yet been used.
    #[sea_orm(nullable)]
    pub used_at: Option<DateTimeUtc>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::users::Entity",
        from = "Column::UserId",
        to = "super::users::Column::Id",
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
