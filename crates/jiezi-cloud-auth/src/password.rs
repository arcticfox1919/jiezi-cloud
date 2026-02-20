//! Argon2id password hashing and verification.
//!
//! # Design
//!
//! - Uses `argon2` crate with the recommended Argon2id variant.
//! - Each hash embeds a random 16-byte salt, so hashing the same password
//!   twice produces distinct PHC strings.
//! - Verification is constant-time to prevent timing side-channels.
//! - Empty passwords are rejected at this layer before any crypto work.

use argon2::{
    password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use jiezi_cloud_core::error::{AppError, AppResult};

/// A stateless helper for Argon2id password hashing and verification.
///
/// All methods are free functions on an empty struct so they can be called
/// without constructing any state: `PasswordService::hash(pw)?`.
pub struct PasswordService;

impl PasswordService {
    /// Hash a plaintext password with Argon2id + a random salt.
    ///
    /// Returns a self-contained PHC string that includes the algorithm
    /// parameters and salt — suitable for direct storage in the database.
    ///
    /// # Errors
    ///
    /// - [`AppError::Validation`] if `password` is empty.
    /// - [`AppError::Internal`] if hashing fails (should not occur in practice).
    pub fn hash(password: &str) -> AppResult<String> {
        if password.is_empty() {
            return Err(AppError::Validation(
                "password must not be empty".to_owned(),
            ));
        }

        let salt = SaltString::generate(&mut OsRng);
        Argon2::default()
            .hash_password(password.as_bytes(), &salt)
            .map(|h| h.to_string())
            .map_err(|e| AppError::Internal(format!("argon2 hash failed: {e}")))
    }

    /// Verify that `password` matches the stored Argon2id PHC `hash`.
    ///
    /// Returns `Ok(true)` on match, `Ok(false)` on mismatch.
    /// This operation is deliberately constant-time to prevent timing attacks.
    ///
    /// # Errors
    ///
    /// - [`AppError::Internal`] if `hash` is not a valid PHC string.
    pub fn verify(password: &str, hash: &str) -> AppResult<bool> {
        let parsed = PasswordHash::new(hash)
            .map_err(|e| AppError::Internal(format!("invalid PHC hash string: {e}")))?;

        Ok(Argon2::default()
            .verify_password(password.as_bytes(), &parsed)
            .is_ok())
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

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
}
