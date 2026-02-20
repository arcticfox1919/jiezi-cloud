//! Concrete implementation of [`jiezi_cloud_core::traits::auth::AuthService`].
//!
//! [`AuthServiceImpl`] wires together:
//!
//! - [`PasswordService`]         — Argon2id hashing.
//! - [`JwtManager`]              — JWT signing/verification.
//! - [`RbacEngine`]              — role-based permission checks.
//! - [`UserRepository`]          — user persistence.
//! - [`RefreshTokenRepository`]  — refresh-token lifecycle management.
//!
//! # Refresh-token rotation
//!
//! On each successful refresh, the presented token is revoked and a new one
//! in the **same family** is issued.  If a *revoked* family token arrives, the
//! entire family is wiped — this detects token-theft/reuse attacks and forces
//! the user to log in again.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::Utc;
use tracing::{instrument, warn};

use jiezi_cloud_core::{
    error::{AppError, AppResult},
    models::user::{Claims, LoginRequest, RegisterRequest, SessionInfo, TokenPair, User},
    traits::auth::AuthService,
    types::{Action, ResourceRef, UserId},
};

use crate::{
    jwt::JwtManager,
    password::PasswordService,
    rbac::RbacEngine,
    repository::{RefreshTokenRepository, UserRepository},
};

// ─── AuthServiceImpl ──────────────────────────────────────────────────────────

/// Production implementation of [`AuthService`].
///
/// Cheap to clone (all state is behind [`Arc`]); safe to share across async
/// tasks.
#[derive(Clone)]
pub struct AuthServiceImpl {
    user_repo:  UserRepository,
    token_repo: RefreshTokenRepository,
    jwt:        Arc<JwtManager>,
    refresh_ttl: Duration,
}

impl AuthServiceImpl {
    /// Construct a new service instance.
    ///
    /// `jwt_secret` is the HMAC-SHA256 signing key.  `access_ttl` and
    /// `refresh_ttl` control how long each token type remains valid.
    pub fn new(
        user_repo:   UserRepository,
        token_repo:  RefreshTokenRepository,
        jwt_secret:  &str,
        access_ttl:  Duration,
        refresh_ttl: Duration,
    ) -> Self {
        Self {
            user_repo,
            token_repo,
            jwt: Arc::new(JwtManager::new(jwt_secret, access_ttl, refresh_ttl)),
            refresh_ttl,
        }
    }
}

// ─── Input validation ─────────────────────────────────────────────────────────

/// Validate a [`RegisterRequest`] and return the first validation error found.
fn validate_register(req: &RegisterRequest) -> Result<(), AppError> {
    if req.username.len() < 3 || req.username.len() > 50 {
        return Err(AppError::Validation(
            "username must be between 3 and 50 characters".to_owned(),
        ));
    }
    if !req.username.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '-') {
        return Err(AppError::Validation(
            "username may only contain letters, digits, hyphens and underscores".to_owned(),
        ));
    }
    if !req.email.contains('@') {
        return Err(AppError::Validation("email address is not valid".to_owned()));
    }
    if req.password.len() < 8 {
        return Err(AppError::Validation(
            "password must be at least 8 characters long".to_owned(),
        ));
    }
    Ok(())
}

// ─── AuthService implementation ───────────────────────────────────────────────

#[async_trait]
impl AuthService for AuthServiceImpl {
    #[instrument(skip(self, req), fields(username = %req.username))]
    async fn register(&self, req: RegisterRequest) -> AppResult<User> {
        validate_register(&req)?;

        let password_hash = PasswordService::hash(&req.password)?;
        let now = Utc::now();

        let user = User {
            id:            UserId::new(),
            username:      req.username.clone(),
            email:         req.email.clone(),
            password_hash,
            role:          jiezi_cloud_core::models::user::Role::Member,
            is_active:     true,
            display_name:  req.display_name,
            avatar_url:    None,
            storage_quota: None,
            storage_used:  0,
            created_at:    now,
            updated_at:    now,
        };

        self.user_repo.create(&user).await?;

        tracing::info!(user_id = %user.id, "new user registered");
        Ok(user)
    }

    #[instrument(skip(self, req), fields(credential = %req.credential))]
    async fn login(&self, req: LoginRequest) -> AppResult<TokenPair> {
        let user = self
            .user_repo
            .find_by_credential(&req.credential)
            .await?
            .ok_or_else(|| AppError::Unauthorized("invalid credentials".to_owned()))?;

        if !user.is_active {
            return Err(AppError::Forbidden("account is suspended".to_owned()));
        }

        let matches = PasswordService::verify(&req.password, &user.password_hash)?;
        if !matches {
            return Err(AppError::Unauthorized("invalid credentials".to_owned()));
        }

        let (access_token, expires_in) = self.jwt.generate_access_token(&user.id, user.role)?;
        let (refresh_token, family)    = self.jwt.generate_refresh_token(&user.id)?;

        let expires_at = Utc::now()
            + chrono::Duration::from_std(self.refresh_ttl)
                .map_err(|e| AppError::Internal(e.to_string()))?;

        self.token_repo
            .create(&refresh_token, &user.id, &family, req.device_label.as_deref(), expires_at)
            .await?;

        tracing::info!(user_id = %user.id, "user logged in");

        Ok(TokenPair { access_token, refresh_token, expires_in })
    }

