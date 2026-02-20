//! First-run setup wizard endpoints.
//!
//! These endpoints are always accessible (bypassed by the setup-guard
//! middleware) because they are the mechanism through which the server is
//! brought from an uninitialised state to an operational one.
//!
//! # Endpoints
//!
//! | Method | Path                    | Auth | Description                    |
//! |--------|-------------------------|------|--------------------------------|
//! | GET    | `/api/v1/setup/status`  | none | Check whether setup is needed  |
//! | POST   | `/api/v1/setup/complete`| none | Run the setup wizard           |
//!
//! # Flow
//!
//! 1. Client polls `GET /setup/status` on first load.
//! 2. If `setup_required: true`, the UI shows the setup wizard form.
//! 3. Client submits `POST /setup/complete` with the wizard payload.
//! 4. Server creates the Owner account, persists settings, and flips
//!    `setup_completed = true` in `system_settings`.
//! 5. `GET /setup/status` now returns `setup_required: false`.
//! 6. All routes become available; the client redirects to the login page.
//!
//! # Idempotency
//!
//! `POST /setup/complete` fails with `409 Conflict` if setup is already done,
//! so re-submission cannot create a second owner account.

use std::sync::atomic::Ordering;

use actix_web::{web, HttpResponse};
use serde::{Deserialize, Serialize};
use tracing::info;

use jiezi_cloud_core::error::AppError;

use crate::{error::ApiError, state::AppState};

// --- Request / response types ------------------------------------------------

/// Response body for `GET /setup/status`.
#[derive(Serialize)]
pub struct SetupStatusResponse {
    /// When `true` the server requires setup before it can be used.
    pub setup_required: bool,
    /// Jiezi Cloud server version string.
    pub version: &'static str,
}

/// Request body for `POST /setup/complete`.
///
/// All fields are validated server-side.  Passwords are never stored in plain
/// text; they are hashed with Argon2id before persistence.
#[derive(Deserialize)]
pub struct SetupCompleteRequest {
    // ---- Owner account ---------------------------------------------------------
    /// Login username for the super-admin account (3-50 chars, alphanumeric/-/_).
    pub admin_username: String,
    /// Email address for the super-admin account.
    pub admin_email: String,
    /// Password for the super-admin account (minimum 8 characters).
    pub admin_password: String,
    /// Optional display name shown in the UI (e.g. "System Administrator").
    pub admin_display_name: Option<String>,

    // ---- Site settings ---------------------------------------------------------
    /// Human-readable site name shown in the web UI and emails.
    pub site_name: String,
    /// Optional short description / tagline for the site.
    #[serde(default)]
    pub site_description: String,
    /// Allow unauthenticated visitors to register new accounts.
    /// Set to `false` for invite-only or enterprise deployments.
    #[serde(default)]
    pub registration_enabled: bool,
    /// Maximum upload size in megabytes.
    #[serde(default = "default_max_upload_mb")]
    pub max_upload_size_mb: u32,
}

fn default_max_upload_mb() -> u32 {
    512
}

/// Response body for a successful `POST /setup/complete`.
#[derive(Serialize)]
pub struct SetupCompleteResponse {
    pub success: bool,
    pub message: &'static str,
}

// --- Route registration ------------------------------------------------------

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.route("/status", web::get().to(status))
        .route("/complete", web::post().to(complete));
}

// --- Handlers ----------------------------------------------------------------

/// `GET /api/v1/setup/status`
///
/// Returns whether the server still needs to be configured.
/// Safe to call anonymously; returns no sensitive information.
pub async fn status(state: web::Data<AppState>) -> HttpResponse {
    let setup_required = !state.setup_completed.load(Ordering::Relaxed);
    HttpResponse::Ok().json(SetupStatusResponse {
        setup_required,
        version: env!("CARGO_PKG_VERSION"),
    })
}

/// `POST /api/v1/setup/complete`
///
/// Performs the first-run setup:
/// 1. Validates the request body.
/// 2. Creates the Owner account via [`AuthService::bootstrap_owner`].
/// 3. Persists site settings to the `system_settings` table.
/// 4. Marks setup as complete (DB + in-memory atomic flag).
///
/// Returns `409 Conflict` if setup has already been completed.
pub async fn complete(
    state: web::Data<AppState>,
    body: web::Json<SetupCompleteRequest>,
) -> Result<HttpResponse, ApiError> {
    // Idempotency guard: reject if already set up.
    if state.setup_completed.load(Ordering::Relaxed) {
        return Err(ApiError(AppError::Conflict(
            "setup has already been completed".into(),
        )));
    }

    let req = body.into_inner();

    // Validate site_name is not empty.
    if req.site_name.trim().is_empty() {
        return Err(ApiError(AppError::Validation(
            "site_name must not be empty".into(),
        )));
    }

    // 1. Create the Owner account (validates username / email / password).
    let owner = state
        .auth
        .bootstrap_owner(
            req.admin_username.clone(),
            req.admin_email.clone(),
            req.admin_password.clone(),
            req.admin_display_name.clone(),
        )
        .await
        .map_err(ApiError)?;

    // 2. Persist site settings via the repository (no raw SQL).
    let max_str = req.max_upload_size_mb.to_string();
    let settings_kv: &[(&str, &str)] = &[
        ("site_name",            req.site_name.as_str()),
        ("site_description",     req.site_description.as_str()),
        ("registration_enabled", if req.registration_enabled { "true" } else { "false" }),
        ("max_upload_size_mb",   max_str.as_str()),
        ("setup_completed",      "true"),
    ];

    for &(key, value) in settings_kv {
        state.settings.set(key, value).await.map_err(ApiError)?;
    }

    // 3. Flip the in-memory flag so the guard lets requests through immediately.
    state.setup_completed.store(true, Ordering::Relaxed);

    info!(
        owner_id  = %owner.id,
        username  = %owner.username,
        site_name = %req.site_name,
        "First-run setup completed"
    );

    Ok(HttpResponse::Ok().json(SetupCompleteResponse {
        success: true,
        message: "Setup completed successfully. You can now log in with the admin account.",
    }))
}


