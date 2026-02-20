//! Jiezi Cloud server — library target.
//!
//! Exposes the internal wiring of the HTTP server so that the integration-test
//! harness (`tests/`) can construct a real Actix-web application wired to an
//! in-memory SQLite database without spawning a child process.
//!
//! # Module layout
//!
//! | Module        | Purpose                                               |
//! |---------------|-------------------------------------------------------|
//! | `entities`    | SeaORM entity definitions for server-local DB tables  |
//! | `error`       | `ApiError` newtype bridging `AppError` → HTTP         |
//! | `middleware`  | `setup_guard` + JWT `AuthUser` extractor              |
//! | `repository`  | `SystemSettingsRepository` (key/value settings table) |
//! | `routes`      | All Actix-web route handlers and `configure()`        |
//! | `state`       | `AppState` — shared data injected into every handler  |

pub mod entities;
pub mod error;
pub mod middleware;
pub mod repository;
pub mod routes;
pub mod state;

use std::sync::{Arc, atomic::AtomicBool};
use std::time::Duration;

use sea_orm::DatabaseConnection;

use jiezi_cloud_auth::{
    repository::{RefreshTokenRepository, UserRepository},
    AuthServiceImpl,
};
use jiezi_cloud_config::AppConfig;
use jiezi_cloud_vfs::{repository::FileNodeRepository, VfsServiceImpl};

use crate::repository::settings::SystemSettingsRepository;

// ─── App-state factory ────────────────────────────────────────────────────────

/// Build [`state::AppState`] from an already-open database connection.
///
/// Both `main.rs` and the integration-test harness use this function to wire
/// service objects, ensuring the production path and the test path are
/// identical.
///
/// # Parameters
///
/// - `db`              — open SeaORM connection pool (migrations already run).
/// - `cfg`             — loaded application configuration.
/// - `setup_completed` — initial value of the setup-wizard flag.
///   Pass `false` for a fresh database (tests exercising the wizard),
///   `true` to skip the wizard check in tests focused on other flows.
pub async fn build_app_state(
    db: DatabaseConnection,
    cfg: &AppConfig,
    setup_completed: bool,
) -> state::AppState {
    let access_ttl  = Duration::from_secs(cfg.auth.access_token_ttl_seconds);
    let refresh_ttl = Duration::from_secs(cfg.auth.refresh_token_ttl_seconds);

    let auth_service = Arc::new(AuthServiceImpl::new(
        UserRepository::new(db.clone()),
        RefreshTokenRepository::new(db.clone()),
        &cfg.auth.jwt_secret,
        access_ttl,
        refresh_ttl,
    ));

    let vfs_service = Arc::new(VfsServiceImpl::new(FileNodeRepository::new(db.clone())));

    let settings_repo = SystemSettingsRepository::new(db.clone());

    state::AppState {
        db,
        auth: auth_service,
        vfs: vfs_service,
        settings: settings_repo,
        setup_completed: Arc::new(AtomicBool::new(setup_completed)),
    }
}