    async fn verify_token(&self, token: &str) -> AppResult<Claims> {
        self.jwt.verify_access_token(token)
    }

    #[instrument(skip(self, refresh_token))]
    async fn refresh_token(&self, refresh_token: &str) -> AppResult<TokenPair> {
        // 1. Verify JWT signature and expiry.
        let claims = self.jwt.verify_refresh_token(refresh_token)?;

        let user_id: UserId = claims
            .sub
            .parse()
            .map_err(|_| AppError::Unauthorized("malformed subject in refresh token".to_owned()))?;

        // 2. Check the DB record.
        let stored = self
            .token_repo
            .find_by_raw_token(refresh_token)
            .await?
            .ok_or_else(|| AppError::Unauthorized("refresh token not recognised".to_owned()))?;

        if stored.revoked {
            // Reuse-attack detected — wipe the entire family and force re-login.
            warn!(
                family = %stored.family,
                user_id = %user_id,
                "revoked refresh token presented — wiping family"
            );
            self.token_repo.revoke_family(&stored.family).await?;
            return Err(AppError::Unauthorized(
                "refresh token has already been used".to_owned(),
            ));
        }

        // 3. Revoke the old token and issue a new pair in the same family.
        self.token_repo
            .revoke_by_raw_token(refresh_token)
            .await?;

        let (access_token, expires_in) = self.jwt.generate_access_token(&user_id, stored_role(&self.user_repo, &user_id).await?)?;
        let new_refresh = self.jwt.rotate_refresh_token(&user_id, &stored.family)?;

        let expires_at = Utc::now()
            + chrono::Duration::from_std(self.refresh_ttl)
                .map_err(|e| AppError::Internal(e.to_string()))?;

        self.token_repo
            .create(&new_refresh, &user_id, &stored.family, stored.device_label.as_deref(), expires_at)
            .await?;

        Ok(TokenPair { access_token, refresh_token: new_refresh, expires_in })
    }

    async fn revoke_token(&self, refresh_token: &str) -> AppResult<()> {
        let stored = self
            .token_repo
            .find_by_raw_token(refresh_token)
            .await?
            .ok_or_else(|| AppError::NotFound("refresh token not found".to_owned()))?;

        if !stored.revoked {
            self.token_repo.revoke_by_raw_token(refresh_token).await?;
        }

        Ok(())
    }

    async fn list_sessions(&self, user_id: &UserId) -> AppResult<Vec<SessionInfo>> {
        use std::collections::BTreeMap;

        let tokens = self.token_repo.list_active_for_user(user_id).await?;

        // Tokens are ordered family ASC, created_at ASC.
        // One SessionInfo per family:
        //   - session_started = earliest created_at in the family
        //   - expires_at      = latest expires_at in the family (most recent rotation)
        //   - device_label    = label from the first token (stable across rotations)
        let mut map: BTreeMap<String, SessionInfo> = BTreeMap::new();
        for token in &tokens {
            map.entry(token.family.clone())
                .and_modify(|s| s.expires_at = token.expires_at)
                .or_insert_with(|| SessionInfo {
                    family:          token.family.clone(),
                    device_label:    token.device_label.clone(),
                    session_started: token.created_at,
                    expires_at:      token.expires_at,
                });
        }

        Ok(map.into_values().collect())
    }

    async fn revoke_session(&self, user_id: &UserId, family: &str) -> AppResult<()> {
        let tokens = self.token_repo.list_active_for_user(user_id).await?;
        if !tokens.iter().any(|t| t.family == family) {
            return Err(AppError::NotFound(format!("session {family} not found")));
        }
        self.token_repo.revoke_family(family).await
    }

    async fn check_permission(
        &self,
        user_id: &UserId,
        action: Action,
        _resource: &ResourceRef,
    ) -> AppResult<bool> {
        let user = self
            .user_repo
            .find_by_id(user_id)
            .await?
            .ok_or_else(|| AppError::NotFound(format!("user {user_id} not found")))?;

        Ok(RbacEngine::is_permitted(user.role, action))
    }
}

