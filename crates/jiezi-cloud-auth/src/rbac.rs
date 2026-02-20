//! Role-Based Access Control (RBAC) permission engine.
//!
//! # Design
//!
//! Phase 2 implements a **pure-function, role-only** permission model.
//! Future phases will layer resource-level ACLs on top, allowing per-file /
//! per-space overrides.
//!
//! ## Role hierarchy (most → least privileged)
//!
//! ```text
//! Owner  > Admin  > Member  > Guest
//! ```
//!
//! ## Action matrix
//!
//! | Action  | Owner | Admin | Member | Guest |
//! |---------|-------|-------|--------|-------|
//! | Read    | ✓     | ✓     | ✓      | ✓     |
//! | Write   | ✓     | ✓     | ✓      | ✗     |
//! | Delete  | ✓     | ✓     | ✓      | ✗     |
//! | Share   | ✓     | ✓     | ✓      | ✗     |
//! | Admin   | ✓     | ✗     | ✗      | ✗     |
//!
//! The `Admin` action covers system-level operations (e.g. managing users,
//! changing server configuration).  Only the `Owner` role grants it.

use jiezi_cloud_core::{
    models::user::Role,
    types::Action,
};

/// Stateless RBAC engine.
///
/// `RbacEngine` is a zero-sized type — instantiate it with `RbacEngine` and
/// call `RbacEngine::is_permitted` directly.  No heap allocation required.
pub struct RbacEngine;

impl RbacEngine {
    /// Return `true` if a user with `role` is permitted to perform `action`.
    ///
    /// This is a **system-level** check and does not account for
    /// resource-ownership or space-scoped overrides (those will be resolved by
    /// a higher-level `PermissionService` in a future phase).
    pub fn is_permitted(role: Role, action: Action) -> bool {
        match role {
            // Owner may do anything.
            Role::Owner => true,

            // Admin may do everything except system-level administration.
            Role::Admin => !matches!(action, Action::Admin),

            // Member may read, write, delete and share — but not administer.
            Role::Member => matches!(
                action,
                Action::Read | Action::Write | Action::Delete | Action::Share
            ),

            // Guest is entirely read-only.
            Role::Guest => matches!(action, Action::Read),
        }
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use jiezi_cloud_core::{models::user::Role, types::Action};

    // TDD task 2.4-1: Owner has all permissions
    #[test]
    fn test_owner_has_all_permissions() {
        for action in all_actions() {
            assert!(
                RbacEngine::is_permitted(Role::Owner, action),
                "Owner must be permitted for {action:?}"
            );
        }
    }

    // TDD task 2.4-2: Admin has all permissions except Admin action
    #[test]
    fn test_admin_cannot_perform_system_admin_action() {
        assert!(!RbacEngine::is_permitted(Role::Admin, Action::Admin));
    }

    #[test]
    fn test_admin_can_read_write_delete_share() {
        for action in [Action::Read, Action::Write, Action::Delete, Action::Share] {
            assert!(
                RbacEngine::is_permitted(Role::Admin, action),
                "Admin must be permitted for {action:?}"
            );
        }
    }

    // TDD task 2.4-3: Member can read/write/delete/share but not admin
    #[test]
    fn test_member_can_read_write_delete_share() {
        for action in [Action::Read, Action::Write, Action::Delete, Action::Share] {
            assert!(
                RbacEngine::is_permitted(Role::Member, action),
                "Member must be permitted for {action:?}"
            );
        }
    }

    #[test]
    fn test_member_cannot_perform_admin_action() {
        assert!(!RbacEngine::is_permitted(Role::Member, Action::Admin));
    }

    // TDD task 2.4-4: Guest is read-only
    #[test]
    fn test_guest_can_only_read() {
        assert!(RbacEngine::is_permitted(Role::Guest, Action::Read));
    }

    #[test]
    fn test_guest_cannot_write_delete_share_or_admin() {
        for action in [Action::Write, Action::Delete, Action::Share, Action::Admin] {
            assert!(
                !RbacEngine::is_permitted(Role::Guest, action),
                "Guest must NOT be permitted for {action:?}"
            );
        }
    }

    // Helper: iterate all defined actions.
    fn all_actions() -> [Action; 5] {
        [Action::Read, Action::Write, Action::Delete, Action::Share, Action::Admin]
    }
}
