//! JWT access-token and refresh-token generation and validation.
//!
//! # Algorithm
//!
//! All tokens use **ES256** (ECDSA with P-256 and SHA-256), an *asymmetric*
//! algorithm.  This is the central security property:
//!
//! | Component           | Key held          | Can sign? | Can verify? |
//! |---------------------|-------------------|-----------|-------------|
//! | Home server         | Private key (PEM) | ✅        | ✅          |
//! | Tunnel (`JwtVerifier`) | Public key (PEM) | ❌       | ✅          |
//!
//! Compromising the tunnel does **not** grant token-forging capability — the
//! private key never leaves the home server.
//!
//! # Key delivery to the tunnel
//!
//! The tunnel does not fetch the public key over the network; it has no way to
//! reach the home server (it sits behind NAT).  Instead, the home server
//! **pushes** the public key as part of the `RegisterNode` message sent over
//! the HMAC-authenticated WebSocket control channel at startup.
//! See [`JwtManager::public_key_pem`].
//!
//! # Token strategy
//!
//! | Token type    | TTL (default) | Stored server-side? | Purpose                        |
//! |---------------|---------------|---------------------|--------------------------------|
//! | Access token  | 15 min        | No                  | Authorise API requests         |
//! | Refresh token | 30 days       | Yes (hash only)     | Obtain a new access token pair |
//!
//! # Rotation & reuse detection
//!
//! Every refresh token belongs to a **family** (a random UUID minted at
//! login).  On each successful refresh, the old token is revoked and a new
//! one in the same family is issued.  If a *revoked* family member is
//! presented, the entire family is wiped, forcing re-authentication.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation, decode, encode};
use p256::pkcs8::{DecodePrivateKey, EncodePublicKey, LineEnding};
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

/// Handles JWT signing, encoding, decoding and validation for the home server.
///
/// Holds both the private key (for signing) and the derived public key (for
/// verification and for pushing to the tunnel during node registration).
///
/// Immutable once constructed — safe to wrap in [`Arc`] and share across threads.
///
/// [`Arc`]: std::sync::Arc
pub struct JwtManager {
    encoding_key:   EncodingKey,
    decoding_key:   DecodingKey,
    /// SPKI PEM of the public key, sent to tunnel in `RegisterNode`.
    public_key_pem: String,
    access_ttl:     Duration,
    refresh_ttl:    Duration,
}

impl JwtManager {
    /// Construct a `JwtManager` from a PKCS#8 PEM-encoded P-256 private key.
    ///
    /// The public key is derived automatically and stored for later retrieval
    /// via [`public_key_pem`](Self::public_key_pem).
    pub fn from_pkcs8_pem(
        private_key_pem: &str,
        access_ttl:      Duration,
        refresh_ttl:     Duration,
    ) -> AppResult<Self> {
        // Parse the private key and derive the public key PEM from it.
        let secret_key = p256::SecretKey::from_pkcs8_pem(private_key_pem)
            .map_err(|e| AppError::Internal(format!("jwt private key parse failed: {e}")))?;
        let public_key_pem = secret_key
            .public_key()
            .to_public_key_pem(LineEnding::LF)
            .map_err(|e| AppError::Internal(format!("jwt public key export failed: {e}")))?;

        let encoding_key = EncodingKey::from_ec_pem(private_key_pem.as_bytes())
            .map_err(|e| AppError::Internal(format!("jwt encoding key build failed: {e}")))?;
        let decoding_key = DecodingKey::from_ec_pem(public_key_pem.as_bytes())
            .map_err(|e| AppError::Internal(format!("jwt decoding key build failed: {e}")))?;

        Ok(Self { encoding_key, decoding_key, public_key_pem, access_ttl, refresh_ttl })
    }

    /// Generate a fresh P-256 keypair and return a ready-to-use `JwtManager`.
    ///
    /// Intended for **development** use when `jwt_private_key_pem = "GENERATE"`
    /// is set in the config.  The private key is ephemeral — tokens will be
    /// invalidated on the next server restart.
    pub fn generate(access_ttl: Duration, refresh_ttl: Duration) -> AppResult<Self> {
        use p256::pkcs8::EncodePrivateKey;

        let secret_key = p256::SecretKey::random(&mut rand_core::OsRng);
        let private_key_pem = secret_key
            .to_pkcs8_pem(LineEnding::LF)
            .map_err(|e| AppError::Internal(format!("keypair pem export failed: {e}")))?;
        Self::from_pkcs8_pem(private_key_pem.as_str(), access_ttl, refresh_ttl)
    }

    /// Return the SPKI PEM-encoded public key.
    ///
    /// Include this in the `RegisterNode` message sent to the tunnel over the
    /// authenticated WebSocket control channel so the tunnel can verify client
    /// JWTs without holding any private/secret material.
    pub fn public_key_pem(&self) -> &str {
        &self.public_key_pem
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

        let token = encode(&Header::new(Algorithm::ES256), &claims, &self.encoding_key)
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
        verify_with_key(token, &self.decoding_key)
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

    /// Decode and validate a refresh token, returning the embedded [`RefreshClaims`].
    pub fn verify_refresh_token(&self, token: &str) -> AppResult<RefreshClaims> {
        let mut validation = Validation::new(Algorithm::ES256);
        validation.validate_exp = true;
        validation.leeway = 0;

        decode::<RefreshClaims>(token, &self.decoding_key, &validation)
            .map(|d| d.claims)
            .map_err(jwt_to_app_error)
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
        encode(&Header::new(Algorithm::ES256), &claims, &self.encoding_key)
            .map_err(|e| AppError::Internal(format!("refresh-token encoding failed: {e}")))
    }
}

// ─── JwtVerifier ─────────────────────────────────────────────────────────────

/// Verify-only JWT validator — holds **only** the public key.
///
/// Intended for use in `jiezi-cloud-tunnel`.  Because it holds no private key,
/// it is cryptographically incapable of forging tokens; compromise of this
/// component does not grant token-issuance capability.
///
/// **Construction**: the public key PEM is received from the home server inside
/// the `RegisterNode` message on the HMAC-authenticated WebSocket control
/// channel.  The tunnel never fetches it over an unauthenticated connection,
/// and has no way to reach the home server directly (it sits behind NAT).
pub struct JwtVerifier {
    decoding_key: DecodingKey,
}

impl JwtVerifier {
    /// Parse a SPKI PEM-encoded P-256 public key.
    pub fn from_spki_pem(public_key_pem: &str) -> AppResult<Self> {
        let decoding_key = DecodingKey::from_ec_pem(public_key_pem.as_bytes())
            .map_err(|e| AppError::Internal(format!("jwt public key parse failed: {e}")))?;
        Ok(Self { decoding_key })
    }

    /// Decode and validate an access token, returning the embedded [`Claims`].
    pub fn verify_access_token(&self, token: &str) -> AppResult<Claims> {
        verify_with_key(token, &self.decoding_key)
    }
}

// ─── Shared helpers ───────────────────────────────────────────────────────────

/// Verify an ES256 access token against any [`DecodingKey`].
/// Shared between [`JwtManager`] and [`JwtVerifier`] to keep validation
/// logic consistent.
fn verify_with_key(token: &str, key: &DecodingKey) -> AppResult<Claims> {
    let mut validation = Validation::new(Algorithm::ES256);
    validation.validate_exp = true;
    validation.leeway = 0; // strict: no clock-skew tolerance

    decode::<Claims>(token, key, &validation)
        .map(|d| d.claims)
        .map_err(jwt_to_app_error)
}

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
}
