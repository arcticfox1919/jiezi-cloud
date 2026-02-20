//! Integration tests for the RBAC permission engine.

use jiezi_cloud_auth::rbac::RbacEngine;
use jiezi_cloud_core::{models::user::Role, types::Action};

// Helper: iterate all defined actions.
fn all_actions() -> [Action; 5] {
    [Action::Read, Action::Write, Action::Delete, Action::Share, Action::Admin]
}

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
