//! Integration tests for Argon2id password hashing and verification.

use jiezi_cloud_auth::password::PasswordService;
use jiezi_cloud_core::error::AppError;

// TDD task 2.2-1: hashed output is non-empty
#[test]
fn test_hash_produces_non_empty_string() {
    let hash = PasswordService::hash("hunter2").unwrap();
    assert!(!hash.is_empty());
}

// TDD task 2.2-2: two hashes of the same password differ (random salt)
#[test]
fn test_same_password_hashes_differ() {
    let h1 = PasswordService::hash("my_password").unwrap();
    let h2 = PasswordService::hash("my_password").unwrap();
    assert_ne!(h1, h2, "each hash must embed a unique salt");
}

// TDD task 2.2-3: correct password verifies successfully
#[test]
fn test_correct_password_verifies() {
    let pw = "correct-horse-battery-staple";
    let hash = PasswordService::hash(pw).unwrap();
    assert!(PasswordService::verify(pw, &hash).unwrap());
}

// TDD task 2.2-4: wrong password fails verification
#[test]
fn test_wrong_password_fails_verification() {
    let hash = PasswordService::hash("original_password").unwrap();
    assert!(!PasswordService::verify("wrong_password", &hash).unwrap());
}

// TDD task 2.2-5: empty password is rejected before hashing
#[test]
fn test_empty_password_is_rejected() {
    let err = PasswordService::hash("").unwrap_err();
    assert!(
        matches!(err, AppError::Validation(_)),
        "expected Validation error, got {err:?}"
    );
}

// TDD task 2.2-6: invalid hash string returns an error
#[test]
fn test_invalid_hash_string_returns_error() {
    let result = PasswordService::verify("some_password", "not_a_valid_phc_string");
    assert!(result.is_err());
}

// TDD task 2.2-7: hash output is a valid Argon2id PHC string
#[test]
fn test_hash_output_is_valid_phc_string() {
    let hash = PasswordService::hash("test_password_123").unwrap();
    // PHC strings start with "$argon2id$"
    assert!(
        hash.starts_with("$argon2id$"),
        "expected Argon2id PHC format, got: {hash}"
    );
}
