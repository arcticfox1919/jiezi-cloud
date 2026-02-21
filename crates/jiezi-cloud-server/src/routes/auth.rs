//! Authentication API handlers.
//!
//! All routes are mounted under `/api/v1/auth` by [`super::configure`].
//!
//! | Method   | Path                          | Auth? | Description                            |
//! |----------|-------------------------------|-------|----------------------------------------|
//! | POST     | `/auth/send-register-otp`     | No    | Send OTP to email before registration  |
//! | POST     | `/auth/register`              | No    | Create a new user account              |
//! | POST     | `/auth/login`                 | No    | Obtain a JWT token pair                |
//! | POST     | `/auth/refresh`               | No    | Rotate the refresh token               |
//! | GET      | `/auth/me`                    | Yes   | Return current user profile (full)     |
//! | PATCH    | `/auth/me`                    | Yes   | Update display name / avatar           |
//! | POST     | `/auth/me/send-change-password-otp` | Yes | Send OTP to own email before changing password |
//! | POST     | `/auth/me/password`           | Yes   | Change own password (old+new+OTP when enabled) |
//! | POST     | `/auth/logout`                | No    | Revoke a refresh token                 |
//! | GET      | `/auth/sessions`              | Yes   | List active sessions                   |
//! | DELETE   | `/auth/sessions/{family}`     | Yes   | Revoke a specific session              |
//! | POST     | `/auth/forgot-password`       | No    | Send password-reset OTP                |
//! | POST     | `/auth/reset-password`        | No    | Reset password using OTP               |
//! | POST     | `/auth/send-unlock-otp`       | No    | Send unlock OTP (if account is locked) |
//! | POST     | `/auth/unlock-account`        | No    | Unlock account using OTP               |

use std::str::FromStr;

use actix_web::{web, HttpResponse};
use serde::Deserialize;

use jiezi_cloud_core::error::AppError;
use jiezi_cloud_core::models::user::{
    ChangeOwnPasswordRequest, LoginRequest, RegisterRequest,
    ResetPasswordWithOtpRequest, SendOtpRequest, SessionInfo, TokenPair, UnlockWithOtpRequest,
    UpdateProfileRequest, User,
};
use jiezi_cloud_core::types::UserId;

use crate::error::ApiError;
use crate::middleware::auth::AuthUser;
use crate::state::AppState;

// ─── Route registration ───────────────────────────────────────────────────────

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg
        // ── OTP helpers (must come before /register so the path is unambiguous) ──
        .route("/send-register-otp",  web::post().to(send_register_otp))
        .route("/forgot-password",    web::post().to(forgot_password))
        .route("/reset-password",     web::post().to(reset_password))
        .route("/send-unlock-otp",    web::post().to(send_unlock_otp))
        .route("/unlock-account",     web::post().to(unlock_account))
        // ── Core auth ─────────────────────────────────────────────────────────
        .route("/register",           web::post().to(register))
        .route("/login",              web::post().to(login))
        .route("/refresh",            web::post().to(refresh))
        .route("/me",                 web::get().to(me))
        .route("/me",                 web::patch().to(update_me))
        .route("/me/send-change-password-otp", web::post().to(send_change_password_otp))
        .route("/me/password",        web::post().to(change_password))
        .route("/logout",             web::post().to(logout))
        .route("/sessions",           web::get().to(list_sessions))
        .route("/sessions/{family}",  web::delete().to(revoke_session));
}

// ─── Request / response helpers ───────────────────────────────────────────────

#[derive(Deserialize, utoipa::ToSchema)]
pub struct RefreshBody {
    pub refresh_token: String,
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct LogoutBody {
    pub refresh_token: String,
}
// ─── Handlers ─────────────────────────────────────────────────────────────────

/// `POST /auth/register` — create a new account.
///
/// Returns `201 Created` with the [`User`] object on success.
#[utoipa::path(
    post,
    path = "/api/v1/auth/register",
    request_body = RegisterRequest,
    responses(
        (status = 201, description = "User registered successfully", body = User),
        (status = 400, description = "Validation error"),
        (status = 409, description = "Username or email already taken"),
    ),
    tag = "auth"
)]
pub async fn register(
    state: web::Data<AppState>,
    body: web::Json<RegisterRequest>,
) -> Result<HttpResponse, ApiError> {
    let user = state.auth.register(body.into_inner()).await?;
    Ok(HttpResponse::Created().json(user))
}

/// `POST /auth/login` — authenticate with credentials.
///
/// Returns `200 OK` with a [`TokenPair`] on success.
#[utoipa::path(
    post,
    path = "/api/v1/auth/login",
    request_body = LoginRequest,
    responses(
        (status = 200, description = "Login successful", body = TokenPair),
        (status = 401, description = "Invalid credentials"),
        (status = 423, description = "Account locked"),
    ),
    tag = "auth"
)]
pub async fn login(
    state: web::Data<AppState>,
    body: web::Json<LoginRequest>,
) -> Result<HttpResponse, ApiError> {
    let tokens = state.auth.login(body.into_inner()).await?;
    Ok(HttpResponse::Ok().json(tokens))
}

