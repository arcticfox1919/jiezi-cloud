//! JWT access-token and refresh-token generation and validation.
//!
//! # Token strategy
//!
//! | Token type    | TTL (default) | Stored server-side? | Purpose                        |
//! |---------------|---------------|---------------------|--------------------------------|
//! | Access token  | 15 min        | No                  | Authorise API requests         |
//! | Refresh token | 7 days        | Yes (hash only)     | Obtain a new access token pair |
//!
//! # Rotation & reuse detection
//!
//! Every refresh token belongs to a **family** (a random UUID minted at
//! login).  On each successful refresh, the old token is revoked and a new
//! one in the same family is issued.  If a *revoked* family member is
//! presented, the entire family is wiped, forcing re-authentication.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation, decode, encode};
use jiezi_cloud_core::{
    error::{AppError, AppResult},
    models::user::{Claims, RefreshClaims, Role},
    types::UserId,
};
use uuid::Uuid;

// ─── Configuration ────────────────────────────────────────────────────────────

/// Sensible defaults used when no explicit TTL is provided.
pub const DEFAULT_ACCESS_TTL:  Duration = Duration::from_secs(15 * 60);          // 15 min
pub const DEFAULT_REFRESH_TTL: Duration = Duration::from_secs(30 * 24 * 60 * 60); // 30 days

// ─── JwtManager ──────────────────────────────────────────────────────────────

/// Handles JWT encoding, decoding and validation for the platform.
///
/// Immutable once constructed — safe to wrap in [`Arc`] and share across threads.
///
/// [`Arc`]: std::sync::Arc
pub struct JwtManager {
    encoding_key: EncodingKey,
    decoding_key: DecodingKey,
    access_ttl:   Duration,
    refresh_ttl:  Duration,
}

impl JwtManager {
    /// Construct a `JwtManager` with explicit TTL durations.
    ///
    /// `secret` is used as the HMAC-SHA256 signing key.  In production this
    /// should be a cryptographically random value of at least 32 bytes.
    pub fn new(secret: &str, access_ttl: Duration, refresh_ttl: Duration) -> Self {
        let bytes = secret.as_bytes();
        Self {
            encoding_key: EncodingKey::from_secret(bytes),
            decoding_key: DecodingKey::from_secret(bytes),
            access_ttl,
            refresh_ttl,
        }
    }

    /// Construct a `JwtManager` with the default TTL values.
    pub fn with_defaults(secret: &str) -> Self {
        Self::new(secret, DEFAULT_ACCESS_TTL, DEFAULT_REFRESH_TTL)
    }

    // ── Access tokens ─────────────────────────────────────────────────────────

    /// Generate a signed access token for the given user.
    ///
    /// Returns `(token_string, expires_in_secs)`.
    pub fn generate_access_token(
        &self,
        user_id: &UserId,
        role: Role,
    ) -> AppResult<(String, u64)> {
        let now = unix_now_secs();
        let claims = Claims {
            sub: user_id.to_string(),
            exp: now + self.access_ttl.as_secs() as usize,
            iat: now,
            role,
        };

        let token = encode(&Header::default(), &claims, &self.encoding_key)
            .map_err(|e| AppError::Internal(format!("access-token encoding failed: {e}")))?;

        Ok((token, self.access_ttl.as_secs()))
    }

    /// Decode and validate an access token, returning the embedded [`Claims`].
    ///
    /// # Errors
    ///
    /// - [`AppError::Unauthorized`] if the token is expired, malformed, or has
    ///   an invalid signature.
    pub fn verify_access_token(&self, token: &str) -> AppResult<Claims> {
        let mut validation = Validation::new(Algorithm::HS256);
        validation.validate_exp = true;
        validation.leeway = 0; // strict: no clock-skew tolerance

        decode::<Claims>(token, &self.decoding_key, &validation)
            .map(|d| d.claims)
            .map_err(jwt_to_app_error)
    }

    // ── Refresh tokens ────────────────────────────────────────────────────────

    /// Generate a signed refresh token for the given user.
    ///
    /// Returns `(token_string, family_id)`.
    /// The `family_id` must be stored alongside the token hash in the DB so
    /// that token-rotation attacks can be detected later.
    pub fn generate_refresh_token(&self, user_id: &UserId) -> AppResult<(String, String)> {
        let family = Uuid::new_v4().to_string();
        let token  = self.mint_refresh_token_in_family(user_id, &family)?;
        Ok((token, family))
    }

    /// Generate a refresh token that belongs to an *existing* family.
    ///
    /// Used during token rotation to preserve the family across the
    /// old → new pair, enabling reuse-attack detection.
    pub fn rotate_refresh_token(
        &self,
        user_id: &UserId,
        family: &str,
    ) -> AppResult<String> {
        self.mint_refresh_token_in_family(user_id, family)
    }

    /// Internal: sign a refresh token JWT with the given family string.
    fn mint_refresh_token_in_family(&self, user_id: &UserId, family: &str) -> AppResult<String> {
        let now = unix_now_secs();
        let claims = RefreshClaims {
            sub:    user_id.to_string(),
            exp:    now + self.refresh_ttl.as_secs() as usize,
            iat:    now,
            family: family.to_owned(),
            jti:    Uuid::new_v4().to_string(), // ensures uniqueness within the same second
        };
        encode(&Header::default(), &claims, &self.encoding_key)
            .map_err(|e| AppError::Internal(format!("refresh-token encoding failed: {e}")))
    }

