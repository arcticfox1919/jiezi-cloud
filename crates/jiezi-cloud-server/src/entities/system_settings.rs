//! SeaORM entity for the `system_settings` table.
//!
//! Only `crate::repository::settings` should reference these types directly.
//! All other code works through [`SystemSettingsRepository`].
//!
//! [`SystemSettingsRepository`]: crate::repository::settings::SystemSettingsRepository

use sea_orm::entity::prelude::*;

/// Row model -- maps 1-to-1 onto the `system_settings` table.
///
/// The table is a flat key-value store for server-wide runtime configuration
/// (site name, registration policy, setup state, …).
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "system_settings")]
pub struct Model {
    /// Well-known setting key (e.g. `"setup_completed"`, `"site_name"`).
    #[sea_orm(primary_key, auto_increment = false, column_type = "Text")]
    pub key: String,

    /// String-serialised value.  Callers are responsible for interpreting the
    /// type (e.g. `"true"/"false"` for booleans, decimal digits for integers).
    #[sea_orm(column_type = "Text")]
    pub value: String,

    /// Timestamp of the last write.
    pub updated_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
