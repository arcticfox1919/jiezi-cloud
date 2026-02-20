//! Repository for the `system_settings` table.
//!
//! Provides a thin, typed wrapper over the SeaORM entity so that call-sites
//! never write raw SQL or know about table column names.
//!
//! # Example
//!
//! ```rust,ignore
//! let repo = SystemSettingsRepository::new(db.clone());
//!
//! // Read
//! let name: Option<String> = repo.get("site_name").await?;
//!
//! // Write
//! repo.set("site_name", "My Drive").await?;
//!
//! // Typed helpers
//! let done: bool = repo.get_bool("setup_completed").await?;
//! ```

use chrono::Utc;
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, DatabaseConnection, EntityTrait,
};

use jiezi_cloud_core::error::{AppError, AppResult};

use crate::entities::system_settings::{self, ActiveModel, Entity};

/// Repository for server-wide key-value settings stored in `system_settings`.
///
/// Cheap to clone: the inner [`DatabaseConnection`] is an Arc-wrapped pool.
#[derive(Clone)]
pub struct SystemSettingsRepository {
    db: DatabaseConnection,
}

impl SystemSettingsRepository {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }

    // --- Read -----------------------------------------------------------------

    /// Return the raw string value for `key`, or `None` if the key is missing.
    pub async fn get(&self, key: &str) -> AppResult<Option<String>> {
        let row = Entity::find_by_id(key)
            .one(&self.db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(row.map(|m| m.value))
    }

    /// Convenience wrapper: parse the stored `"true"`/`"false"` string as a
    /// bool.  Returns `false` when the key is absent or the value is
    /// unrecognised.
    pub async fn get_bool(&self, key: &str) -> AppResult<bool> {
        Ok(self.get(key).await?.map(|v| v == "true").unwrap_or(false))
    }

    // --- Write ----------------------------------------------------------------

    /// Upsert `key = value` using native SeaORM active-model upsert so the
    /// correct dialect (SQLite REPLACE / Postgres INSERT … ON CONFLICT) is
    /// chosen automatically.
    pub async fn set(&self, key: &str, value: &str) -> AppResult<()> {
        let am = ActiveModel {
            key:        Set(key.to_owned()),
            value:      Set(value.to_owned()),
            updated_at: Set(Utc::now()),
        };

        // `save()` does INSERT if the PK is absent, UPDATE otherwise.
        am.save(&self.db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(())
    }

    /// Write a bool setting as `"true"` or `"false"`.
    pub async fn set_bool(&self, key: &str, value: bool) -> AppResult<()> {
        self.set(key, if value { "true" } else { "false" }).await
    }

    // --- Bulk read (used by admin settings endpoint) -------------------------

    /// Return all settings as a `Vec<Model>` (all rows in the table).
    pub async fn all(&self) -> AppResult<Vec<system_settings::Model>> {
        Entity::find()
            .all(&self.db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))
    }
}