    /// Decode and validate a refresh token, returning the embedded
    /// [`RefreshClaims`].
    ///
    /// # Errors
    ///
    /// - [`AppError::Unauthorized`] if the token is expired, malformed, or has
    ///   an invalid signature.
    pub fn verify_refresh_token(&self, token: &str) -> AppResult<RefreshClaims> {
        let mut validation = Validation::new(Algorithm::HS256);
        validation.validate_exp = true;
        validation.leeway = 0; // strict: no clock-skew tolerance

        decode::<RefreshClaims>(token, &self.decoding_key, &validation)
            .map(|d| d.claims)
            .map_err(jwt_to_app_error)
    }
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

/// Return the current Unix timestamp in seconds.
fn unix_now_secs() -> usize {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock must be after UNIX epoch")
        .as_secs() as usize
}

/// Map `jsonwebtoken::errors::Error` to [`AppError`] with context.
fn jwt_to_app_error(e: jsonwebtoken::errors::Error) -> AppError {
    use jsonwebtoken::errors::ErrorKind;

    match e.kind() {
        ErrorKind::ExpiredSignature => {
            AppError::Unauthorized("token has expired".to_owned())
        }
        ErrorKind::InvalidSignature | ErrorKind::InvalidToken => {
            AppError::Unauthorized("token signature is invalid".to_owned())
        }
        _ => AppError::Unauthorized(format!("invalid token: {e}")),
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn manager() -> JwtManager {
        JwtManager::with_defaults("test-secret-at-least-32-bytes-long!!")
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
        let id = user_id();
        let mgr = manager();
        let (token, _) = mgr.generate_access_token(&id, Role::Admin).unwrap();
        let claims = mgr.verify_access_token(&token).unwrap();
        assert_eq!(claims.sub, id.to_string());
        assert_eq!(claims.role, Role::Admin);
    }

    // TDD task 2.3-3: expires_in matches the configured TTL
    #[test]
    fn test_access_token_expires_in_matches_ttl() {
        let ttl = Duration::from_secs(900);
        let mgr = JwtManager::new("secret-pad-to-32-bytes-xxxxxxxxx", ttl, DEFAULT_REFRESH_TTL);
        let (_, exp_secs) = mgr.generate_access_token(&user_id(), Role::Guest).unwrap();
        assert_eq!(exp_secs, 900);
    }

    // TDD task 2.3-4: expired access token is rejected
    #[test]
    fn test_expired_access_token_is_rejected() {
        // Use a 1-second TTL.  Sleep 2.1 s to ensure we land at least one full
        // second past expiry regardless of when within the second we started.
        let mgr = JwtManager::new(
            "secret-pad-to-32-bytes-xxxxxxxxx",
            Duration::from_secs(1),
            DEFAULT_REFRESH_TTL,
        );
        let (token, _) = mgr.generate_access_token(&user_id(), Role::Member).unwrap();
        std::thread::sleep(Duration::from_millis(2100));
        let err = mgr.verify_access_token(&token).unwrap_err();
        assert!(
            matches!(err, AppError::Unauthorized(_)),
            "expected Unauthorized, got {err:?}"
        );
    }

    // TDD task 2.3-5: tampered access token is rejected
    #[test]
    fn test_tampered_access_token_is_rejected() {
        let (token, _) = manager().generate_access_token(&user_id(), Role::Member).unwrap();
        let tampered = format!("{token}X"); // corrupt the signature
        let err = manager().verify_access_token(&tampered).unwrap_err();
        assert!(matches!(err, AppError::Unauthorized(_)));
    }

    // TDD task 2.3-6: token signed with a different secret is rejected
    #[test]
    fn test_token_from_different_secret_is_rejected() {
        let mgr_a = JwtManager::with_defaults("secret-A-pad-to-32-bytes-xxxxxxxxx");
        let mgr_b = JwtManager::with_defaults("secret-B-pad-to-32-bytes-xxxxxxxxx");
        let (token, _) = mgr_a.generate_access_token(&user_id(), Role::Member).unwrap();
        let err = mgr_b.verify_access_token(&token).unwrap_err();
        assert!(matches!(err, AppError::Unauthorized(_)));
    }

    // TDD task 2.3-7: refresh token carries correct sub and family
    #[test]
    fn test_refresh_token_round_trip() {
        let id  = user_id();
        let mgr = manager();
        let (token, family) = mgr.generate_refresh_token(&id).unwrap();
        let claims = mgr.verify_refresh_token(&token).unwrap();
        assert_eq!(claims.sub,    id.to_string());
        assert_eq!(claims.family, family);
    }

    // TDD task 2.3-8: two refresh tokens for the same user have distinct families
    #[test]
    fn test_refresh_tokens_have_unique_families() {
        let mgr = manager();
        let id  = user_id();
        let (_, family1) = mgr.generate_refresh_token(&id).unwrap();
        let (_, family2) = mgr.generate_refresh_token(&id).unwrap();
        assert_ne!(family1, family2);
    }
}