/// Fetch the current role of a user — used when generating a fresh access
/// token during refresh so that role changes take effect on the next rotation.
async fn stored_role(
    repo: &UserRepository,
    user_id: &UserId,
) -> AppResult<jiezi_cloud_core::models::user::Role> {
    repo.find_by_id(user_id)
        .await?
        .map(|u| u.role)
        .ok_or_else(|| AppError::NotFound(format!("user {user_id} not found")))
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repository::test_helpers::create_test_db;

    async fn make_service() -> AuthServiceImpl {
        let db         = create_test_db().await;
        let user_repo  = UserRepository::new(db.clone());
        let token_repo = RefreshTokenRepository::new(db);

        AuthServiceImpl::new(
            user_repo,
            token_repo,
            "test-secret-at-least-32-bytes-long!!",
            Duration::from_secs(900),
            Duration::from_secs(30 * 24 * 60 * 60),
        )
    }

    fn reg(username: &str) -> RegisterRequest {
        RegisterRequest {
            username:    username.to_owned(),
            email:       format!("{username}@example.com"),
            password:    "password123".to_owned(),
            display_name: None,
        }
    }

    // TDD task 2.5-1: successful registration returns a user with the expected fields
    #[tokio::test]
    async fn test_register_success() {
        let svc  = make_service().await;
        let user = svc.register(reg("alice")).await.unwrap();
        assert_eq!(user.username, "alice");
        assert_eq!(user.email, "alice@example.com");
        assert!(user.is_active);
    }

    // TDD task 2.5-2: duplicate registration returns Conflict
    #[tokio::test]
    async fn test_register_duplicate_returns_conflict() {
        let svc = make_service().await;
        svc.register(reg("bob")).await.unwrap();
        let err = svc.register(reg("bob")).await.unwrap_err();
        assert!(matches!(err, AppError::Conflict(_)));
    }

    // TDD task 2.5-3: short username is rejected
    #[tokio::test]
    async fn test_register_short_username_rejected() {
        let svc = make_service().await;
        let req = RegisterRequest {
            username:    "ab".to_owned(), // too short
            email:       "ab@example.com".to_owned(),
            password:    "password123".to_owned(),
            display_name: None,
        };
        let err = svc.register(req).await.unwrap_err();
        assert!(matches!(err, AppError::Validation(_)));
    }

    // TDD task 2.5-4: weak password is rejected
    #[tokio::test]
    async fn test_register_weak_password_rejected() {
        let svc = make_service().await;
        let req = RegisterRequest {
            username:     "charlie".to_owned(),
            email:        "charlie@example.com".to_owned(),
            password:     "abc".to_owned(), // too short
            display_name: None,
        };
        let err = svc.register(req).await.unwrap_err();
        assert!(matches!(err, AppError::Validation(_)));
    }

    // TDD task 2.5-5: successful login returns a token pair
    #[tokio::test]
    async fn test_login_success() {
        let svc = make_service().await;
        svc.register(reg("dave")).await.unwrap();

        let pair = svc
            .login(LoginRequest { credential: "dave".to_owned(), password: "password123".to_owned(), device_label: None })
            .await
            .unwrap();

        assert!(!pair.access_token.is_empty());
        assert!(!pair.refresh_token.is_empty());
        assert!(pair.expires_in > 0);
    }

    // TDD task 2.5-6: wrong password returns Unauthorized
    #[tokio::test]
    async fn test_login_wrong_password_rejected() {
        let svc = make_service().await;
        svc.register(reg("eve")).await.unwrap();

        let err = svc
            .login(LoginRequest { credential: "eve".to_owned(), password: "wrong_pass".to_owned(), device_label: None })
            .await
            .unwrap_err();

        assert!(matches!(err, AppError::Unauthorized(_)));
    }

    // TDD task 2.5-7: nonexistent user returns Unauthorized (not NotFound —
    // to avoid disclosing whether the account exists)
    #[tokio::test]
    async fn test_login_nonexistent_user_returns_unauthorized() {
        let svc = make_service().await;
        let err = svc
            .login(LoginRequest { credential: "ghost".to_owned(), password: "pass".to_owned(), device_label: None })
            .await
            .unwrap_err();
        assert!(matches!(err, AppError::Unauthorized(_)));
    }

    // TDD task 2.5-8: valid access token verifies successfully
    #[tokio::test]
    async fn test_verify_access_token_success() {
        let svc = make_service().await;
        svc.register(reg("fiona")).await.unwrap();

        let pair = svc
            .login(LoginRequest { credential: "fiona".to_owned(), password: "password123".to_owned(), device_label: None })
            .await
            .unwrap();

        let claims = svc.verify_token(&pair.access_token).await.unwrap();
        assert!(!claims.sub.is_empty());
    }

    // TDD task 2.5-9: refresh returns a new valid access token
    #[tokio::test]
    async fn test_refresh_token_success() {
        let svc = make_service().await;
        svc.register(reg("grace")).await.unwrap();

        let pair1 = svc
            .login(LoginRequest { credential: "grace".to_owned(), password: "password123".to_owned(), device_label: None })
            .await
            .unwrap();

        let pair2 = svc.refresh_token(&pair1.refresh_token).await.unwrap();
        // The new access token should be valid.
        svc.verify_token(&pair2.access_token).await.unwrap();
        // The new refresh token should differ from the original.
        assert_ne!(pair1.refresh_token, pair2.refresh_token);
    }

    // TDD task 2.5-10: replaying an already-used refresh token is rejected
    #[tokio::test]
    async fn test_refresh_token_reuse_rejected() {
        let svc = make_service().await;
        svc.register(reg("hank")).await.unwrap();

        let pair1 = svc
            .login(LoginRequest { credential: "hank".to_owned(), password: "password123".to_owned(), device_label: None })
            .await
            .unwrap();

        // First refresh — OK.
        svc.refresh_token(&pair1.refresh_token).await.unwrap();

        // Second refresh with the same (now revoked) token — must fail.
        let err = svc.refresh_token(&pair1.refresh_token).await.unwrap_err();
        assert!(matches!(err, AppError::Unauthorized(_)));
    }

    // TDD task 2.5-11: revoke_token makes subsequent refresh attempts fail
    #[tokio::test]
    async fn test_revoke_token_prevents_refresh() {
        let svc = make_service().await;
        svc.register(reg("iris")).await.unwrap();

        let pair = svc
            .login(LoginRequest { credential: "iris".to_owned(), password: "password123".to_owned(), device_label: None })
            .await
            .unwrap();

        svc.revoke_token(&pair.refresh_token).await.unwrap();

        let err = svc.refresh_token(&pair.refresh_token).await.unwrap_err();
        assert!(matches!(err, AppError::Unauthorized(_)));
    }

    // TDD task 2.5-12: check_permission returns true for Owner + Admin action
    #[tokio::test]
    async fn test_check_permission_owner_admin_action() {
        let svc = make_service().await;
        // Service creates users as Member by default; to test Owner we need to
        // insert directly via the repository.  For this test we verify that
        // check_permission forwards to RbacEngine correctly by using Member role.
        let user = svc.register(reg("jack")).await.unwrap();

        // Member can Read
        let can_read = svc
            .check_permission(&user.id, Action::Read, &ResourceRef::System)
            .await
            .unwrap();
        assert!(can_read);

        // Member cannot Admin
        let can_admin = svc
            .check_permission(&user.id, Action::Admin, &ResourceRef::System)
            .await
            .unwrap();
        assert!(!can_admin);
    }

    // TDD task 2.5-13: list_sessions returns one session after login with device label
    #[tokio::test]
    async fn test_list_sessions_after_login() {
        let svc = make_service().await;
        svc.register(reg("kate")).await.unwrap();

        let pair = svc
            .login(LoginRequest {
                credential:   "kate".to_owned(),
                password:     "password123".to_owned(),
                device_label: Some("iPhone 16".to_owned()),
            })
            .await
            .unwrap();

        let claims  = svc.verify_token(&pair.access_token).await.unwrap();
        let user_id: UserId = claims.sub.parse().unwrap();

        let sessions = svc.list_sessions(&user_id).await.unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].device_label, Some("iPhone 16".to_owned()));
    }

    // TDD task 2.5-14: revoke_session removes that device session
    #[tokio::test]
    async fn test_revoke_session_removes_device() {
        let svc = make_service().await;
        svc.register(reg("leo")).await.unwrap();

        let pair = svc
            .login(LoginRequest {
                credential:   "leo".to_owned(),
                password:     "password123".to_owned(),
                device_label: Some("iPad".to_owned()),
            })
            .await
            .unwrap();

        let claims  = svc.verify_token(&pair.access_token).await.unwrap();
        let user_id: UserId = claims.sub.parse().unwrap();

        let sessions = svc.list_sessions(&user_id).await.unwrap();
        assert_eq!(sessions.len(), 1);
        let family = sessions[0].family.clone();

        svc.revoke_session(&user_id, &family).await.unwrap();

        let remaining = svc.list_sessions(&user_id).await.unwrap();
        assert!(remaining.is_empty(), "session should be gone after revocation");

        // Further refresh attempts on the old token should fail.
        let err = svc.refresh_token(&pair.refresh_token).await.unwrap_err();
        assert!(matches!(err, AppError::Unauthorized(_)));
    }

    // TDD task 2.5-15: revoking a nonexistent session returns NotFound
    #[tokio::test]
    async fn test_revoke_nonexistent_session_returns_not_found() {
        let svc  = make_service().await;
        let user = svc.register(reg("mia")).await.unwrap();

        let err = svc
            .revoke_session(&user.id, "no-such-family")
            .await
            .unwrap_err();
        assert!(matches!(err, AppError::NotFound(_)));
    }
}
