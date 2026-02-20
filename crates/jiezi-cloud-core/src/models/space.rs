//! Space and space-membership models.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::models::user::Role;
use crate::types::{FileId, SpaceId, UserId};

// ─── Space ────────────────────────────────────────────────────────────────────

/// A logical container for files and directories.
///
/// Each user has at least one personal space.  Additional collaborative spaces
/// can be created and shared with other users.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Space {
    pub id: SpaceId,
    pub name: String,
    pub description: Option<String>,
    /// The user who owns this space.
    pub owner_id: UserId,
    /// ID of the root [`crate::models::file::FileNode`] for this space.
    pub root_id: FileId,
    /// Maximum bytes this space may consume.  `None` means unlimited.
    pub storage_quota: Option<u64>,
    /// Bytes currently used by files in this space.
    pub storage_used: u64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Space {
    /// Return `true` if the space has a storage quota and it is exceeded.
    pub fn is_quota_exceeded(&self) -> bool {
        self.storage_quota.is_some_and(|quota| self.storage_used > quota)
    }

    /// Return remaining storage capacity in bytes, or `None` if unlimited.
    pub fn remaining_quota(&self) -> Option<u64> {
        self.storage_quota.map(|quota| quota.saturating_sub(self.storage_used))
    }
}

// ─── SpaceMember ─────────────────────────────────────────────────────────────

/// Associates a user with a space and records their role within it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpaceMember {
    pub space_id: SpaceId,
    pub user_id: UserId,
    /// The member's role within this specific space (may differ from their
    /// system-level role).
    pub role: Role,
    pub joined_at: DateTime<Utc>,
}
