//! SeaORM-backed repositories for user accounts and refresh tokens.
//!
//! # Design
//!
//! Two focused repository structs keep persistence concerns separate from
//! business logic - both share the same [`DatabaseConnection`] handle, which
//! is internally reference-counted and cheap to clone.
//!
//! - [`UserRepository`]          - CRUD for user records.
//! - [`RefreshTokenRepository`]  - token lifecycle (create, lookup, revoke,
//!   family revocation, session listing).
//!
//! # Database backend
//!
//! The backend (SQLite / PostgreSQL / MySQL) is selected at **compile time**
//! via Cargo feature flags on this crate.  At **runtime** the URL is read from
//! the application configuration and passed to [`sea_orm::Database::connect`].
//!
//! # Token hashing
//!
//! Raw JWT strings are **never** stored.  The SHA-256 hex digest of each JWT
//! is the primary key.  A database dump cannot be used to replay stolen tokens.

use chrono::{DateTime, Utc};
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, Condition, DatabaseConnection,
    EntityTrait, QueryFilter, QueryOrder, sea_query::Expr,
};
use sha2::{Digest, Sha256};

use jiezi_cloud_core::{
    error::{AppError, AppResult},
    models::user::{Role, User},
    types::UserId,
};

use crate::entities::{email_otps, refresh_tokens, users};

// ------ Helpers ------------------------------------------------------------------------------------------------------------------------------------

