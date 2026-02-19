//! Authentication and authorization service trait.

use async_trait::async_trait;

use crate::error::AppResult;
use crate::models::user::{Claims, LoginRequest, RegisterRequest, TokenPair, User};
use crate::types::{Action, ResourceRef, UserId};

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
            Err(crate::error::AppError::Forbidden(
                format!("user {user_id} is not authorised for action {action:?} on {resource:?}"),
            ))
        }
    }
}
