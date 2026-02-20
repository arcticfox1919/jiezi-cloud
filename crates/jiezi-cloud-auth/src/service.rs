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
    models::user::{
        Claims, LoginRequest, RegisterRequest, ResetPasswordWithOtpRequest,
        SendOtpRequest, SessionInfo, TokenPair, UnlockWithOtpRequest, User,
    },
    traits::auth::AuthService,
    types::{Action, ResourceRef, UserId},
};

use crate::{
    email::EmailService,
    jwt::JwtManager,
    password::PasswordService,
    rbac::RbacEngine,
    repository::{EmailOtpRepository, RefreshTokenRepository, UserRepository},
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
    /// Maximum consecutive failed logins before account lockout.  0 = disabled.
    max_login_attempts:   u32,
    /// How long (seconds) an account stays locked after exceeding the limit.
    lockout_duration_secs: u64,
    /// Maximum password byte length accepted before Argon2 work begins.
    max_password_bytes:   usize,
    // ─ Email OTP (None = feature disabled) ────────────────────────────────
    email_repo:                  Option<EmailOtpRepository>,
    email_svc:                   Option<Arc<EmailService>>,
    /// If true, registration requires a valid OTP code.
    email_verification_required: bool,
    /// How long (seconds) each OTP stays valid.  Default: 600 (10 min).
    email_otp_ttl_secs:          u64,
}

impl AuthServiceImpl {
    /// Construct a new service instance.
    ///
    /// `jwt` is a pre-constructed [`JwtManager`] (ES256 private key already
    /// loaded).  `refresh_ttl` controls how long refresh tokens remain valid;
    /// the access-token TTL is embedded in the `JwtManager`.
    pub fn new(
        user_repo:   UserRepository,
        token_repo:  RefreshTokenRepository,
        jwt:         Arc<JwtManager>,
        refresh_ttl: Duration,
    ) -> Self {
        Self {
            user_repo,
            token_repo,
            jwt,
            refresh_ttl,
            max_login_attempts:    5,
            lockout_duration_secs: 900,
            max_password_bytes:    128,
            email_repo:                  None,
            email_svc:                   None,
            email_verification_required: false,
            email_otp_ttl_secs:          600,
        }
    }

    /// Override the brute-force lockout policy (called from server startup).
    pub fn with_security(
        mut self,
        max_login_attempts:    u32,
        lockout_duration_secs: u64,
        max_password_bytes:    usize,
    ) -> Self {
        self.max_login_attempts    = max_login_attempts;
        self.lockout_duration_secs = lockout_duration_secs;
        self.max_password_bytes    = max_password_bytes;
        self
    }

