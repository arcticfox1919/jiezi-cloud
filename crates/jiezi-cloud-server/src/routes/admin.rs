//! Administrator user-management API handlers.
//!
//! All routes are mounted under `/api/v1/admin` by [`super::configure`].
//! Every endpoint requires a valid Bearer token (via [`AuthUser`]) **and** the
//! caller must hold at least `Role::Admin`.  Some operations additionally
//! require `Role::Owner` (noted below).
//!
//! | Method | Path                               | Min role | Description                     |
//! |--------|------------------------------------|----------|---------------------------------|
//! | GET    | `/admin/users`                     | Admin    | Paginated user list             |
//! | GET    | `/admin/users/{id}`                | Admin    | Fetch a single user             |
//! | PATCH  | `/admin/users/{id}/role`           | Admin    | Change a user's system role     |
//! | PATCH  | `/admin/users/{id}/status`         | Admin    | Suspend / reactivate a user     |
//! | POST   | `/admin/users/{id}/reset-password` | Admin    | Force-reset a user's password   |
//! | PATCH  | `/admin/users/{id}/quota`          | Owner    | Set / clear a user's quota      |
//! | DELETE | `/admin/users/{id}`                | Owner    | Permanently delete a user       |

use std::str::FromStr;

use actix_web::{web, HttpResponse};

use jiezi_cloud_core::error::AppError;
use jiezi_cloud_core::models::user::{
    AdminResetPasswordRequest, ChangeRoleRequest, Role, SetActiveRequest, SetQuotaRequest,
};
use jiezi_cloud_core::types::{PageRequest, UserId};

use crate::error::ApiError;
use crate::middleware::auth::AuthUser;
use crate::state::AppState;

// ─── Route registration ───────────────────────────────────────────────────────

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg
        // List / get
        .route("/users",                          web::get().to(list_users))
        .route("/users/{id}",                     web::get().to(get_user))
        // Modification
        .route("/users/{id}/role",                web::patch().to(change_role))
        .route("/users/{id}/status",              web::patch().to(set_status))
        .route("/users/{id}/reset-password",      web::post().to(reset_password))
        // Owner-only
        .route("/users/{id}/quota",               web::patch().to(set_quota))
        .route("/users/{id}",                     web::delete().to(delete_user));
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

/// Parse a `UserId` from a path segment, returning a typed API error on failure.
fn parse_user_id(s: &str) -> Result<UserId, ApiError> {
    UserId::from_str(s).map_err(|_| {
        ApiError(AppError::Validation(format!("'{s}' is not a valid user ID")))
    })
}

/// Require the caller to be at least `Admin`.  Returns `403` otherwise.
fn require_admin(role: Role) -> Result<(), ApiError> {
    match role {
        Role::Owner | Role::Admin => Ok(()),
        _ => Err(ApiError(AppError::Forbidden(
            "this endpoint requires admin or owner privileges".into(),
        ))),
    }
}

/// Require the caller to be `Owner`.  Returns `403` otherwise.
fn require_owner(role: Role) -> Result<(), ApiError> {
    match role {
        Role::Owner => Ok(()),
        _ => Err(ApiError(AppError::Forbidden(
            "this endpoint requires owner privileges".into(),
        ))),
    }
}

// ─── Handlers ─────────────────────────────────────────────────────────────────

/// `GET /admin/users?page=1&per_page=20`
///
/// Returns a paginated list of all users.
/// Requires `Admin` or `Owner`.
async fn list_users(
    state: web::Data<AppState>,
    auth:  AuthUser,
    query: web::Query<PageQueryParams>,
) -> Result<HttpResponse, ApiError> {
    require_admin(auth.0.role)?;

    let page = PageRequest {
        page:     query.page.unwrap_or(1).max(1),
        per_page: query.per_page.unwrap_or(20).min(200).max(1),
    };

    let result = state.auth.list_users(&page).await.map_err(ApiError)?;
    Ok(HttpResponse::Ok().json(result))
}

