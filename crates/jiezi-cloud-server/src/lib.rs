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
pub mod quic;
pub mod repository;
pub mod routes;
pub mod state;

use std::sync::{Arc, atomic::AtomicBool};
use std::time::Duration;

use sea_orm::DatabaseConnection;

use jiezi_cloud_auth::{
    EmailService, EmailOtpRepository, JwtManager,
    repository::{RefreshTokenRepository, UserRepository},
    AuthServiceImpl,
};
use jiezi_cloud_config::AppConfig;
use jiezi_cloud_storage::{DownloadService, LocalFsBackend, StorageManager, UploadService};
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

    // ── JWT keypair ───────────────────────────────────────────────────────────
    //
    // ES256 (ECDSA P-256): home server holds the private key and can sign;
    // the tunnel receives only the public key via RegisterNode and can verify
    // but never forge tokens.
    let jwt_manager = Arc::new(if cfg.auth.jwt_private_key_pem == "GENERATE" {
        tracing::warn!(
            "jwt_private_key_pem = \"GENERATE\": using ephemeral ES256 keypair. \
             All tokens are invalidated on restart. \
             Set JIEZI__AUTH__JWT_PRIVATE_KEY_PEM for a persistent key."
        );
        JwtManager::generate(access_ttl, refresh_ttl)
            .expect("ES256 keypair generation must succeed")
    } else {
        JwtManager::from_pkcs8_pem(&cfg.auth.jwt_private_key_pem, access_ttl, refresh_ttl)
            .expect("jwt_private_key_pem: invalid PKCS#8 PEM — check JIEZI__AUTH__JWT_PRIVATE_KEY_PEM")
    });
    tracing::info!(
        public_key_pem = %jwt_manager.public_key_pem(),
        "ES256 JWT public key (include in RegisterNode message when connecting to tunnel)"
    );

    let auth_service = {
        let base = AuthServiceImpl::new(
            UserRepository::new(db.clone()),
            RefreshTokenRepository::new(db.clone()),
            jwt_manager.clone(),
            refresh_ttl,
        )
        .with_security(
            cfg.security.max_login_attempts,
            cfg.security.lockout_duration_secs,
            cfg.security.max_password_bytes,
        );

        // Wire email verification only when the feature is actually needed.
        // Conditions:
        //   email.enabled = true          → SMTP or dev-log service is active;
        //                                   verification tokens are issued on register.
        //   email.verification_required   → login is blocked until email is confirmed;
        //                                   implies the feature must be enabled.
        // When both are false: with_email() is never called, so email_repo and
        // email_svc stay None throughout the lifetime of the service — the verify
        // and resend routes return 500 "not configured" if somehow called.
        let needs_email = cfg.email.enabled || cfg.email.verification_required;

        let auth = if needs_email {
            let email_svc = match EmailService::new(
                cfg.email.enabled,
                &cfg.email.smtp_host,
                cfg.email.smtp_port,
                &cfg.email.smtp_username,
                &cfg.email.smtp_password,
                cfg.email.from_address.clone(),
                cfg.email.from_name.clone(),
            ) {
                Ok(svc) => svc,
                Err(e) => {
                    // Fall back to a disabled (log-only) service so the server
                    // can still start — email is not critical to the core file
                    // sync functionality.
                    tracing::warn!(
                        error = %e,
                        "failed to build email service; OTP emails will be logged only"
                    );
                    EmailService::new(
                        false, "", 587, "", "",
                        cfg.email.from_address.clone(),
                        cfg.email.from_name.clone(),
                    ).expect("fallback email service build must succeed")
                }
            };
            Arc::new(
                base.with_email(
                    cfg.email.verification_required,
                    EmailOtpRepository::new(db.clone()),
                    Arc::new(email_svc),
                    cfg.email.otp_ttl_secs,
                ),
            )
        } else {
            Arc::new(base)
        };
        auth
    };

    let vfs_service = Arc::new(VfsServiceImpl::new(FileNodeRepository::new(db.clone())));

    // ── Storage / upload / download ───────────────────────────────────────────
    //
    // Bootstrap a single LocalFsBackend from the configured `local_root`.
    // A full `BackendRepository` (reading from `storage_backend_configs`) is
    // a TODO; for now the local backend is sufficient for single-node installs.
    let storage = {
        let backend = Arc::new(
            LocalFsBackend::new("local", cfg.storage.local_root.clone()),
        );
        let mgr = StorageManager::with_backends(vec![
            backend as Arc<dyn jiezi_cloud_core::traits::storage::StorageBackend>,
        ]);
        Arc::new(mgr)
    };
    let upload   = UploadService::new(storage.clone(), db.clone());
    let download = DownloadService::new(storage.clone(), db.clone());

    let settings_repo = SystemSettingsRepository::new(db.clone());

    state::AppState {
        db,
        auth: auth_service,
        jwt: jwt_manager,
        vfs: vfs_service,
        storage,
        upload,
        download,
        settings: settings_repo,
        setup_completed: Arc::new(AtomicBool::new(setup_completed)),
    }
}