    /// Enable email OTP (called from server startup when
    /// `email.enabled = true` or `verification_required = true` in config).
    pub fn with_email(
        mut self,
        required:      bool,
        repo:          EmailOtpRepository,
        svc:           Arc<EmailService>,
        otp_ttl_secs:  u64,
    ) -> Self {
        self.email_verification_required = required;
        self.email_repo                  = Some(repo);
        self.email_svc                   = Some(svc);
        self.email_otp_ttl_secs          = otp_ttl_secs;
        self
    }
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

/// Generate a 6-digit numeric OTP (zero-padded).
///
/// Uses a UUID v4 value as an entropy source \u2014 no extra dependencies.
fn generate_otp() -> String {
    use uuid::Uuid;
    let n = u64::from_str_radix(&Uuid::new_v4().simple().to_string()[..15], 16)
        .unwrap_or(0);
    format!("{:06}", n % 1_000_000)
}

// ─── Input validation ─────────────────────────────────────────────────────────

/// Validate a [`RegisterRequest`] and return the first validation error found.
fn validate_register(req: &RegisterRequest, max_password_bytes: usize) -> Result<(), AppError> {
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
    if req.password.len() > max_password_bytes {
        return Err(AppError::Validation(format!(
            "password must not exceed {max_password_bytes} bytes"
        )));
    }
    Ok(())
}

// ─── AuthService implementation ───────────────────────────────────────────────

#[async_trait]
impl AuthService for AuthServiceImpl {
    #[instrument(skip(self, req), fields(username = %req.username))]
    async fn register(&self, req: RegisterRequest) -> AppResult<User> {
        validate_register(&req, self.max_password_bytes)?;

        // When email verification is required, the client must supply the
        // OTP code that was emailed via `POST /auth/send-register-otp`.
        // Verify and consume it BEFORE creating the user row so we never
        // store an account that cannot subsequently log in.
        if self.email_verification_required {
            let email_repo = self
                .email_repo
                .as_ref()
                .ok_or_else(|| AppError::Internal("email OTP not configured".into()))?;

            let code = req.email_otp.as_deref().unwrap_or("").trim().to_owned();
            if code.is_empty() {
                return Err(AppError::Validation(
                    "email_otp is required when email verification is enabled".into(),
                ));
            }
            let row = email_repo
                .find_valid(&req.email, &code, "register")
                .await?
                .ok_or_else(|| {
                    AppError::Validation(
                        "email OTP is invalid, expired, or already used".into(),
                    )
                })?;
            email_repo.mark_used(&row.id).await?;
        }

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
            failed_login_count:  0,
            locked_until:        None,
            last_failed_login_at: None,
            // The account is always fully verified at creation:
            //   - verification_required=true  → OTP was checked above.
            //   - verification_required=false → verification is skipped.
            email_verified: true,
            created_at:    now,
            updated_at:    now,
        };

        self.user_repo.create(&user).await?;

        tracing::info!(user_id = %user.id, "new user registered");
        Ok(user)
    }

