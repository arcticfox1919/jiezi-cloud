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

use crate::entities::{refresh_tokens, users};

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

// ------ Test helpers --------------------------------------------------------------------------------------------------------------------------

#[cfg(test)]
pub mod test_helpers {
    use sea_orm::{ConnectionTrait, Database, DatabaseConnection, Schema};

    /// Create an in-memory SQLite database and initialise the auth schema.
    ///
    /// Uses `Schema::create_table_from_entity` so there is no dependency on
    /// the `jiezi-cloud-migration` crate.
    pub async fn create_test_db() -> DatabaseConnection {
        let db = Database::connect("sqlite::memory:")
            .await
            .expect("in-memory SQLite pool");

        let backend = db.get_database_backend();
        let schema  = Schema::new(backend);

        // Create tables in dependency order (users first, then FK referencing it)
        db.execute(
            backend.build(&schema.create_table_from_entity(crate::entities::users::Entity)),
        )
        .await
        .expect("create users table");

        db.execute(
            backend.build(
                &schema.create_table_from_entity(crate::entities::refresh_tokens::Entity),
            ),
        )
        .await
        .expect("create refresh_tokens table");

        db
    }
}

// ------ Tests ----------------------------------------------------------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::{test_helpers::create_test_db, *};
    use chrono::Duration;
    use jiezi_cloud_core::models::user::Role;

    // ---- Helpers ------------------------------------------------------------------------------------------------------------------------------

    fn sample_user() -> User {
        let now = Utc::now();
        User {
            id:            UserId::new(),
            username:      "alice".to_owned(),
            email:         "alice@example.com".to_owned(),
            password_hash: "$argon2id$...".to_owned(),
            role:          Role::Member,
            is_active:     true,
            display_name:  Some("Alice".to_owned()),
            avatar_url:    None,
            storage_quota: Some(10 * 1024 * 1024 * 1024), // 10 GiB
            storage_used:  0,
            created_at:    now,
            updated_at:    now,
        }
    }

    // ---- UserRepository tests ----------------------------------------------------------------------------------------------------

    // TDD task 2.1-1: inserting a user and retrieving it by ID succeeds
    #[tokio::test]
    async fn test_create_and_find_user_by_id() {
        let db   = create_test_db().await;
        let repo = UserRepository::new(db);
        let user = sample_user();

        repo.create(&user).await.unwrap();
        let found = repo.find_by_id(&user.id).await.unwrap().unwrap();

        assert_eq!(found.id,       user.id);
        assert_eq!(found.username, user.username);
        assert_eq!(found.email,    user.email);
        assert_eq!(found.role,     user.role);
    }

    // TDD task 2.1-2: find user by username credential
    #[tokio::test]
    async fn test_find_user_by_username() {
        let db   = create_test_db().await;
        let repo = UserRepository::new(db);
        let user = sample_user();

        repo.create(&user).await.unwrap();
        let found = repo
            .find_by_credential("alice")
            .await
            .unwrap()
            .expect("user should be found by username");

        assert_eq!(found.id, user.id);
    }

    // TDD task 2.1-3: find user by email credential
    #[tokio::test]
    async fn test_find_user_by_email() {
        let db   = create_test_db().await;
        let repo = UserRepository::new(db);
        let user = sample_user();

        repo.create(&user).await.unwrap();
        let found = repo
            .find_by_credential("alice@example.com")
            .await
            .unwrap()
            .expect("user should be found by email");

        assert_eq!(found.id, user.id);
    }

    // TDD task 2.1-4: unknown credential returns None
    #[tokio::test]
    async fn test_find_nonexistent_user_returns_none() {
        let db   = create_test_db().await;
        let repo = UserRepository::new(db);

        let result = repo.find_by_credential("nobody").await.unwrap();
        assert!(result.is_none());
    }

    // TDD task 2.1-5: duplicate username triggers Conflict error
    #[tokio::test]
    async fn test_duplicate_username_returns_conflict() {
        let db   = create_test_db().await;
        let repo = UserRepository::new(db);

        let mut user2 = sample_user();
        user2.id    = UserId::new();
        user2.email = "other@example.com".to_owned();

        repo.create(&sample_user()).await.unwrap();
        let err = repo.create(&user2).await.unwrap_err();
        assert!(
            matches!(err, AppError::Conflict(_)),
            "expected Conflict, got {err:?}"
        );
    }

    // TDD task 2.1-6: duplicate email triggers Conflict error
    #[tokio::test]
    async fn test_duplicate_email_returns_conflict() {
        let db   = create_test_db().await;
        let repo = UserRepository::new(db);

        let mut user2 = sample_user();
        user2.id       = UserId::new();
        user2.username = "bob".to_owned();

        repo.create(&sample_user()).await.unwrap();
        let err = repo.create(&user2).await.unwrap_err();
        assert!(matches!(err, AppError::Conflict(_)));
    }

    // TDD task 2.1-7: storage_used counter is incremented correctly
    #[tokio::test]
    async fn test_add_storage_used() {
        let db   = create_test_db().await;
        let repo = UserRepository::new(db);
        let user = sample_user();

        repo.create(&user).await.unwrap();
        repo.add_storage_used(&user.id, 1024).await.unwrap();

        let found = repo.find_by_id(&user.id).await.unwrap().unwrap();
        assert_eq!(found.storage_used, 1024);
    }

    // ---- RefreshTokenRepository tests ------------------------------------------------------------------------------------

    // TDD task 2.1-8: token can be stored and retrieved
    #[tokio::test]
    async fn test_create_and_find_refresh_token() {
        let db         = create_test_db().await;
        let user_repo  = UserRepository::new(db.clone());
        let token_repo = RefreshTokenRepository::new(db);
        let user       = sample_user();
        user_repo.create(&user).await.unwrap();

        let raw     = "some.raw.jwt";
        let family  = "family-uuid";
        let expires = Utc::now() + Duration::days(30);

        token_repo
            .create(raw, &user.id, family, Some("iPhone 15"), expires)
            .await
            .unwrap();

        let stored = token_repo
            .find_by_raw_token(raw)
            .await
            .unwrap()
            .expect("token should be found");

        assert_eq!(stored.user_id,      user.id);
        assert_eq!(stored.family,       family);
        assert_eq!(stored.device_label, Some("iPhone 15".to_owned()));
        assert!(!stored.revoked);
    }

    // TDD task 2.1-9: revoked token is marked correctly
    #[tokio::test]
    async fn test_revoke_refresh_token() {
        let db         = create_test_db().await;
        let user_repo  = UserRepository::new(db.clone());
        let token_repo = RefreshTokenRepository::new(db);
        let user       = sample_user();
        user_repo.create(&user).await.unwrap();

        let raw     = "revoke.me.jwt";
        let expires = Utc::now() + Duration::days(30);
        token_repo.create(raw, &user.id, "fam", None, expires).await.unwrap();

        token_repo.revoke_by_raw_token(raw).await.unwrap();

        let stored = token_repo.find_by_raw_token(raw).await.unwrap().unwrap();
        assert!(stored.revoked);
    }

    // TDD task 2.1-10: revoking a family marks all family members as revoked
    #[tokio::test]
    async fn test_revoke_family_marks_all_tokens() {
        let db         = create_test_db().await;
        let user_repo  = UserRepository::new(db.clone());
        let token_repo = RefreshTokenRepository::new(db);
        let user       = sample_user();
        user_repo.create(&user).await.unwrap();

        let family  = "shared-family";
        let expires = Utc::now() + Duration::days(30);

        token_repo.create("token1.jwt", &user.id, family, None, expires).await.unwrap();
        token_repo.create("token2.jwt", &user.id, family, None, expires).await.unwrap();

        token_repo.revoke_family(family).await.unwrap();

        let t1 = token_repo.find_by_raw_token("token1.jwt").await.unwrap().unwrap();
        let t2 = token_repo.find_by_raw_token("token2.jwt").await.unwrap().unwrap();
        assert!(t1.revoked, "token1 should be revoked");
        assert!(t2.revoked, "token2 should be revoked");
    }

    // TDD task 2.1-11: sha256_hex is deterministic and produces hex characters
    #[test]
    fn test_sha256_hex_is_deterministic() {
        let h1 = sha256_hex("hello");
        let h2 = sha256_hex("hello");
        assert_eq!(h1, h2);
        assert!(h1.chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(h1.len(), 64);
    }

    // TDD task 2.1-12: list_active_for_user returns only non-revoked, non-expired tokens
    #[tokio::test]
    async fn test_list_active_for_user() {
        let db         = create_test_db().await;
        let user_repo  = UserRepository::new(db.clone());
        let token_repo = RefreshTokenRepository::new(db);
        let user       = sample_user();
        user_repo.create(&user).await.unwrap();

        let expires = Utc::now() + Duration::days(30);
        token_repo.create("active.jwt",  &user.id, "fam-a", Some("iPhone"), expires).await.unwrap();
        token_repo.create("revoked.jwt", &user.id, "fam-b", Some("iPad"),   expires).await.unwrap();
        token_repo.revoke_by_raw_token("revoked.jwt").await.unwrap();

        let active = token_repo.list_active_for_user(&user.id).await.unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].family, "fam-a");
    }
}

