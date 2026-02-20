//! Integration tests for JWT token generation and verification.

use std::time::Duration;

use jiezi_cloud_auth::{JwtManager, JwtVerifier};
use jiezi_cloud_auth::jwt::{DEFAULT_ACCESS_TTL, DEFAULT_REFRESH_TTL};
use jiezi_cloud_core::{error::AppError, models::user::Role, types::UserId};

/// Generate a fresh keypair for each test — intentionally avoids any
/// shared static secret so tests do not accidentally depend on HS256
/// semantics.
fn manager() -> JwtManager {
    JwtManager::generate(DEFAULT_ACCESS_TTL, DEFAULT_REFRESH_TTL)
        .expect("test keypair generation must succeed")
}

fn user_id() -> UserId {
    UserId::new()
}

// TDD task 2.3-1: generated token has three Base64 segments (JWT format)
#[test]
fn test_access_token_has_jwt_structure() {
    let (token, _) = manager().generate_access_token(&user_id(), Role::Member).unwrap();
    let parts: Vec<&str> = token.splitn(3, '.').collect();
    assert_eq!(parts.len(), 3, "JWT must have header.payload.signature");
}

// TDD task 2.3-2: valid access token verifies and carries correct sub
#[test]
fn test_access_token_round_trip() {
    let id  = user_id();
    let mgr = manager();
    let (token, _) = mgr.generate_access_token(&id, Role::Admin).unwrap();
    let claims = mgr.verify_access_token(&token).unwrap();
    assert_eq!(claims.sub,  id.to_string());
    assert_eq!(claims.role, Role::Admin);
}

// TDD task 2.3-3: expires_in matches the configured TTL
#[test]
fn test_access_token_expires_in_matches_ttl() {
    let ttl = Duration::from_secs(900);
    let mgr = JwtManager::generate(ttl, DEFAULT_REFRESH_TTL).unwrap();
    let (_, exp_secs) = mgr.generate_access_token(&user_id(), Role::Guest).unwrap();
    assert_eq!(exp_secs, 900);
}

// TDD task 2.3-4: expired access token is rejected
#[test]
fn test_expired_access_token_is_rejected() {
    // Use a 1-second TTL.  Sleep 2.1 s to ensure we land at least one full
    // second past expiry regardless of when within the second we started.
    let mgr = JwtManager::generate(Duration::from_secs(1), DEFAULT_REFRESH_TTL).unwrap();
    let (token, _) = mgr.generate_access_token(&user_id(), Role::Member).unwrap();
    std::thread::sleep(Duration::from_millis(2100));
    let err = mgr.verify_access_token(&token).unwrap_err();
    assert!(
        matches!(err, AppError::Unauthorized(_)),
        "expected Unauthorized, got {err:?}"
    );
}

// TDD task 2.3-5: tampered token is rejected
#[test]
fn test_tampered_access_token_is_rejected() {
    let mgr = manager();
    let (token, _) = mgr.generate_access_token(&user_id(), Role::Member).unwrap();
    let tampered = format!("{token}X"); // corrupt the signature
    assert!(matches!(mgr.verify_access_token(&tampered).unwrap_err(),
                      AppError::Unauthorized(_)));
}

// TDD task 2.3-6: token signed by a different keypair is rejected
#[test]
fn test_token_from_different_keypair_is_rejected() {
    let mgr_a = manager();
    let mgr_b = manager();
    let (token, _) = mgr_a.generate_access_token(&user_id(), Role::Member).unwrap();
    assert!(matches!(mgr_b.verify_access_token(&token).unwrap_err(),
                      AppError::Unauthorized(_)));
}

// TDD task 2.3-7: JwtVerifier (public-key only) can verify a token
#[test]
fn test_jwt_verifier_accepts_valid_token() {
    let mgr = manager();
    let id  = user_id();
    let (token, _) = mgr.generate_access_token(&id, Role::Member).unwrap();

    let verifier = JwtVerifier::from_spki_pem(mgr.public_key_pem()).unwrap();
    let claims   = verifier.verify_access_token(&token).unwrap();
    assert_eq!(claims.sub, id.to_string());
}

// TDD task 2.3-8: JwtVerifier rejects a token from a different keypair
#[test]
fn test_jwt_verifier_rejects_wrong_keypair() {
    let mgr_a = manager();
    let mgr_b = manager();
    let (token, _) = mgr_a.generate_access_token(&user_id(), Role::Member).unwrap();

    let verifier = JwtVerifier::from_spki_pem(mgr_b.public_key_pem()).unwrap();
    assert!(matches!(verifier.verify_access_token(&token).unwrap_err(),
                      AppError::Unauthorized(_)));
}

// TDD task 2.3-9: refresh token carries correct sub and family
#[test]
fn test_refresh_token_round_trip() {
    let id  = user_id();
    let mgr = manager();
    let (token, family) = mgr.generate_refresh_token(&id).unwrap();
    let claims = mgr.verify_refresh_token(&token).unwrap();
    assert_eq!(claims.sub,    id.to_string());
    assert_eq!(claims.family, family);
}

// TDD task 2.3-10: two refresh tokens for the same user have distinct families
#[test]
fn test_refresh_tokens_have_unique_families() {
    let mgr = manager();
    let id  = user_id();
    let (_, family1) = mgr.generate_refresh_token(&id).unwrap();
    let (_, family2) = mgr.generate_refresh_token(&id).unwrap();
    assert_ne!(family1, family2);
}
