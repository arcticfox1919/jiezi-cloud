//! Share link models for publicly accessible or password-protected file shares.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::types::{FileId, ShareId, UserId};

// ─── ShareAccess ──────────────────────────────────────────────────────────────

/// The level of access granted to a share link recipient.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShareAccess {
    /// The recipient may only read and download files.
    ReadOnly,
    /// The recipient may read, download, and upload files.
    ReadWrite,
}

// ─── ShareLink ────────────────────────────────────────────────────────────────

/// A shareable link that grants access to a file or directory.
///
/// Links may be optionally protected by a password, have an expiry date, and
/// limit the total number of downloads allowed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShareLink {
    pub id: ShareId,
    /// The file or directory being shared.
    pub file_id: FileId,
    /// User who created this link.
    pub created_by: UserId,
    /// Permissions granted to the link holder.
    pub access: ShareAccess,
    /// Argon2id hash of the optional access password.
    #[serde(skip_serializing)]
    pub password_hash: Option<String>,
    /// Optional expiry datetime after which the link becomes invalid.
    pub expires_at: Option<DateTime<Utc>>,
    /// Maximum number of downloads allowed.  `None` means unlimited.
    pub download_limit: Option<u32>,
    /// Total number of downloads recorded so far.
    pub download_count: u32,
    pub created_at: DateTime<Utc>,
}

impl ShareLink {
    /// Return `true` if this link is still valid at the given point in time.
    pub fn is_valid_at(&self, now: &DateTime<Utc>) -> bool {
        if let Some(ref exp) = self.expires_at
            && now > exp
        {
            return false;
        }
        if let Some(limit) = self.download_limit
            && self.download_count >= limit
        {
            return false;
        }
        true
    }

    /// Return `true` if this link requires a password to access.
    pub fn is_password_protected(&self) -> bool {
        self.password_hash.is_some()
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{FileId, ShareId, UserId};

    fn make_link(
        expires_at: Option<DateTime<Utc>>,
        download_limit: Option<u32>,
        download_count: u32,
    ) -> ShareLink {
        ShareLink {
            id: ShareId::new(),
            file_id: FileId::new(),
            created_by: UserId::new(),
            access: ShareAccess::ReadOnly,
            password_hash: None,
            expires_at,
            download_limit,
            download_count,
            created_at: Utc::now(),
        }
    }

    #[test]
    fn test_valid_link_with_no_constraints() {
        let link = make_link(None, None, 0);
        assert!(link.is_valid_at(&Utc::now()));
    }

    #[test]
    fn test_expired_link_is_invalid() {
        use chrono::Duration;
        let expired_at = Utc::now() - Duration::hours(1);
        let link = make_link(Some(expired_at), None, 0);
        assert!(!link.is_valid_at(&Utc::now()));
    }

    #[test]
    fn test_future_expiry_link_is_valid() {
        use chrono::Duration;
        let expires_at = Utc::now() + Duration::hours(24);
        let link = make_link(Some(expires_at), None, 0);
        assert!(link.is_valid_at(&Utc::now()));
    }

    #[test]
    fn test_download_limit_exhausted_invalidates_link() {
        let link = make_link(None, Some(5), 5);
        assert!(!link.is_valid_at(&Utc::now()));
    }

    #[test]
    fn test_download_limit_not_exhausted_keeps_link_valid() {
        let link = make_link(None, Some(5), 4);
        assert!(link.is_valid_at(&Utc::now()));
    }

    #[test]
    fn test_password_protection_detection() {
        let mut link = make_link(None, None, 0);
        assert!(!link.is_password_protected());
        link.password_hash = Some("$argon2id$...".into());
        assert!(link.is_password_protected());
    }

    #[test]
    fn test_share_link_password_hash_not_serialized() {
        let mut link = make_link(None, None, 0);
        link.password_hash = Some("secret_hash".into());
        let json = serde_json::to_string(&link).expect("serialize");
        assert!(!json.contains("secret_hash"), "password_hash must be skipped in JSON output");
    }

    #[test]
    fn test_share_link_serde_round_trip() {
        let link = make_link(None, Some(10), 3);
        let json = serde_json::to_string(&link).expect("serialize");
        let back: ShareLink = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(link.id, back.id);
        assert_eq!(link.download_limit, back.download_limit);
        assert_eq!(link.download_count, back.download_count);
    }
}