/// Compute the hex-encoded SHA-256 digest of `input`.
pub fn sha256_hex(input: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Map a [`sea_orm::DbErr`] to an [`AppError`], treating UNIQUE violations as
/// [`AppError::Conflict`].
fn map_db_err(e: sea_orm::DbErr) -> AppError {
    let msg = e.to_string();
    // SQLite: "UNIQUE constraint failed"
    // PostgreSQL: "duplicate key value violates unique constraint"
    // MySQL: "Duplicate entry"
    if msg.to_uppercase().contains("UNIQUE")
        || msg.contains("duplicate key")
        || msg.contains("Duplicate entry")
    {
        AppError::Conflict("username or email already taken".to_owned())
    } else {
        AppError::Database(msg)
    }
}

/// Convert a [`users::Model`] row to the domain [`User`].
fn model_to_user(m: users::Model) -> AppResult<User> {
    Ok(User {
        id:            m.id.parse::<UserId>().map_err(|e| AppError::Database(e.to_string()))?,
        username:      m.username,
        email:         m.email,
        password_hash: m.password_hash,
        role:          parse_role(&m.role)?,
        is_active:     m.is_active,
        display_name:  m.display_name,
        avatar_url:    m.avatar_url,
        storage_quota: m.storage_quota.map(|q| q as u64),
        storage_used:  m.storage_used as u64,
        failed_login_count:  m.failed_login_count,
        locked_until:        m.locked_until,
        last_failed_login_at: m.last_failed_login_at,
        email_verified: m.email_verified,
        created_at:    m.created_at,
        updated_at:    m.updated_at,
    })
}

fn parse_role(s: &str) -> AppResult<Role> {
    match s {
        "owner"  => Ok(Role::Owner),
        "admin"  => Ok(Role::Admin),
        "member" => Ok(Role::Member),
        "guest"  => Ok(Role::Guest),
        other    => Err(AppError::Database(format!("unknown role: {other}"))),
    }
}

// ------ StoredRefreshToken --------------------------------------------------------------------------------------------------------------

/// A persisted refresh-token record.  The raw JWT is NOT included.
#[derive(Debug, Clone)]
pub struct StoredRefreshToken {
    pub token_hash:   String,
    pub user_id:      UserId,
    pub family:       String,
    pub device_label: Option<String>,
    pub expires_at:   DateTime<Utc>,
    pub revoked:      bool,
    pub created_at:   DateTime<Utc>,
}

fn model_to_stored_token(m: refresh_tokens::Model) -> AppResult<StoredRefreshToken> {
    Ok(StoredRefreshToken {
        token_hash:   m.token_hash,
        user_id:      m.user_id.parse::<UserId>().map_err(|e| AppError::Database(e.to_string()))?,
        family:       m.family,
        device_label: m.device_label,
        expires_at:   m.expires_at,
        revoked:      m.revoked,
        created_at:   m.created_at,
    })
}

// ------ UserRepository ----------------------------------------------------------------------------------------------------------------------

/// Repository for [`User`] records.
#[derive(Clone)]
pub struct UserRepository {
    db: DatabaseConnection,
}

impl UserRepository {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }

    /// Persist a new user record.
    ///
    /// # Errors
    ///
    /// - [`AppError::Conflict`] if the username or email is already taken.
    pub async fn create(&self, user: &User) -> AppResult<()> {
        let am = users::ActiveModel {
            id:            Set(user.id.to_string()),
            username:      Set(user.username.clone()),
            email:         Set(user.email.clone()),
            password_hash: Set(user.password_hash.clone()),
            role:          Set(user.role.to_string()),
            is_active:     Set(user.is_active),
            display_name:  Set(user.display_name.clone()),
            avatar_url:    Set(user.avatar_url.clone()),
            storage_quota: Set(user.storage_quota.map(|q| q as i64)),
            storage_used:  Set(user.storage_used as i64),
            failed_login_count:   Set(0),
            locked_until:         Set(None),
            last_failed_login_at: Set(None),
            email_verified:       Set(user.email_verified),
            created_at:    Set(user.created_at),
            updated_at:    Set(user.updated_at),
        };
        am.insert(&self.db).await.map_err(map_db_err)?;
        Ok(())
    }

    /// Find a user by their exact username **or** email address.
    pub async fn find_by_credential(&self, credential: &str) -> AppResult<Option<User>> {
        users::Entity::find()
            .filter(
                Condition::any()
                    .add(users::Column::Username.eq(credential))
                    .add(users::Column::Email.eq(credential)),
            )
            .one(&self.db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))?
            .map(model_to_user)
            .transpose()
    }

    /// Find a user by their exact email address.
    pub async fn find_by_email(&self, email: &str) -> AppResult<Option<User>> {
        users::Entity::find()
            .filter(users::Column::Email.eq(email))
            .one(&self.db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))?
            .map(model_to_user)
            .transpose()
    }

    /// Find a user by their UUID primary key.
    pub async fn find_by_id(&self, id: &UserId) -> AppResult<Option<User>> {
        users::Entity::find_by_id(id.to_string())
            .one(&self.db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))?
            .map(model_to_user)
            .transpose()
    }

    /// Atomically increment `storage_used` by `delta_bytes` (may be negative
    /// to decrement on file deletion).
    pub async fn add_storage_used(&self, id: &UserId, delta_bytes: i64) -> AppResult<()> {
        users::Entity::update_many()
            .col_expr(
                users::Column::StorageUsed,
                Expr::col(users::Column::StorageUsed).add(delta_bytes),
            )
            .col_expr(users::Column::UpdatedAt, Expr::value(Utc::now()))
            .filter(users::Column::Id.eq(id.to_string()))
            .exec(&self.db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(())
    }

    /// Overwrite the password hash for a user.
    ///
    /// Used by the first-run setup wizard (`POST /setup/complete`) and future
    /// admin-triggered password-reset flows.  Does NOT require the old password.
    ///
    /// # Errors
    ///
    /// - [`AppError::NotFound`] if `id` does not match any user.
    pub async fn update_password(&self, id: &UserId, new_hash: &str) -> AppResult<()> {
        let result = users::Entity::update_many()
            .col_expr(users::Column::PasswordHash, Expr::value(new_hash))
            .col_expr(users::Column::UpdatedAt, Expr::value(Utc::now()))
            .filter(users::Column::Id.eq(id.to_string()))
            .exec(&self.db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))?;
        if result.rows_affected == 0 {
            Err(AppError::NotFound(format!("user {id} not found")))
        } else {
            Ok(())
        }
    }

    /// Count the number of users with a specific `role` string.
    ///
    /// The setup wizard calls this to detect whether an Owner account already
    /// exists before attempting to create one.
    pub async fn count_by_role(&self, role: &str) -> AppResult<u64> {
        use sea_orm::PaginatorTrait;
        users::Entity::find()
            .filter(users::Column::Role.eq(role))
            .count(&self.db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))
    }

    /// Return a paginated page of users ordered by creation time (newest first).
    ///
    /// Returns `(rows, total_count)`.
    pub async fn list(&self, offset: u64, limit: u64) -> AppResult<(Vec<User>, u64)> {
        use sea_orm::{PaginatorTrait, QueryOrder, QuerySelect};

        let total = users::Entity::find()
            .count(&self.db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))?;

        let rows = users::Entity::find()
            .order_by_desc(users::Column::CreatedAt)
            .offset(offset)
            .limit(limit)
            .all(&self.db)
            .await
            .map_err(|e: sea_orm::DbErr| AppError::Database(e.to_string()))?;

        let users_vec = rows.into_iter().map(model_to_user).collect::<AppResult<Vec<_>>>()?;
        Ok((users_vec, total))
    }

    /// Overwrite the system-level role string for a user.
    pub async fn update_role(&self, id: &UserId, role: Role) -> AppResult<()> {
        let result = users::Entity::update_many()
            .col_expr(users::Column::Role, Expr::value(role.to_string()))
            .col_expr(users::Column::UpdatedAt, Expr::value(Utc::now()))
            .filter(users::Column::Id.eq(id.to_string()))
            .exec(&self.db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))?;
        if result.rows_affected == 0 {
            Err(AppError::NotFound(format!("user {id} not found")))
        } else {
            Ok(())
        }
    }

    /// Set `is_active` for a user (suspend / reactivate).
    pub async fn set_active(&self, id: &UserId, active: bool) -> AppResult<()> {
        let result = users::Entity::update_many()
            .col_expr(users::Column::IsActive, Expr::value(active))
            .col_expr(users::Column::UpdatedAt, Expr::value(Utc::now()))
            .filter(users::Column::Id.eq(id.to_string()))
            .exec(&self.db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))?;
        if result.rows_affected == 0 {
            Err(AppError::NotFound(format!("user {id} not found")))
        } else {
            Ok(())
        }
    }

    /// Permanently delete a user row and all their refresh tokens.
    pub async fn delete(&self, id: &UserId) -> AppResult<()> {
        let result = users::Entity::delete_by_id(id.to_string())
            .exec(&self.db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))?;
        if result.rows_affected == 0 {
            Err(AppError::NotFound(format!("user {id} not found")))
        } else {
            Ok(())
        }
    }

    /// Update profile fields.
    ///
    /// Each argument is `Option<Option<String>>`:
    /// - `None`       → field not touched.
    /// - `Some(None)` → field cleared to NULL.
    /// - `Some(Some(v))` → field set to `v`.
    pub async fn update_profile(
        &self,
        id:           &UserId,
        display_name: Option<Option<String>>,
        avatar_url:   Option<Option<String>>,
    ) -> AppResult<()> {
        use sea_orm::ActiveValue::{NotSet, Set};

        // Build a partial ActiveModel — only set the fields that changed.
        let am = users::ActiveModel {
            id:           Set(id.to_string()),
            updated_at:   Set(Utc::now()),
            display_name: match display_name {
                Some(v) => Set(v),
                None    => NotSet,
            },
            avatar_url:   match avatar_url {
                Some(v) => Set(v),
                None    => NotSet,
            },
            ..Default::default()
        };
        am.update(&self.db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(())
    }

    /// Overwrite the per-user storage quota.  `None` = unlimited.
    pub async fn update_quota(&self, id: &UserId, quota: Option<u64>) -> AppResult<()> {
        let result = users::Entity::update_many()
            .col_expr(
                users::Column::StorageQuota,
                Expr::value(quota.map(|q| q as i64)),
            )
            .col_expr(users::Column::UpdatedAt, Expr::value(Utc::now()))
            .filter(users::Column::Id.eq(id.to_string()))
            .exec(&self.db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))?;
        if result.rows_affected == 0 {
            Err(AppError::NotFound(format!("user {id} not found")))
        } else {
            Ok(())
        }
    }

    // ── Brute-force lockout helpers ─────────────────────────────────────────────────────

    /// Increment the failed-login counter and record the attempt timestamp.
    ///
    /// Returns the **new** counter value so the service can decide whether to
    /// apply a lockout without fetching the user row again.
    pub async fn record_failed_login(&self, id: &UserId) -> AppResult<i32> {
        // Atomically increment so concurrent requests don't race-to-zero.
        users::Entity::update_many()
            .col_expr(
                users::Column::FailedLoginCount,
                Expr::col(users::Column::FailedLoginCount).add(1),
            )
            .col_expr(users::Column::LastFailedLoginAt, Expr::value(Utc::now()))
            .col_expr(users::Column::UpdatedAt, Expr::value(Utc::now()))
            .filter(users::Column::Id.eq(id.to_string()))
            .exec(&self.db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))?;

        // Re-fetch to get the new value (SQLite doesn't support RETURNING).
        let row = users::Entity::find_by_id(id.to_string())
            .one(&self.db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))?
            .ok_or_else(|| AppError::NotFound(format!("user {id} not found")))?;

        Ok(row.failed_login_count)
    }

    /// Lock the account until `until`, called after exceeding the attempt limit.
    pub async fn lock_account(&self, id: &UserId, until: chrono::DateTime<Utc>) -> AppResult<()> {
        users::Entity::update_many()
            .col_expr(users::Column::LockedUntil, Expr::value(until))
            .col_expr(users::Column::UpdatedAt, Expr::value(Utc::now()))
            .filter(users::Column::Id.eq(id.to_string()))
            .exec(&self.db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(())
    }

    /// Reset the failed-login counter and remove any lockout after a successful
    /// authentication.
    pub async fn reset_failed_login(&self, id: &UserId) -> AppResult<()> {
        users::Entity::update_many()
            .col_expr(users::Column::FailedLoginCount, Expr::value(0_i32))
            .col_expr(users::Column::LockedUntil, Expr::value(Option::<chrono::DateTime<Utc>>::None))
            .col_expr(users::Column::UpdatedAt, Expr::value(Utc::now()))
            .filter(users::Column::Id.eq(id.to_string()))
            .exec(&self.db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(())
    }

    /// Mark the user's email address as verified.
    pub async fn set_email_verified(&self, id: &UserId) -> AppResult<()> {
        let result = users::Entity::update_many()
            .col_expr(users::Column::EmailVerified, Expr::value(true))
            .col_expr(users::Column::UpdatedAt, Expr::value(Utc::now()))
            .filter(users::Column::Id.eq(id.to_string()))
            .exec(&self.db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))?;
        if result.rows_affected == 0 {
            Err(AppError::NotFound(format!("user {id} not found")))
        } else {
            Ok(())
        }
    }
}

