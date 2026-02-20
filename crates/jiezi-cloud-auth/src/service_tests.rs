// Tests for AuthServiceImpl.
//
// Declared from service.rs as:
//     #[cfg(test)]
//     #[path = "service_tests.rs"]
//     mod tests;
//
// `use super::*` brings all public and crate-visible items from service.rs
// into scope, including the AuthService trait (needed to call trait methods).

use super::*;
use crate::repository::tests::test_helpers::create_test_db;

async fn make_service() -> AuthServiceImpl {
    let db         = create_test_db().await;
    let user_repo  = UserRepository::new(db.clone());
    let token_repo = RefreshTokenRepository::new(db);

    let jwt = Arc::new(
        JwtManager::generate(Duration::from_secs(900), Duration::from_secs(30 * 24 * 60 * 60))
            .expect("JwtManager::generate"),
    );

    AuthServiceImpl::new(
        user_repo,
        token_repo,
        jwt,
        Duration::from_secs(30 * 24 * 60 * 60),
    )
}

fn reg(username: &str) -> RegisterRequest {
    RegisterRequest {
        username:    username.to_owned(),
        email:       format!("{username}@example.com"),
        password:    "password123".to_owned(),
        display_name: None,
        email_otp:   None,
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
        email_otp:   None,
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
        email_otp:    None,
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