/// `POST /auth/refresh` — exchange a refresh token for a new pair (rotation).
#[utoipa::path(
    post,
    path = "/api/v1/auth/refresh",
    request_body = RefreshBody,
    responses(
        (status = 200, description = "Token refreshed", body = TokenPair),
        (status = 401, description = "Invalid or expired refresh token"),
    ),
    tag = "auth"
)]
pub async fn refresh(
    state: web::Data<AppState>,
    body: web::Json<RefreshBody>,
) -> Result<HttpResponse, ApiError> {
    let tokens = state.auth.refresh_token(&body.refresh_token).await?;
    Ok(HttpResponse::Ok().json(tokens))
}

/// `GET /auth/me` — return the current user's full profile.
#[utoipa::path(
    get,
    path = "/api/v1/auth/me",
    responses(
        (status = 200, description = "Current user profile", body = User),
        (status = 401, description = "Unauthorized"),
    ),
    security(("bearer_auth" = [])),
    tag = "auth"
)]
pub async fn me(
    state: web::Data<AppState>,
    auth:  AuthUser,
) -> Result<HttpResponse, ApiError> {
    let user_id = parse_user_id(&auth.0.sub)?;
    let user = state.auth.get_user(&user_id).await.map_err(ApiError)?;
    Ok(HttpResponse::Ok().json(user))
}

/// `PATCH /auth/me` — update the current user's display name and / or avatar.
///
/// Only the fields present in the request body are updated.  Omit a field
/// entirely to leave it unchanged; set it to `null` to clear it.
#[utoipa::path(
    patch,
    path = "/api/v1/auth/me",
    request_body = UpdateProfileRequest,
    responses(
        (status = 200, description = "Profile updated", body = User),
        (status = 401, description = "Unauthorized"),
    ),
    security(("bearer_auth" = [])),
    tag = "auth"
)]
pub async fn update_me(
    state: web::Data<AppState>,
    auth:  AuthUser,
    body:  web::Json<UpdateProfileRequest>,
) -> Result<HttpResponse, ApiError> {
    let user_id = parse_user_id(&auth.0.sub)?;
    let updated = state
        .auth
        .update_profile(&user_id, body.into_inner())
        .await
        .map_err(ApiError)?;
    Ok(HttpResponse::Ok().json(updated))
}

/// `POST /auth/me/password` — change the current user's own password.
///
/// Requires the caller to supply their **current** password in `old_password`.
/// For admin-initiated forced resets see `POST /admin/users/{id}/reset-password`.
#[utoipa::path(
    post,
    path = "/api/v1/auth/me/password",
    request_body = ChangeOwnPasswordRequest,
    responses(
        (status = 204, description = "Password changed"),
        (status = 401, description = "Unauthorized or wrong current password"),
    ),
    security(("bearer_auth" = [])),
    tag = "auth"
)]
pub async fn change_password(
    state: web::Data<AppState>,
    auth:  AuthUser,
    body:  web::Json<ChangeOwnPasswordRequest>,
) -> Result<HttpResponse, ApiError> {
    let user_id = parse_user_id(&auth.0.sub)?;
    state
        .auth
        .change_own_password(&user_id, body.into_inner())
        .await
        .map_err(ApiError)?;
    Ok(HttpResponse::NoContent().finish())
}

/// `POST /auth/me/send-change-password-otp` — send OTP to own email before changing password.
///
/// No-op (204) when email verification is disabled in config.
#[utoipa::path(
    post,
    path = "/api/v1/auth/me/send-change-password-otp",
    responses(
        (status = 204, description = "OTP sent (or no-op if email verification is disabled)"),
        (status = 401, description = "Unauthorized"),
    ),
    security(("bearer_auth" = [])),
    tag = "auth"
)]
pub async fn send_change_password_otp(
    state: web::Data<AppState>,
    auth:  AuthUser,
) -> Result<HttpResponse, ApiError> {
    let user_id = parse_user_id(&auth.0.sub)?;
    state
        .auth
        .send_change_password_otp(&user_id)
        .await
        .map_err(ApiError)?;
    Ok(HttpResponse::NoContent().finish())
}

/// `POST /auth/logout` — revoke a refresh token (log out a device).
#[utoipa::path(
    post,
    path = "/api/v1/auth/logout",
    request_body = LogoutBody,
    responses(
        (status = 204, description = "Logged out"),
    ),
    tag = "auth"
)]
pub async fn logout(
    state: web::Data<AppState>,
    body: web::Json<LogoutBody>,
) -> Result<HttpResponse, ApiError> {
    state.auth.revoke_token(&body.refresh_token).await?;
    Ok(HttpResponse::NoContent().finish())
}

