//! User, role, permission and authentication payload models.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::types::{SpaceId, UserId};

// ─── Role ─────────────────────────────────────────────────────────────────────

/// User role — determines the base set of permissions.
/// Resource-level overrides can refine these defaults.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// Full system owner; has every permission everywhere.
    Owner,
    /// Space-level administrator; can manage members and settings.
    Admin,
    /// Regular member with read/write access to assigned spaces.
    Member,
    /// Read-only observer.
    Guest,
}

impl std::fmt::Display for Role {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Role::Owner  => "owner",
            Role::Admin  => "admin",
            Role::Member => "member",
            Role::Guest  => "guest",
        };
        write!(f, "{s}")
    }
}

// ─── Permission ───────────────────────────────────────────────────────────────

/// A permission grant scoped to an optional space.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Permission {
    /// The role this permission grant corresponds to.
    pub role: Role,
    /// When `Some`, the permission is scoped to a specific space.
    /// When `None`, it applies system-wide.
    pub space_id: Option<SpaceId>,
}

// ─── User ─────────────────────────────────────────────────────────────────────

/// Core user entity.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    pub id: UserId,
    pub username: String,
    pub email: String,

    /// Argon2id hash of the user's password.
    /// Excluded from serialisation to prevent accidental exposure.
    #[serde(skip_serializing)]
    pub password_hash: String,

    /// System-level role.
    pub role: Role,

    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,

    /// Whether the account is active (not suspended).
    pub is_active: bool,

    /// Optional display name shown in the UI.
    pub display_name: Option<String>,

    /// Optional URL to the user's avatar image.
    pub avatar_url: Option<String>,

    /// Maximum storage this user may consume across all their spaces,
    /// in bytes.  `None` means unlimited.
    pub storage_quota: Option<u64>,

    /// Total bytes currently consumed by this user.
    pub storage_used: u64,
}

// ─── JWT claims ───────────────────────────────────────────────────────────────

/// Claims embedded in a JWT access token.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claims {
    /// Subject — the user's ID serialised as a UUID string.
    pub sub: String,
    /// Expiry (Unix timestamp in seconds).
    pub exp: usize,
    /// Issued-at (Unix timestamp in seconds).
    pub iat: usize,
    /// The user's current system role.
    pub role: Role,
}

/// Claims embedded in a JWT refresh token.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefreshClaims {
    /// Subject — the user's ID serialised as a UUID string.
    pub sub: String,
    /// Expiry (Unix timestamp in seconds).
    pub exp: usize,
    /// Issued-at (Unix timestamp in seconds).
    pub iat: usize,
    /// Token family identifier used to detect refresh-token reuse attacks.
    /// A reuse within the same family immediately revokes all tokens for the user.
    pub family: String,
}

// ─── Token pair ───────────────────────────────────────────────────────────────

/// A short-lived access token paired with a longer-lived refresh token.
/// Returned after a successful login or token refresh.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenPair {
    pub access_token: String,
    pub refresh_token: String,
    /// Seconds until `access_token` expires.
    pub expires_in: u64,
}

// ─── Request DTOs ─────────────────────────────────────────────────────────────

/// Payload for the user registration endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegisterRequest {
    pub username: String,
    pub email: String,
    pub password: String,
    /// Optional friendly name shown in the UI.
    pub display_name: Option<String>,
}

/// Payload for the user login endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoginRequest {
    /// Either a username or an email address.
    pub credential: String,
    pub password: String,
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_role_display() {
        assert_eq!(Role::Owner.to_string(),  "owner");
        assert_eq!(Role::Admin.to_string(),  "admin");
        assert_eq!(Role::Member.to_string(), "member");
        assert_eq!(Role::Guest.to_string(),  "guest");
    }

    #[test]
    fn test_role_serde_round_trip() {
        for role in [Role::Owner, Role::Admin, Role::Member, Role::Guest] {
            let json = serde_json::to_string(&role).expect("serialize role");
            let back: Role = serde_json::from_str(&json).expect("deserialize role");
            assert_eq!(role, back);
        }
    }

    #[test]
    fn test_token_pair_serde_round_trip() {
        let pair = TokenPair {
            access_token:  "aaa.bbb.ccc".into(),
            refresh_token: "xxx.yyy.zzz".into(),
            expires_in:    900,
        };
        let json = serde_json::to_string(&pair).expect("serialize");
        let back: TokenPair = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(pair.access_token,  back.access_token);
        assert_eq!(pair.refresh_token, back.refresh_token);
        assert_eq!(pair.expires_in,    back.expires_in);
    }

    #[test]
    fn test_user_password_hash_not_serialized() {
        use crate::types::UserId;
        let user = User {
            id:             UserId::new(),
            username:       "alice".into(),
            email:          "alice@example.com".into(),
            password_hash:  "secret_hash".into(),
            role:           Role::Member,
            created_at:     Utc::now(),
            updated_at:     Utc::now(),
            is_active:      true,
            display_name:   None,
            avatar_url:     None,
            storage_quota:  None,
            storage_used:   0,
        };
        let json = serde_json::to_string(&user).expect("serialize user");
        assert!(
            !json.contains("secret_hash"),
            "password_hash must never appear in serialised output"
        );
    }

    #[test]
    fn test_register_request_serde_round_trip() {
        let req = RegisterRequest {
            username:     "bob".into(),
            email:        "bob@example.com".into(),
            password:     "hunter2".into(),
            display_name: Some("Bob Smith".into()),
        };
        let json = serde_json::to_string(&req).expect("serialize");
        let back: RegisterRequest = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(req.username, back.username);
        assert_eq!(req.email,    back.email);
    }
}
