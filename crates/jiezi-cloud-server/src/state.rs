//! Shared application state threaded through every Actix-web handler.
//!
//! `AppState` is stored in Actix-web's data container via `App::app_data` and
//! extracted in handlers with `web::Data<AppState>`.  All fields are cheaply
//! cloneable (the database pool and `Arc<dyn Trait>` clone without copying
//! underlying resources).

use std::sync::{atomic::AtomicBool, Arc};

use sea_orm::DatabaseConnection;

use jiezi_cloud_auth::JwtManager;
use jiezi_cloud_core::traits::{auth::AuthService, vfs::VfsService};
use jiezi_cloud_storage::{DownloadService, StorageManager, UploadService};

use crate::repository::settings::SystemSettingsRepository;

/// Application-wide shared state.
#[derive(Clone)]
pub struct AppState {
    /// SeaORM connection pool (SQLite / PostgreSQL / MySQL).
    pub db: DatabaseConnection,
    /// Authentication and authorization service.
    pub auth: Arc<dyn AuthService>,
    /// ES256 JWT manager — used by the QUIC server to validate bearer tokens.
    pub jwt: Arc<JwtManager>,
    /// Virtual file system service.
    pub vfs: Arc<dyn VfsService>,
    /// Multi-backend storage manager (reads + backend health).
    pub storage: Arc<StorageManager>,
    /// Upload pipeline: CDC → backends → DB chunk records.
    pub upload: UploadService,
    /// Download pipeline: DB chunk records → backends → byte stream.
    pub download: DownloadService,
    /// Repository for the `system_settings` key-value table.
    ///
    /// Used by setup and admin-settings handlers to read/write server
    /// configuration without embedding raw SQL in route modules.
    pub settings: SystemSettingsRepository,
    /// Whether the first-run setup wizard has been completed.
    ///
    /// Loaded from `system_settings.setup_completed` at startup and flipped
    /// to `true` by `POST /api/v1/setup/complete`.  The setup-guard middleware
    /// reads this atomically on every request without touching the database.
    pub setup_completed: Arc<AtomicBool>,
}
