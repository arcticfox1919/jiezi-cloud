//! Authentication API handlers.
//!
//! All routes are mounted under `/api/v1/auth` by [`super::configure`].
//!
//! | Method   | Path                        | Auth? | Description                            |
//! |----------|-----------------------------|-------|----------------------------------------|
//! | POST     | `/auth/register`            | No    | Create a new user account              |
//! | POST     | `/auth/login`               | No    | Obtain a JWT token pair                |
//! | POST     | `/auth/refresh`             | No    | Rotate the refresh token               |
//! | GET      | `/auth/me`                  | Yes   | Return current user profile (full)     |
//! | PATCH    | `/auth/me`                  | Yes   | Update display name / avatar           |
//! | POST     | `/auth/me/password`         | Yes   | Change own password (old+new required) |
//! | POST     | `/auth/logout`              | No    | Revoke a refresh token                 |
//! | GET      | `/auth/sessions`            | Yes   | List active sessions                   |
//! | DELETE   | `/auth/sessions/{family}`   | Yes   | Revoke a specific session              |

use std::str::FromStr;

use actix_web::{web, HttpResponse};
use serde::Deserialize;

use jiezi_cloud_core::error::AppError;
use jiezi_cloud_core::models::user::{ChangeOwnPasswordRequest, LoginRequest, RegisterRequest, UpdateProfileRequest};
use jiezi_cloud_core::types::UserId;

use crate::error::ApiError;
use crate::middleware::auth::AuthUser;
use crate::state::AppState;

// ─── Route registration ───────────────────────────────────────────────────────

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.route("/register", web::post().to(register))
        .route("/login", web::post().to(login))
        .route("/refresh", web::post().to(refresh))
        .route("/me", web::get().to(me))
        .route("/me", web::patch().to(update_me))
        .route("/me/password", web::post().to(change_password))
        .route("/logout", web::post().to(logout))
        .route("/sessions", web::get().to(list_sessions))
        .route("/sessions/{family}", web::delete().to(revoke_session));
}

// ─── Request / response helpers ───────────────────────────────────────────────

#[derive(Deserialize)]
struct RefreshBody {
    refresh_token: String,
}

#[derive(Deserialize)]
struct LogoutBody {
    refresh_token: String,
}

// ─── Handlers ─────────────────────────────────────────────────────────────────

/// `POST /auth/register` — create a new account.
///
/// Returns `201 Created` with the [`User`] object on success.
async fn register(
    state: web::Data<AppState>,
    body: web::Json<RegisterRequest>,
) -> Result<HttpResponse, ApiError> {
    let user = state.auth.register(body.into_inner()).await?;
    Ok(HttpResponse::Created().json(user))
}

/// `POST /auth/login` — authenticate with credentials.
///
/// Returns `200 OK` with a [`TokenPair`] on success.
async fn login(
    state: web::Data<AppState>,
    body: web::Json<LoginRequest>,
) -> Result<HttpResponse, ApiError> {
    let tokens = state.auth.login(body.into_inner()).await?;
    Ok(HttpResponse::Ok().json(tokens))
}

/// `POST /auth/refresh` — exchange a refresh token for a new pair (rotation).
async fn refresh(
    state: web::Data<AppState>,
    body: web::Json<RefreshBody>,
) -> Result<HttpResponse, ApiError> {
    let tokens = state.auth.refresh_token(&body.refresh_token).await?;
    Ok(HttpResponse::Ok().json(tokens))
}

/// `GET /auth/me` — return the current user's full profile.
async fn me(
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
async fn update_me(
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
async fn change_password(
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

/// `POST /auth/logout` — revoke a refresh token (log out a device).
async fn logout(
    state: web::Data<AppState>,
    body: web::Json<LogoutBody>,
) -> Result<HttpResponse, ApiError> {
    state.auth.revoke_token(&body.refresh_token).await?;
    Ok(HttpResponse::NoContent().finish())
}

/// `GET /auth/sessions` — list all active sessions for the current user.
async fn list_sessions(
    state: web::Data<AppState>,
    auth: AuthUser,
) -> Result<HttpResponse, ApiError> {
    let user_id = parse_user_id(&auth.0.sub)?;
    let sessions = state.auth.list_sessions(&user_id).await?;
    Ok(HttpResponse::Ok().json(sessions))
}

/// `DELETE /auth/sessions/{family}` — revoke a single device session.
async fn revoke_session(
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
