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
        self.storage_quota
            .map_or(false, |quota| self.storage_used > quota)
    }

    /// Return remaining storage capacity in bytes, or `None` if unlimited.
    pub fn remaining_quota(&self) -> Option<u64> {
        self.storage_quota
            .map(|quota| quota.saturating_sub(self.storage_used))
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

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{FileId, SpaceId, UserId};

    fn make_space(quota: Option<u64>, used: u64) -> Space {
        Space {
            id:            SpaceId::new(),
            name:          "My Files".into(),
            description:   None,
            owner_id:      UserId::new(),
            root_id:       FileId::new(),
            storage_quota: quota,
            storage_used:  used,
            created_at:    Utc::now(),
            updated_at:    Utc::now(),
        }
    }

    #[test]
    fn test_space_quota_not_exceeded_when_under_limit() {
        let space = make_space(Some(100), 50);
        assert!(!space.is_quota_exceeded());
    }

    #[test]
    fn test_space_quota_exceeded_when_over_limit() {
        let space = make_space(Some(100), 150);
        assert!(space.is_quota_exceeded());
    }

    #[test]
    fn test_space_quota_never_exceeded_when_unlimited() {
        let space = make_space(None, u64::MAX);
        assert!(!space.is_quota_exceeded());
    }

    #[test]
    fn test_space_remaining_quota_calculated_correctly() {
        let space = make_space(Some(100), 40);
        assert_eq!(space.remaining_quota(), Some(60));
    }

    #[test]
    fn test_space_remaining_quota_saturates_at_zero() {
        let space = make_space(Some(100), 200);
        assert_eq!(space.remaining_quota(), Some(0));
    }

    #[test]
    fn test_space_serde_round_trip() {
        let space = make_space(Some(1_073_741_824), 512_000);
        let json  = serde_json::to_string(&space).expect("serialize");
        let back: Space = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(space.id,   back.id);
        assert_eq!(space.name, back.name);
    }

    #[test]
    fn test_space_member_serde_round_trip() {
        let member = SpaceMember {
            space_id:  SpaceId::new(),
            user_id:   UserId::new(),
            role:      Role::Member,
            joined_at: Utc::now(),
        };
        let json = serde_json::to_string(&member).expect("serialize");
        let back: SpaceMember = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(member.role, back.role);
    }
}
