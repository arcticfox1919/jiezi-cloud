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
