//! User, role, permission and authentication payload models.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::types::{SpaceId, UserId};

// ─── Role ─────────────────────────────────────────────────────────────────────

/// User role — determines the base set of permissions.
/// Resource-level overrides can refine these defaults.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, utoipa::ToSchema)]
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
            Role::Owner => "owner",
            Role::Admin => "admin",
            Role::Member => "member",
            Role::Guest => "guest",
        };
        write!(f, "{s}")
    }
}

// ─── Permission ───────────────────────────────────────────────────────────────

/// A permission grant scoped to an optional space.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Permission {
    /// The role this permission grant corresponds to.
    pub role: Role,
    /// When `Some`, the permission is scoped to a specific space.
    /// When `None`, it applies system-wide.
    pub space_id: Option<SpaceId>,
}

// ─── User ─────────────────────────────────────────────────────────────────────

/// Core user entity.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
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

    /// Running total of bytes consumed.
    pub storage_used: u64,

    // ─ Brute-force lockout (not serialised — internal security state) ────

    /// Number of consecutive failed login attempts since the last successful
    /// login.  Reset to 0 after a successful authentication.
    #[serde(skip)]
    pub failed_login_count: i32,

    /// When set, the account is temporarily locked and `login()` will reject
    /// all attempts until this timestamp passes.
    #[serde(skip)]
    pub locked_until: Option<DateTime<Utc>>,

    /// Timestamp of the most recent failed login attempt — helps the service
    /// decide whether to extend or reset the lockout window.
    #[serde(skip)]
    pub last_failed_login_at: Option<DateTime<Utc>>,

    // ─ Email verification ─────────────────────────────────────────

    /// Whether the user has verified their email address via OTP.
    ///
    /// When `email.verification_required = true` in config, users must supply
    /// the correct OTP code during registration before the account is confirmed.
    pub email_verified: bool,
}

// ─── JWT claims ───────────────────────────────────────────────────────────────

/// Claims embedded in a JWT access token.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
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
    /// JWT ID — a random UUID making every token cryptographically unique even
    /// when other claims are identical (e.g. rapid rotation within the same second).
    pub jti: String,
}

// ─── Token pair ───────────────────────────────────────────────────────────────

/// A short-lived access token paired with a longer-lived refresh token.
/// Returned after a successful login or token refresh.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct TokenPair {
    pub access_token: String,
    pub refresh_token: String,
    /// Seconds until `access_token` expires.
    pub expires_in: u64,
}

// ─── Request DTOs ─────────────────────────────────────────────────────────────

/// Payload for the user registration endpoint.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct RegisterRequest {
    pub username: String,
    pub email: String,
    pub password: String,
    /// Optional friendly name shown in the UI.
    pub display_name: Option<String>,
    /// OTP code sent to `email` via `POST /auth/send-register-otp`.
    ///
    /// Required when `email.verification_required = true` in config;
    /// ignored (and may be absent) otherwise.
    pub email_otp: Option<String>,
}

/// Payload for updating one's own profile (display name, avatar).
///
/// Each field is doubly-wrapped in `Option` so the client can distinguish
/// "omit this field" (`None`) from "clear this field" (`Some(None)`).
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct UpdateProfileRequest {
    /// Pass `Some(Some("Alice"))` to set, `Some(None)` to clear.
    pub display_name: Option<Option<String>>,
    /// Pass `Some(Some("https://…"))` to set, `Some(None)` to clear.
    pub avatar_url:   Option<Option<String>>,
}

/// Payload for `POST /auth/me/password` — change own password.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ChangeOwnPasswordRequest {
    /// The caller's current password (verified before accepting the change).
    pub old_password: String,
    /// The desired new password (minimum 8 characters).
    pub new_password: String,
    /// 6-digit OTP sent to the account email.  Required when
    /// `email_verification_required` is enabled in config.
    pub email_otp: Option<String>,
}

/// Payload for `PATCH /admin/users/{id}/role` — change a user's system role.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ChangeRoleRequest {
    pub new_role: Role,
}

/// Payload for `PATCH /admin/users/{id}/status` — suspend or reactivate.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SetActiveRequest {
    pub is_active: bool,
}

/// Payload for `POST /admin/users/{id}/reset-password` — admin force-reset.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct AdminResetPasswordRequest {
    /// The new password to assign.  Minimum 8 characters.
    pub new_password: String,
}

/// Payload for `PATCH /admin/users/{id}/quota` — set storage quota.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SetQuotaRequest {
    /// Storage maximum in bytes.  `null` means unlimited.
    pub storage_quota: Option<u64>,
}

/// Payload for `POST /auth/logout` — revoke a refresh token (log out a device).
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct LogoutRequest {
    pub refresh_token: String,
}

/// Payload for `POST /auth/send-register-otp` — request an OTP for registration.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SendOtpRequest {
    /// The email address to send the OTP to.
    pub email: String,
}

/// Payload for `POST /auth/reset-password` — reset password using an OTP code.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ResetPasswordWithOtpRequest {
    pub email:        String,
    /// The 6-digit OTP that was emailed via `POST /auth/forgot-password`.
    pub code:         String,
    pub new_password: String,
}

/// Payload for `POST /auth/unlock-account` — unlock a locked account using OTP.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct UnlockWithOtpRequest {
    pub email: String,
    /// The 6-digit OTP that was emailed via `POST /auth/send-unlock-otp`.
    pub code:  String,
}

/// Payload for the user login endpoint.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct LoginRequest {
    /// Either a username or an email address.
    pub credential: String,
    pub password: String,
    /// Optional human-readable label for the device initiating the login.
    /// Shown in the active sessions list so users can identify and revoke
    /// specific devices (e.g. "iPhone 15", "Home PC — Firefox").
    pub device_label: Option<String>,
}

// ─── Session info ─────────────────────────────────────────────────────────────

/// Represents a single active login session visible to the user.
///
/// Identified by its `family` UUID, which stays constant across all
/// refresh-token rotations within the same login session.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SessionInfo {
    /// Stable identifier for this session (JWT rotation family UUID).
    pub family: String,
    /// Human-readable label supplied by the client at login time.
    pub device_label: Option<String>,
    /// When the session was first created (i.e., when the user logged in).
    pub session_started: DateTime<Utc>,
    /// When the active refresh token in this session expires.
    pub expires_at: DateTime<Utc>,
}
