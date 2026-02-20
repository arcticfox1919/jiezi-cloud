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