// ------ EmailOtpRepository ────────────────────────────────────────────────────────────────

/// A persisted OTP row (pending or used).
#[derive(Debug, Clone)]
pub struct OtpRow {
    pub id:         String,
    /// The email address the OTP was sent to.  No FK to `users` — the user
    /// may not exist yet when the `register` OTP is created.
    pub email:      String,
    /// The 6-digit numeric code (plaintext; TTL is the security control).
    pub code:       String,
    /// One of `"register"`, `"reset_password"`, or `"unlock"`.
    pub purpose:    String,
    pub expires_at: DateTime<Utc>,
    pub used_at:    Option<DateTime<Utc>>,
}

/// Repository for email OTP records.
#[derive(Clone)]
pub struct EmailOtpRepository {
    db: DatabaseConnection,
}

impl EmailOtpRepository {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }

    /// Persist a new OTP row.
    pub async fn create(
        &self,
        email:      &str,
        code:       &str,
        purpose:    &str,
        expires_at: DateTime<Utc>,
    ) -> AppResult<()> {
        use uuid::Uuid;
        let am = email_otps::ActiveModel {
            id:         Set(Uuid::new_v4().to_string()),
            email:      Set(email.to_owned()),
            code:       Set(code.to_owned()),
            purpose:    Set(purpose.to_owned()),
            expires_at: Set(expires_at),
            used_at:    Set(None),
        };
        am.insert(&self.db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(())
    }

    /// Find a valid (not expired, not used) OTP matching email + code + purpose.
    pub async fn find_valid(
        &self,
        email:   &str,
        code:    &str,
        purpose: &str,
    ) -> AppResult<Option<OtpRow>> {
        let now = Utc::now();
        let row = email_otps::Entity::find()
            .filter(email_otps::Column::Email.eq(email))
            .filter(email_otps::Column::Code.eq(code))
            .filter(email_otps::Column::Purpose.eq(purpose))
            .filter(email_otps::Column::ExpiresAt.gt(now))
            .filter(email_otps::Column::UsedAt.is_null())
            .one(&self.db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))?;

        Ok(row.map(|m| OtpRow {
            id:         m.id,
            email:      m.email,
            code:       m.code,
            purpose:    m.purpose,
            expires_at: m.expires_at,
            used_at:    m.used_at,
        }))
    }

    /// Mark an OTP as consumed (sets `used_at = now()`).
    pub async fn mark_used(&self, id: &str) -> AppResult<()> {
        email_otps::Entity::update_many()
            .col_expr(email_otps::Column::UsedAt, Expr::value(Utc::now()))
            .filter(email_otps::Column::Id.eq(id))
            .exec(&self.db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(())
    }

    /// Delete all unused OTPs for a given (email, purpose) pair.
    ///
    /// Called before issuing a fresh code to prevent accumulation.
    pub async fn delete_by_email_purpose(&self, email: &str, purpose: &str) -> AppResult<()> {
        email_otps::Entity::delete_many()
            .filter(email_otps::Column::Email.eq(email))
            .filter(email_otps::Column::Purpose.eq(purpose))
            .filter(email_otps::Column::UsedAt.is_null())
            .exec(&self.db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(())
    }

    /// Count OTPs for a given (email, purpose) created since `since`.
    ///
    /// Used for per-email rate limiting: if the count exceeds a threshold, the
    /// service should reject the send request.
    pub async fn count_recent(
        &self,
        email:   &str,
        purpose: &str,
        since:   DateTime<Utc>,
    ) -> AppResult<u64> {
        use sea_orm::PaginatorTrait;
        email_otps::Entity::find()
            .filter(email_otps::Column::Email.eq(email))
            .filter(email_otps::Column::Purpose.eq(purpose))
            .filter(email_otps::Column::ExpiresAt.gt(since))
            .count(&self.db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))
    }
}

