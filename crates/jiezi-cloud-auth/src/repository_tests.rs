//! Unit tests for [`UserRepository`] and [`RefreshTokenRepository`].
//!
//! Each test gets a fresh in-memory SQLite database via [`create_test_db`],
//! so tests are fully isolated and leave no filesystem footprint.

// ─── Test helpers ─────────────────────────────────────────────────────────────

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
                &schema.create_table_from_entity(
                    crate::entities::refresh_tokens::Entity,
                ),
            ),
        )
        .await
        .expect("create refresh_tokens table");

        db.execute(
            backend.build(
                &schema.create_table_from_entity(
                    crate::entities::email_otps::Entity,
                ),
            ),
        )
        .await
        .expect("create email_otps table");

        db
    }
}

use super::*;
use test_helpers::create_test_db;
use chrono::Duration;
use jiezi_cloud_core::models::user::Role;

// ─── Shared fixture ───────────────────────────────────────────────────────────

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
        storage_quota: Some(10 * 1024 * 1024 * 1024),
        storage_used:  0,
        failed_login_count:   0,
        locked_until:         None,
        last_failed_login_at: None,
        email_verified:       true,
        created_at:    now,
        updated_at:    now,
    }
}

// ─── UserRepository ───────────────────────────────────────────────────────────

/// Inserting a user and retrieving it by ID round-trips cleanly.
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

/// `find_by_credential` resolves via username.
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

/// `find_by_credential` resolves via email address.
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

/// Unknown credential returns `None` rather than an error.
#[tokio::test]
async fn test_find_nonexistent_user_returns_none() {
    let db   = create_test_db().await;
    let repo = UserRepository::new(db);

    let result = repo.find_by_credential("nobody").await.unwrap();
    assert!(result.is_none());
}

/// Duplicate username yields `AppError::Conflict`.
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

/// Duplicate email yields `AppError::Conflict`.
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

/// `add_storage_used` atomically accumulates bytes.
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

// ─── RefreshTokenRepository ───────────────────────────────────────────────────

/// Token can be persisted and retrieved by raw JWT.
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

/// `revoke_by_raw_token` sets the `revoked` flag.
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

/// `revoke_family` marks every member of the family revoked.
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

/// `sha256_hex` is deterministic and produces 64 lowercase hex characters.
#[test]
fn test_sha256_hex_is_deterministic() {
    let h1 = sha256_hex("hello");
    let h2 = sha256_hex("hello");
    assert_eq!(h1, h2);
    assert!(h1.chars().all(|c| c.is_ascii_hexdigit()));
    assert_eq!(h1.len(), 64);
}

/// `list_active_for_user` excludes revoked tokens.
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
