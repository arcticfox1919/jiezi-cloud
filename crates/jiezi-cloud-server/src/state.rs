//! Shared application state threaded through every Actix-web handler.
//!
//! `AppState` is stored in Actix-web's data container via `App::app_data` and
//! extracted in handlers with `web::Data<AppState>`.  All fields are cheaply
//! cloneable (the database pool and `Arc<dyn Trait>` clone without copying
//! underlying resources).

use std::sync::{atomic::AtomicBool, Arc};

use sea_orm::DatabaseConnection;
use tokio::sync::broadcast;

use jiezi_cloud_auth::JwtManager;
use jiezi_cloud_core::{
    events::DomainEvent,
    traits::{auth::AuthService, kb::KbService, vfs::VfsService},
};
use jiezi_cloud_storage::{DownloadService, StorageManager, UploadService};

use crate::repository::{
    download_token::DownloadTokenRepository,
    settings::SystemSettingsRepository,
    upload_session::UploadSessionRepository,
};

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
    /// Resumable-upload session repository.
    ///
    /// Tracks chunked upload sessions and their temp files.
    pub upload_sessions: UploadSessionRepository,
    /// Download token repository.
    ///
    /// Issues and validates time-limited, optionally single-use download tokens.
    pub download_tokens: DownloadTokenRepository,
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

    // ── Domain event bus ──────────────────────────────────────────────────────

    /// Internal in-process domain event bus.
    ///
    /// Upload and VFS mutation handlers publish events here.  Subscriber tasks
    /// (KB indexer, future thumbnail generator, audit log, …) receive them via
    /// `domain_events.subscribe()`.
    pub domain_events: Arc<broadcast::Sender<DomainEvent>>,

    // ── Knowledge-base layer ──────────────────────────────────────────────────

    /// Knowledge-base service — Markdown indexing, WikiLink backlinks, blog
    /// publishing.  `None` until the KB feature is fully configured.
    pub kb: Arc<dyn KbService>,

    // ── File-transfer routing parameters ─────────────────────────────────────

    /// UDP port of the QUIC transfer server, or `None` when QUIC is disabled.
    ///
    /// Returned to native clients in `426 Upgrade Required` responses so that
    /// they can reconnect on the correct port.
    pub quic_port: Option<u16>,

    /// Whether the tunnel relay is currently enabled.
    ///
    /// Controls which web-upload limit is enforced:
    /// - `false` → `web_upload_limit_no_tunnel` (500 MiB by default)
    /// - `true`  → `web_upload_limit_with_tunnel` (100 MiB by default)
    pub tunnel_enabled: bool,

    /// Minimum file size (bytes) that forces native clients to use QUIC.
    ///
    /// Derived from `storage.large_file_threshold_bytes`.
    pub large_file_threshold: u64,

    /// Maximum HTTP upload size for web clients when the tunnel is disabled.
    pub web_upload_limit_no_tunnel: u64,

    /// Maximum HTTP upload size for web clients when the tunnel is enabled.
    pub web_upload_limit_with_tunnel: u64,
}
