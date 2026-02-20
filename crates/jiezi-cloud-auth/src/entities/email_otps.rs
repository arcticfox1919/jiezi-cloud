//! SeaORM entity for the `email_otps` table.
//!
//! Stores 6-digit OTP codes keyed by *email address* (not user ID) so that
//! the registration flow can verify a code before the user row exists.

use sea_orm::entity::prelude::*;

/// All valid values for the `purpose` column.
pub const PURPOSE_REGISTER:        &str = "register";
pub const PURPOSE_RESET_PASSWORD:  &str = "reset_password";
pub const PURPOSE_UNLOCK:          &str = "unlock";
pub const PURPOSE_CHANGE_PASSWORD: &str = "change_password";

/// One OTP record — may be pending (used_at IS NULL) or consumed.
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq)]
#[sea_orm(table_name = "email_otps")]
pub struct Model {
    /// UUID v4 primary key.
    #[sea_orm(primary_key, auto_increment = false, column_type = "Text")]
    pub id: String,

    /// Target email address.  Not a foreign key: during registration the user
    /// row does not exist yet.
    #[sea_orm(column_type = "Text")]
    pub email: String,

    /// 6-digit numeric code (e.g. "083421"), stored as plain text.
    /// Short TTL and rate-limiting are the primary security controls.
    #[sea_orm(column_type = "Text")]
    pub code: String,

    /// `"register"` | `"reset_password"` | `"unlock"`
    #[sea_orm(column_type = "Text")]
    pub purpose: String,

    /// When this code expires and becomes unusable.
    pub expires_at: DateTimeUtc,

    /// Set when the code was successfully consumed; NULL while still valid.
    #[sea_orm(nullable)]
    pub used_at: Option<DateTimeUtc>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
