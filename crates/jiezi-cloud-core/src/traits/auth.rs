//! Authentication and authorization service trait.

use async_trait::async_trait;

use crate::error::AppResult;
use crate::models::user::{
    AdminResetPasswordRequest, ChangeOwnPasswordRequest, ChangeRoleRequest,
    Claims, LoginRequest, RegisterRequest, Role, SessionInfo, SetActiveRequest,
    SetQuotaRequest, TokenPair, UpdateProfileRequest, User,
};
use crate::types::{Action, PageRequest, PageResponse, ResourceRef, UserId};

/// Authentication and authorization service contract.
///
/// Provides user management, JWT token lifecycle, and RBAC-based permission
/// checks.  The concrete implementation lives in the `jiezi-cloud-auth` crate.
///
/// # Token Strategy
///
/// - **Access tokens** are short-lived JWT tokens (default: 15 min).
/// - **Refresh tokens** are longer-lived (default: 7 days) and stored server-
///   side so they can be revoked.  On each refresh the old token is revoked and
///   a new pair is issued (token rotation).  Replaying a revoked refresh token
///   immediately invalidates the entire token family (reuse detection).
#[async_trait]
pub trait AuthService: Send + Sync {
    /// Create a new user account.
    ///
    /// # Errors
    ///
    /// - [`AppError::Conflict`] if the username or email is already taken.
    /// - [`AppError::Validation`] if the request payload is invalid.
    async fn register(&self, req: RegisterRequest) -> AppResult<User>;

    /// Authenticate with credentials and return a new token pair on success.
    ///
    /// # Errors
    ///
    /// - [`AppError::Unauthorized`] if the credentials are incorrect.
    /// - [`AppError::Forbidden`] if the account is suspended.
    async fn login(&self, req: LoginRequest) -> AppResult<TokenPair>;

    /// Validate an access token and extract its embedded claims.
    ///
    /// # Errors
    ///
    /// - [`AppError::Unauthorized`] if the token is malformed, expired,
    ///   or signed with an unknown key.
    async fn verify_token(&self, token: &str) -> AppResult<Claims>;

    /// Exchange a valid refresh token for a new [`TokenPair`].
    ///
    /// The supplied refresh token is revoked after this call (rotation).
    ///
    /// # Errors
    ///
    /// - [`AppError::Unauthorized`] if the refresh token is invalid, expired,
    ///   or has already been used (reuse attack detected).
    async fn refresh_token(&self, refresh_token: &str) -> AppResult<TokenPair>;

    /// Revoke a refresh token (logout).
    ///
    /// All future attempts to use this token will fail.
    ///
    /// # Errors
    ///
    /// - [`AppError::NotFound`] if the token does not exist.
    async fn revoke_token(&self, refresh_token: &str) -> AppResult<()>;

    /// List all active (non-revoked, non-expired) sessions for a user.
    ///
    /// Each entry in the returned list corresponds to a distinct device /
    /// browser session.  The `family` field can be passed to
    /// [`AuthService::revoke_session`] to log out a specific device.
    async fn list_sessions(&self, user_id: &UserId) -> AppResult<Vec<SessionInfo>>;

    /// Revoke all refresh tokens belonging to a specific session family,
    /// effectively logging out a single device.
    ///
    /// # Errors
    ///
    /// - [`AppError::NotFound`] if no active session with `family` exists for the user.
    async fn revoke_session(&self, user_id: &UserId, family: &str) -> AppResult<()>;

    // ─── User lookup ──────────────────────────────────────────────────────────

    /// Return a single user by ID.
    ///
    /// # Errors
    ///
    /// - [`AppError::NotFound`] if no user with `user_id` exists.
    async fn get_user(&self, user_id: &UserId) -> AppResult<User>;

    /// Return a paginated list of all users.
    ///
    /// Intended for admin dashboards.  Callers are responsible for asserting
    /// the necessary role before invoking this method.
    async fn list_users(&self, page: &PageRequest) -> AppResult<PageResponse<User>>;

    // ─── Self-service ─────────────────────────────────────────────────────────

    /// Update the caller's own profile (display name and / or avatar URL).
    ///
    /// Only the fields wrapped in `Some` are written; `None` fields are left
    /// unchanged.  Pass `Some(None)` to explicitly clear a field.
    async fn update_profile(
        &self,
        user_id: &UserId,
        req:     UpdateProfileRequest,
    ) -> AppResult<User>;

    /// Change the caller's own password after verifying the current one.
    ///
    /// # Errors
    ///
    /// - [`AppError::Unauthorized`] if `old_password` is incorrect.
    /// - [`AppError::Validation`] if `new_password` is shorter than 8 chars.
    async fn change_own_password(
        &self,
        user_id: &UserId,
        req:     ChangeOwnPasswordRequest,
    ) -> AppResult<()>;