/// `GET /admin/users/{id}`
///
/// Returns the full user record for the given ID.
/// Requires `Admin` or `Owner`.
async fn get_user(
    state: web::Data<AppState>,
    auth:  AuthUser,
    path:  web::Path<String>,
) -> Result<HttpResponse, ApiError> {
    require_admin(auth.0.role)?;

    let target_id = parse_user_id(&path.into_inner())?;
    let user = state.auth.get_user(&target_id).await.map_err(ApiError)?;
    Ok(HttpResponse::Ok().json(user))
}

/// `PATCH /admin/users/{id}/role`
///
/// Change the system-level role of a user.
///
/// Authorization rules (enforced by the service layer):
/// - Owner may grant any role.
/// - Admin may only promote/demote between `Member` and `Guest`.
/// - Nobody may change their own role.
async fn change_role(
    state: web::Data<AppState>,
    auth:  AuthUser,
    path:  web::Path<String>,
    body:  web::Json<ChangeRoleRequest>,
) -> Result<HttpResponse, ApiError> {
    require_admin(auth.0.role)?;

    let caller_id = UserId::from_str(&auth.0.sub)
        .map_err(|_| ApiError(AppError::Internal("invalid caller ID in token".into())))?;
    let target_id = parse_user_id(&path.into_inner())?;

    let updated = state
        .auth
        .update_user_role(auth.0.role, &caller_id, &target_id, body.into_inner())
        .await
        .map_err(ApiError)?;

    Ok(HttpResponse::Ok().json(updated))
}

/// `PATCH /admin/users/{id}/status`
///
/// Suspend (`is_active: false`) or reactivate (`is_active: true`) a user.
/// Admin cannot suspend an Owner.
async fn set_status(
    state: web::Data<AppState>,
    auth:  AuthUser,
    path:  web::Path<String>,
    body:  web::Json<SetActiveRequest>,
) -> Result<HttpResponse, ApiError> {
    require_admin(auth.0.role)?;

    let target_id = parse_user_id(&path.into_inner())?;

    state
        .auth
        .set_user_active(auth.0.role, &target_id, body.into_inner())
        .await
        .map_err(ApiError)?;

    Ok(HttpResponse::NoContent().finish())
}

/// `POST /admin/users/{id}/reset-password`
///
/// Force-reset a user's password without knowing their current one.
/// Requires `Admin` or `Owner`.
async fn reset_password(
    state: web::Data<AppState>,
    auth:  AuthUser,
    path:  web::Path<String>,
    body:  web::Json<AdminResetPasswordRequest>,
) -> Result<HttpResponse, ApiError> {
    require_admin(auth.0.role)?;

    let target_id = parse_user_id(&path.into_inner())?;

    state
        .auth
        .admin_reset_password(&target_id, body.into_inner())
        .await
        .map_err(ApiError)?;

    Ok(HttpResponse::NoContent().finish())
}

/// `PATCH /admin/users/{id}/quota`
///
/// Set or clear a user's storage quota.
/// Requires `Owner` (only the owner controls quotas).
async fn set_quota(
    state: web::Data<AppState>,
    auth:  AuthUser,
    path:  web::Path<String>,
    body:  web::Json<SetQuotaRequest>,
) -> Result<HttpResponse, ApiError> {
    require_owner(auth.0.role)?;

    let target_id = parse_user_id(&path.into_inner())?;

    state
        .auth
        .update_user_quota(&target_id, body.into_inner())
        .await
        .map_err(ApiError)?;

    Ok(HttpResponse::NoContent().finish())
}

/// `DELETE /admin/users/{id}`
///
/// Permanently delete a user account and revoke all their sessions.
/// Requires `Owner`.  Cannot delete the last owner account.
async fn delete_user(
    state: web::Data<AppState>,
    auth:  AuthUser,
    path:  web::Path<String>,
) -> Result<HttpResponse, ApiError> {
    require_owner(auth.0.role)?;

    let target_id = parse_user_id(&path.into_inner())?;

    state.auth.delete_user(&target_id).await.map_err(ApiError)?;

    Ok(HttpResponse::NoContent().finish())
}

// ─── Query parameter types ────────────────────────────────────────────────────

/// Query parameters for the list endpoint.
#[derive(serde::Deserialize)]
struct PageQueryParams {
    page:     Option<u32>,
    per_page: Option<u32>,
}