/// `GET /auth/sessions` — list all active sessions for the current user.
#[utoipa::path(
    get,
    path = "/api/v1/auth/sessions",
    responses(
        (status = 200, description = "Active sessions", body = Vec<SessionInfo>),
        (status = 401, description = "Unauthorized"),
    ),
    security(("bearer_auth" = [])),
    tag = "auth"
)]
pub async fn list_sessions(
    state: web::Data<AppState>,
    auth: AuthUser,
) -> Result<HttpResponse, ApiError> {
    let user_id = parse_user_id(&auth.0.sub)?;
    let sessions = state.auth.list_sessions(&user_id).await?;
    Ok(HttpResponse::Ok().json(sessions))
}

/// `DELETE /auth/sessions/{family}` — revoke a single device session.
#[utoipa::path(
    delete,
    path = "/api/v1/auth/sessions/{family}",
    params(
        ("family" = String, Path, description = "Session family UUID to revoke")
    ),
    responses(
        (status = 204, description = "Session revoked"),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Session not found"),
    ),
    security(("bearer_auth" = [])),
    tag = "auth"
)]
pub async fn revoke_session(
    state: web::Data<AppState>,
    auth: AuthUser,
    path: web::Path<String>,
) -> Result<HttpResponse, ApiError> {
    let user_id = parse_user_id(&auth.0.sub)?;
    let family = path.into_inner();
    state.auth.revoke_session(&user_id, &family).await?;
    Ok(HttpResponse::NoContent().finish())
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

fn parse_user_id(sub: &str) -> Result<UserId, ApiError> {
    UserId::from_str(sub).map_err(|_| {
        ApiError(AppError::Unauthorized(
            "invalid user ID in token subject".into(),
        ))
    })
}

// ─── OTP handlers ─────────────────────────────────────────────────────────────

/// `POST /auth/send-register-otp` — email a 6-digit code before registration.
///
/// The client should call this before `POST /auth/register` when
/// `email.verification_required = true`.  Returns `204 No Content`.
#[utoipa::path(
    post,
    path = "/api/v1/auth/send-register-otp",
    request_body = SendOtpRequest,
    responses(
        (status = 204, description = "OTP sent"),
    ),
    tag = "auth"
)]
pub async fn send_register_otp(
    state: web::Data<AppState>,
    body:  web::Json<SendOtpRequest>,
) -> Result<HttpResponse, ApiError> {
    state.auth.send_register_otp(body.into_inner()).await?;
    Ok(HttpResponse::NoContent().finish())
}

/// `POST /auth/forgot-password` — email a password-reset OTP.
///
/// Always returns `204 No Content` (even if the email is not registered)
/// to prevent email enumeration.
#[utoipa::path(
    post,
    path = "/api/v1/auth/forgot-password",
    request_body = SendOtpRequest,
    responses(
        (status = 204, description = "Password reset OTP sent (always 204 to prevent email enumeration)"),
    ),
    tag = "auth"
)]
pub async fn forgot_password(
    state: web::Data<AppState>,
    body:  web::Json<SendOtpRequest>,
) -> Result<HttpResponse, ApiError> {
    state.auth.send_reset_password_otp(body.into_inner()).await?;
    Ok(HttpResponse::NoContent().finish())
}

/// `POST /auth/reset-password` — set a new password using the OTP.
///
/// Returns `204 No Content` on success.
#[utoipa::path(
    post,
    path = "/api/v1/auth/reset-password",
    request_body = ResetPasswordWithOtpRequest,
    responses(
        (status = 204, description = "Password reset successful"),
        (status = 400, description = "Invalid OTP or expired"),
    ),
    tag = "auth"
)]
pub async fn reset_password(
    state: web::Data<AppState>,
    body:  web::Json<ResetPasswordWithOtpRequest>,
) -> Result<HttpResponse, ApiError> {
    state.auth.reset_password_with_otp(body.into_inner()).await?;
    Ok(HttpResponse::NoContent().finish())
}

/// `POST /auth/send-unlock-otp` — email an unlock OTP for a locked account.
///
/// Returns `204 No Content` always (prevents enumeration).
#[utoipa::path(
    post,
    path = "/api/v1/auth/send-unlock-otp",
    request_body = SendOtpRequest,
    responses(
        (status = 204, description = "Unlock OTP sent (always 204 to prevent enumeration)"),
    ),
    tag = "auth"
)]
pub async fn send_unlock_otp(
    state: web::Data<AppState>,
    body:  web::Json<SendOtpRequest>,
) -> Result<HttpResponse, ApiError> {
    state.auth.send_unlock_otp(body.into_inner()).await?;
    Ok(HttpResponse::NoContent().finish())
}

/// `POST /auth/unlock-account` — clear the lockout using the OTP.
///
/// Returns `204 No Content` on success.
#[utoipa::path(
    post,
    path = "/api/v1/auth/unlock-account",
    request_body = UnlockWithOtpRequest,
    responses(
        (status = 204, description = "Account unlocked"),
        (status = 400, description = "Invalid OTP or expired"),
    ),
    tag = "auth"
)]
pub async fn unlock_account(
    state: web::Data<AppState>,
    body:  web::Json<UnlockWithOtpRequest>,
) -> Result<HttpResponse, ApiError> {
    state.auth.unlock_account_with_otp(body.into_inner()).await?;
    Ok(HttpResponse::NoContent().finish())
}