    #[instrument(skip(self, req), fields(credential = %req.credential))]
    async fn login(&self, req: LoginRequest) -> AppResult<TokenPair> {
        // ── Argon2 DoS guard: reject oversized passwords before hashing ────────────
        if req.password.len() > self.max_password_bytes {
            return Err(AppError::Unauthorized("invalid credentials".to_owned()));
        }

        // ── Look up user ──────────────────────────────────────────────────────────────
        // Return the same 401 for "no such user" and "wrong password" to
        // prevent username enumeration via timing or error messages.
        let user = self
            .user_repo
            .find_by_credential(&req.credential)
            .await?
            .ok_or_else(|| AppError::Unauthorized("invalid credentials".to_owned()))?;

        // ── Suspension check ─────────────────────────────────────────────────────────
        if !user.is_active {
            return Err(AppError::Forbidden("account is suspended".to_owned()));
        }
        // ── Email verification check ──────────────────────────────────────────────
        if self.email_verification_required && !user.email_verified {
            return Err(AppError::Forbidden(
                "please verify your email address before logging in".to_owned(),
            ));
        }
        // ── Lockout check ──────────────────────────────────────────────────────────
        // Check before doing any crypto work so a locked account doesn't burn
        // Argon2 CPU time even on repeated attempts.
        if let Some(locked_until) = user.locked_until {
            if locked_until > Utc::now() {
                let remaining = (locked_until - Utc::now()).num_seconds().max(0);
                warn!(
                    user_id  = %user.id,
                    username = %user.username,
                    remaining_secs = remaining,
                    "login rejected: account is locked",
                );
                return Err(AppError::Forbidden(format!(
                    "account is temporarily locked due to too many failed attempts; \
                     try again in {remaining} seconds"
                )));
            }
            // Lock has expired — let the attempt proceed.
        }

        // ── Password verification ────────────────────────────────────────────────────
        let matches = PasswordService::verify(&req.password, &user.password_hash)?;
        if !matches {
            // Track the failure and potentially trigger lockout.
            let new_count = self.user_repo.record_failed_login(&user.id).await?;

            if self.max_login_attempts > 0 && new_count >= self.max_login_attempts as i32 {
                let until = Utc::now()
                    + chrono::Duration::seconds(self.lockout_duration_secs as i64);
                self.user_repo.lock_account(&user.id, until).await?;
                warn!(
                    user_id          = %user.id,
                    username         = %user.username,
                    failed_count     = new_count,
                    locked_until_secs = self.lockout_duration_secs,
                    "account locked after too many failed login attempts",
                );
            } else {
                warn!(
                    user_id      = %user.id,
                    username     = %user.username,
                    failed_count = new_count,
                    "failed login attempt",
                );
            }
            // Always return the same generic error to prevent user enumeration.
            return Err(AppError::Unauthorized("invalid credentials".to_owned()));
        }

        // ── Success: reset failure counter, issue tokens ────────────────────────
        self.user_repo.reset_failed_login(&user.id).await?;

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

    async fn bootstrap_owner(
        &self,
        username: String,
        email: String,
        password: String,
        display_name: Option<String>,
    ) -> AppResult<User> {
        // Guard: fail if any owner account already exists.
        let owner_count = self.user_repo.count_by_role("owner").await?;
        if owner_count > 0 {
            return Err(AppError::Conflict(
                "setup already completed; an owner account exists".into(),
            ));
        }

        // Re-use the same password / username validation used by register().
        validate_register(&RegisterRequest {
            username:     username.clone(),
            email:        email.clone(),
            password:     password.clone(),
            display_name: display_name.clone(),
            email_otp:    None,
        }, self.max_password_bytes)?;

        let password_hash = PasswordService::hash(&password)?;
        let now = Utc::now();

        let owner = User {
            id:            UserId::new(),
            username,
            email,
            password_hash,
            role:          jiezi_cloud_core::models::user::Role::Owner,
            is_active:     true,
            display_name,
            avatar_url:    None,
            storage_quota: None,
            storage_used:  0,
            failed_login_count:  0,
            locked_until:        None,
            last_failed_login_at: None,
            // The setup wizard is a trusted first-party action — no email
            // verification loop needed for the initial owner.
            email_verified: true,
            created_at:    now,
            updated_at:    now,
        };

        self.user_repo.create(&owner).await?;
        tracing::info!(user_id = %owner.id, "owner account created during first-run setup");
        Ok(owner)
    }

    async fn change_password(&self, user_id: &UserId, new_password: &str) -> AppResult<()> {
        if new_password.len() < 8 {
            return Err(AppError::Validation(
                "password must be at least 8 characters long".into(),
            ));
        }
        if new_password.len() > self.max_password_bytes {
            return Err(AppError::Validation(format!(
                "password must not exceed {} bytes", self.max_password_bytes
            )));
        }
        let new_hash = PasswordService::hash(new_password)?;
        self.user_repo.update_password(user_id, &new_hash).await
    }

    // ─── Email OTP ─────────────────────────────────────────────────────────────

    async fn send_register_otp(&self, req: SendOtpRequest) -> AppResult<()> {
        // When the feature is disabled, silently succeed.
        let (Some(email_repo), Some(email_svc)) = (&self.email_repo, &self.email_svc) else {
            return Ok(());
        };

        let code       = generate_otp();
        let expires_at = Utc::now()
            + chrono::Duration::seconds(self.email_otp_ttl_secs as i64);

        // Clear any pending registration OTPs for this address before issuing a new one.
        email_repo
            .delete_by_email_purpose(&req.email, "register")
            .await?;
        email_repo
            .create(&req.email, &code, "register", expires_at)
            .await?;

        if let Err(e) = email_svc
            .send_otp(&req.email, &req.email, &code, "register")
            .await
        {
            warn!(error = %e, email = %req.email, "failed to send register OTP");
        }

        Ok(())
    }

    async fn send_reset_password_otp(&self, req: SendOtpRequest) -> AppResult<()> {
        // Silently succeed to prevent email enumeration.
        let (Some(email_repo), Some(email_svc)) = (&self.email_repo, &self.email_svc) else {
            return Ok(());
        };

        // If no user with this email exists, return Ok silently.
        let Some(user) = self.user_repo.find_by_email(&req.email).await? else {
            return Ok(());
        };

        let code       = generate_otp();
        let expires_at = Utc::now()
            + chrono::Duration::seconds(self.email_otp_ttl_secs as i64);

        email_repo
            .delete_by_email_purpose(&req.email, "reset_password")
            .await?;
        email_repo
            .create(&req.email, &code, "reset_password", expires_at)
            .await?;

        if let Err(e) = email_svc
            .send_otp(&user.email, &user.username, &code, "reset_password")
            .await
        {
            warn!(error = %e, email = %req.email, "failed to send password-reset OTP");
        }

        Ok(())
    }

    async fn reset_password_with_otp(&self, req: ResetPasswordWithOtpRequest) -> AppResult<()> {
        let email_repo = self.email_repo.as_ref()
            .ok_or_else(|| AppError::Internal("email OTP not configured".into()))?;

        // Validate new password length before hitting the DB.
        if req.new_password.len() < 8 {
            return Err(AppError::Validation(
                "password must be at least 8 characters long".into(),
            ));
        }
        if req.new_password.len() > self.max_password_bytes {
            return Err(AppError::Validation(format!(
                "password must not exceed {} bytes", self.max_password_bytes
            )));
        }

        let row = email_repo
            .find_valid(&req.email, &req.code, "reset_password")
            .await?
            .ok_or_else(|| {
                AppError::Gone(
                    "OTP is invalid, expired, or already used".into(),
                )
            })?;
        email_repo.mark_used(&row.id).await?;

        let user = self
            .user_repo
            .find_by_email(&req.email)
            .await?
            .ok_or_else(|| AppError::NotFound(format!("no account for email {}", req.email)))?;

        let new_hash = PasswordService::hash(&req.new_password)?;
        self.user_repo.update_password(&user.id, &new_hash).await?;

        tracing::info!(email = %req.email, "password reset via OTP");
        Ok(())
    }

    async fn send_unlock_otp(&self, req: SendOtpRequest) -> AppResult<()> {
        // Silently succeed if feature disabled or account not found.
        let (Some(email_repo), Some(email_svc)) = (&self.email_repo, &self.email_svc) else {
            return Ok(());
        };

        let Some(user) = self.user_repo.find_by_email(&req.email).await? else {
            return Ok(());
        };

        // Only send if the account is currently locked.
        let is_locked = user
            .locked_until
            .map(|t| t > Utc::now())
            .unwrap_or(false);
        if !is_locked {
            return Ok(());
        }

        let code       = generate_otp();
        let expires_at = Utc::now()
            + chrono::Duration::seconds(self.email_otp_ttl_secs as i64);

        email_repo
            .delete_by_email_purpose(&req.email, "unlock")
            .await?;
        email_repo
            .create(&req.email, &code, "unlock", expires_at)
            .await?;

        if let Err(e) = email_svc
            .send_otp(&user.email, &user.username, &code, "unlock")
            .await
        {
            warn!(error = %e, email = %req.email, "failed to send unlock OTP");
        }

        Ok(())
    }

    async fn unlock_account_with_otp(&self, req: UnlockWithOtpRequest) -> AppResult<()> {
        let email_repo = self.email_repo.as_ref()
            .ok_or_else(|| AppError::Internal("email OTP not configured".into()))?;

        let row = email_repo
            .find_valid(&req.email, &req.code, "unlock")
            .await?
            .ok_or_else(|| {
                AppError::Gone("OTP is invalid, expired, or already used".into())
            })?;
        email_repo.mark_used(&row.id).await?;

        let user = self
            .user_repo
            .find_by_email(&req.email)
            .await?
            .ok_or_else(|| AppError::NotFound(format!("no account for email {}", req.email)))?;

        // Clear the lockout state.
        self.user_repo.reset_failed_login(&user.id).await?;

        tracing::info!(email = %req.email, "account unlocked via OTP");
        Ok(())
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

    // ── User lookup ───────────────────────────────────────────────────────────

    async fn get_user(&self, user_id: &UserId) -> AppResult<User> {
        self.user_repo
            .find_by_id(user_id)
            .await?
            .ok_or_else(|| AppError::NotFound(format!("user {user_id} not found")))
    }

    async fn list_users(
        &self,
        page: &jiezi_cloud_core::types::PageRequest,
    ) -> AppResult<jiezi_cloud_core::types::PageResponse<User>> {
        let (users, total) = self
            .user_repo
            .list(page.offset(), page.limit())
            .await?;
        Ok(jiezi_cloud_core::types::PageResponse::new(
            users,
            total,
            page.page,
            page.per_page,
        ))
    }

    // ── Self-service ──────────────────────────────────────────────────────────

    async fn update_profile(
        &self,
        user_id: &UserId,
        req:     jiezi_cloud_core::models::user::UpdateProfileRequest,
    ) -> AppResult<User> {
        // Ensure the user exists before writing.
        self.user_repo
            .find_by_id(user_id)
            .await?
            .ok_or_else(|| AppError::NotFound(format!("user {user_id} not found")))?;

        self.user_repo
            .update_profile(user_id, req.display_name, req.avatar_url)
            .await?;

        // Return the refreshed user.
        self.user_repo
            .find_by_id(user_id)
            .await?
            .ok_or_else(|| AppError::NotFound(format!("user {user_id} not found")))
    }

    async fn change_own_password(
        &self,
        user_id: &UserId,
        req:     jiezi_cloud_core::models::user::ChangeOwnPasswordRequest,
    ) -> AppResult<()> {
        if req.new_password.len() < 8 {
            return Err(AppError::Validation(
                "new password must be at least 8 characters long".into(),
            ));
        }
        let user = self
            .user_repo
            .find_by_id(user_id)
            .await?
            .ok_or_else(|| AppError::NotFound(format!("user {user_id} not found")))?;

        let matches = PasswordService::verify(&req.old_password, &user.password_hash)?;
        if !matches {
            return Err(AppError::Unauthorized("current password is incorrect".into()));
        }

        // When email verification is required, the client must first call
        // `POST /auth/me/send-change-password-otp` and supply the code.
        if self.email_verification_required {
            let email_repo = self.email_repo.as_ref()
                .ok_or_else(|| AppError::Internal("email OTP not configured".into()))?;
            let code = req.email_otp.as_deref().unwrap_or("").trim().to_owned();
            if code.is_empty() {
                return Err(AppError::Validation(
                    "email_otp is required when email verification is enabled".into(),
                ));
            }
            let row = email_repo
                .find_valid(&user.email, &code, "change_password")
                .await?
                .ok_or_else(|| {
                    AppError::Validation(
                        "email OTP is invalid, expired, or already used".into(),
                    )
                })?;
            email_repo.mark_used(&row.id).await?;
        }

        let new_hash = PasswordService::hash(&req.new_password)?;
        self.user_repo.update_password(user_id, &new_hash).await
    }

    async fn send_change_password_otp(&self, user_id: &UserId) -> AppResult<()> {
        // No-op when the feature is disabled.
        let (Some(email_repo), Some(email_svc)) = (&self.email_repo, &self.email_svc) else {
            return Ok(());
        };

        let user = self
            .user_repo
            .find_by_id(user_id)
            .await?
            .ok_or_else(|| AppError::NotFound(format!("user {user_id} not found")))?;

        let code       = generate_otp();
        let expires_at = Utc::now()
            + chrono::Duration::seconds(self.email_otp_ttl_secs as i64);

        // Replace any previous change-password OTP for this address.
        email_repo
            .delete_by_email_purpose(&user.email, "change_password")
            .await?;
        email_repo
            .create(&user.email, &code, "change_password", expires_at)
            .await?;

        if let Err(e) = email_svc
            .send_otp(&user.email, &user.username, &code, "change_password")
            .await
        {
            warn!(error = %e, user_id = %user_id, "failed to send change-password OTP");
        }

        Ok(())
    }

    // ── Admin operations ──────────────────────────────────────────────────────

    async fn update_user_role(
        &self,
        caller_role: jiezi_cloud_core::models::user::Role,
        caller_id:   &UserId,
        target_id:   &UserId,
        req:         jiezi_cloud_core::models::user::ChangeRoleRequest,
    ) -> AppResult<User> {
        use jiezi_cloud_core::models::user::Role;

        if caller_id == target_id {
            return Err(AppError::Forbidden(
                "you cannot change your own role".into(),
            ));
        }

        let target = self
            .user_repo
            .find_by_id(target_id)
            .await?
            .ok_or_else(|| AppError::NotFound(format!("user {target_id} not found")))?;

        // Permission matrix:
        //  - Owner  → can set any role, including Owner and Admin.
        //  - Admin  → can only set Member / Guest; cannot touch Owner/Admin.
        match caller_role {
            Role::Owner => { /* unrestricted */ }
            Role::Admin => {
                if matches!(target.role, Role::Owner | Role::Admin) {
                    return Err(AppError::Forbidden(
                        "admin cannot change the role of an owner or another admin".into(),
                    ));
                }
                if matches!(req.new_role, Role::Owner | Role::Admin) {
                    return Err(AppError::Forbidden(
                        "admin cannot promote a user to owner or admin".into(),
                    ));
                }
            }
            _ => {
                return Err(AppError::Forbidden(
                    "only owner or admin can change user roles".into(),
                ));
            }
        }

        self.user_repo.update_role(target_id, req.new_role).await?;

        self.user_repo
            .find_by_id(target_id)
            .await?
            .ok_or_else(|| AppError::NotFound(format!("user {target_id} not found")))
    }

    async fn set_user_active(
        &self,
        caller_role: jiezi_cloud_core::models::user::Role,
        target_id:   &UserId,
        req:         jiezi_cloud_core::models::user::SetActiveRequest,
    ) -> AppResult<()> {
        use jiezi_cloud_core::models::user::Role;

        let target = self
            .user_repo
            .find_by_id(target_id)
            .await?
            .ok_or_else(|| AppError::NotFound(format!("user {target_id} not found")))?;

        // Admin cannot suspend an Owner.
        if matches!(caller_role, Role::Admin) && matches!(target.role, Role::Owner) {
            return Err(AppError::Forbidden(
                "admin cannot suspend an owner account".into(),
            ));
        }

        self.user_repo.set_active(target_id, req.is_active).await
    }

    async fn admin_reset_password(
        &self,
        target_id: &UserId,
        req:       jiezi_cloud_core::models::user::AdminResetPasswordRequest,
    ) -> AppResult<()> {
        // Delegate validation + hashing to the existing change_password method.
        self.change_password(target_id, &req.new_password).await
    }

    async fn update_user_quota(
        &self,
        target_id: &UserId,
        req:       jiezi_cloud_core::models::user::SetQuotaRequest,
    ) -> AppResult<()> {
        // Ensure user exists.
        self.user_repo
            .find_by_id(target_id)
            .await?
            .ok_or_else(|| AppError::NotFound(format!("user {target_id} not found")))?;

        self.user_repo.update_quota(target_id, req.storage_quota).await
    }

    async fn delete_user(&self, target_id: &UserId) -> AppResult<()> {
        // Guard: do not allow deleting the last Owner.
        let owner_count = self.user_repo.count_by_role("owner").await?;
        let target = self
            .user_repo
            .find_by_id(target_id)
            .await?
            .ok_or_else(|| AppError::NotFound(format!("user {target_id} not found")))?;

        if matches!(target.role, jiezi_cloud_core::models::user::Role::Owner)
            && owner_count <= 1
        {
            return Err(AppError::Forbidden(
                "cannot delete the last owner account".into(),
            ));
        }

        // Revoke all refresh tokens so active sessions cannot be replayed.
        self.token_repo.revoke_all_for_user(target_id).await?;

        self.user_repo.delete(target_id).await
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
#[path = "service_tests.rs"]
mod tests;