// ------ RefreshTokenRepository ------------------------------------------------------------------------------------------------------

/// Repository for refresh-token records.
#[derive(Clone)]
pub struct RefreshTokenRepository {
    db: DatabaseConnection,
}

impl RefreshTokenRepository {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }

    /// Persist a new refresh token.
    ///
    /// `device_label` is the optional human-readable device label supplied at
    /// login time.  It is copied verbatim to every rotation token within the
    /// same family so that `list_active_for_user` can display it without extra
    /// queries.
    pub async fn create(
        &self,
        raw_token:    &str,
        user_id:      &UserId,
        family:       &str,
        device_label: Option<&str>,
        expires_at:   DateTime<Utc>,
    ) -> AppResult<()> {
        let am = refresh_tokens::ActiveModel {
            token_hash:   Set(sha256_hex(raw_token)),
            user_id:      Set(user_id.to_string()),
            family:       Set(family.to_owned()),
            device_label: Set(device_label.map(str::to_owned)),
            expires_at:   Set(expires_at),
            revoked:      Set(false),
            created_at:   Set(Utc::now()),
        };
        am.insert(&self.db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(())
    }

    /// Look up by the *raw* JWT (hashes internally).
    pub async fn find_by_raw_token(
        &self,
        raw_token: &str,
    ) -> AppResult<Option<StoredRefreshToken>> {
        self.find_by_hash(&sha256_hex(raw_token)).await
    }

    /// Look up by pre-computed SHA-256 hash.
    pub async fn find_by_hash(&self, token_hash: &str) -> AppResult<Option<StoredRefreshToken>> {
        refresh_tokens::Entity::find_by_id(token_hash.to_owned())
            .one(&self.db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))?
            .map(model_to_stored_token)
            .transpose()
    }

    /// Mark a single token as revoked (by raw JWT).
    pub async fn revoke_by_raw_token(&self, raw_token: &str) -> AppResult<()> {
        let hash = sha256_hex(raw_token);
        refresh_tokens::Entity::update_many()
            .col_expr(refresh_tokens::Column::Revoked, Expr::value(true))
            .filter(refresh_tokens::Column::TokenHash.eq(hash))
            .exec(&self.db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(())
    }

    /// Revoke all tokens belonging to a family (reuse-attack response or
    /// per-device logout).
    pub async fn revoke_family(&self, family: &str) -> AppResult<()> {
        refresh_tokens::Entity::update_many()
            .col_expr(refresh_tokens::Column::Revoked, Expr::value(true))
            .filter(refresh_tokens::Column::Family.eq(family))
            .exec(&self.db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(())
    }

    /// Revoke every token for a user — used when the account is being deleted
    /// so that active sessions cannot be replayed.
    pub async fn revoke_all_for_user(&self, user_id: &UserId) -> AppResult<()> {
        refresh_tokens::Entity::update_many()
            .col_expr(refresh_tokens::Column::Revoked, Expr::value(true))
            .filter(refresh_tokens::Column::UserId.eq(user_id.to_string()))
            .exec(&self.db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(())
    }

    /// Delete expired tokens for a user (housekeeping).
    pub async fn delete_expired_for_user(&self, user_id: &UserId) -> AppResult<()> {
        refresh_tokens::Entity::delete_many()
            .filter(refresh_tokens::Column::UserId.eq(user_id.to_string()))
            .filter(refresh_tokens::Column::ExpiresAt.lte(Utc::now()))
            .exec(&self.db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(())
    }

    /// Return all non-revoked, non-expired tokens for a user, ordered by
    /// family then creation time.  Used by `AuthService::list_sessions` to
    /// build the per-device session list.
    pub async fn list_active_for_user(
        &self,
        user_id: &UserId,
    ) -> AppResult<Vec<StoredRefreshToken>> {
        refresh_tokens::Entity::find()
            .filter(refresh_tokens::Column::UserId.eq(user_id.to_string()))
            .filter(refresh_tokens::Column::Revoked.eq(false))
            .filter(refresh_tokens::Column::ExpiresAt.gt(Utc::now()))
            .order_by_asc(refresh_tokens::Column::Family)
            .order_by_asc(refresh_tokens::Column::CreatedAt)
            .all(&self.db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))?
            .into_iter()
            .map(model_to_stored_token)
            .collect()
    }
}

#[cfg(test)]
#[path = "repository_tests.rs"]
pub(crate) mod tests;

