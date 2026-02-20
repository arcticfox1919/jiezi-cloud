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