    // ─── Admin operations ─────────────────────────────────────────────────────

    /// Change the system-level role of a user.
    ///
    /// Authorization rules (enforced by the implementation):
    /// - `Owner` may grant any role, including `Admin`.
    /// - `Admin` may only promote/demote between `Member` and `Guest`.
    /// - Nobody may change their own role.
    /// - Nobody may demote another `Owner` unless they are also an `Owner`.
    ///
    /// # Errors
    ///
    /// - [`AppError::Forbidden`] if the caller lacks the privilege.
    /// - [`AppError::NotFound`] if `target_id` does not exist.
    async fn update_user_role(
        &self,
        caller_role: Role,
        caller_id:   &UserId,
        target_id:   &UserId,
        req:         ChangeRoleRequest,
    ) -> AppResult<User>;

    /// Suspend or reactivate a user account.
    ///
    /// A suspended user cannot log in; existing sessions remain valid until
    /// their tokens expire or are explicitly revoked.
    ///
    /// # Errors
    ///
    /// - [`AppError::NotFound`] if `target_id` does not exist.
    /// - [`AppError::Forbidden`] if an Admin attempts to suspend an Owner.
    async fn set_user_active(
        &self,
        caller_role: Role,
        target_id:   &UserId,
        req:         SetActiveRequest,
    ) -> AppResult<()>;

    /// Admin-force-reset a user's password without knowing the old one.
    ///
    /// # Errors
    ///
    /// - [`AppError::NotFound`] if `target_id` does not exist.
    /// - [`AppError::Validation`] if `new_password` is too short.
    async fn admin_reset_password(
        &self,
        target_id: &UserId,
        req:       AdminResetPasswordRequest,
    ) -> AppResult<()>;

    /// Set (or remove) the per-user storage quota.
    ///
    /// `None` quota means unlimited storage.
    ///
    /// # Errors
    ///
    /// - [`AppError::NotFound`] if `target_id` does not exist.
    async fn update_user_quota(
        &self,
        target_id: &UserId,
        req:       SetQuotaRequest,
    ) -> AppResult<()>;

    /// Permanently delete a user account and all associated sessions.
    ///
    /// Does **not** cascade to files — callers must handle file cleanup
    /// separately.  Only `Owner` should be permitted to call this.
    ///
    /// # Errors
    ///
    /// - [`AppError::NotFound`] if `target_id` does not exist.
    /// - [`AppError::Forbidden`] if attempting to delete the last Owner.
    async fn delete_user(&self, target_id: &UserId) -> AppResult<()>;

    /// Create the initial Owner account during the first-run setup wizard.
    ///
    /// Fails with [`AppError::Conflict`] if an Owner account already exists,
    /// preventing accidental re-initialisation after setup is complete.
    ///
    /// The owner is granted `Role::Owner` and `is_active = true`.
    async fn bootstrap_owner(
        &self,
        username: String,
        email: String,
        password: String,
        display_name: Option<String>,
    ) -> AppResult<User>;

    /// Change the password for a user by ID.
    ///
    /// This is a force-change (no old-password verification) intended for the
    /// setup wizard and future admin-reset flows.
    ///
    /// # Errors
    ///
    /// - [`AppError::NotFound`] if `user_id` does not exist.
    /// - [`AppError::Validation`] if `new_password` is too short (< 8 chars).
    async fn change_password(&self, user_id: &UserId, new_password: &str) -> AppResult<()>;

    /// Check whether a user is authorised to perform an action on a resource.
    ///
    /// Returns `Ok(true)` if the action is permitted, `Ok(false)` otherwise.
    /// Use this for conditional logic; prefer [`AuthService::assert_permission`]
    /// when you want to short-circuit with an error on denial.
    async fn check_permission(
        &self,
        user_id: &UserId,
        action: Action,
        resource: &ResourceRef,
    ) -> AppResult<bool>;

    /// Assert that a user is authorised to perform an action on a resource.
    ///
    /// # Errors
    ///
    /// - [`AppError::Forbidden`] if the action is not permitted.
    async fn assert_permission(
        &self,
        user_id: &UserId,
        action: Action,
        resource: &ResourceRef,
    ) -> AppResult<()> {
        if self.check_permission(user_id, action, resource).await? {
            Ok(())
        } else {
            Err(crate::error::AppError::Forbidden(format!(
                "user {user_id} is not authorised for action {action:?} on {resource:?}"
            )))
        }
    }
}
