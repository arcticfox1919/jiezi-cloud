//! Domain events for the internal in-process event bus.
//!
//! # Design
//!
//! Events are published over Tokio `broadcast` / `mpsc` channels and consumed
//! by subscriber tasks — for example, the search indexer subscribes to
//! [`DomainEvent::FileUploaded`] to trigger content indexing.
//!
//! Using events instead of direct calls keeps modules decoupled: the file
//! upload handler only knows how to persist bytes; it does not know or care
//! about search, thumbnails, or audit logging.
//!
//! # Serialisation
//!
//! All events derive `Serialize` / `Deserialize` so they can optionally be
//! forwarded to an external message broker (Redis Streams, NATS, etc.) when
//! running in a cluster.

use serde::{Deserialize, Serialize};

use crate::models::user::Role;
use crate::types::{FileId, ShareId, SpaceId, TaskId, UserId};

/// The canonical set of domain events emitted within the platform.
///
/// The `#[serde(tag = "type")]` attribute produces a JSON object with a `type`
/// discriminant field, which is suitable for logging and external message bus
/// forwarding:
///
/// ```json
/// { "type": "file_uploaded", "file_id": "…", "user_id": "…", "space_id": "…" }
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DomainEvent {
    // ── File lifecycle ─────────────────────────────────────────────────────
    /// All chunks for a new file have been persisted and the file record is visible.
    FileUploaded { file_id: FileId, user_id: UserId, space_id: SpaceId },

    /// A file or directory has been soft-deleted (moved to trash).
    FileDeleted { file_id: FileId, user_id: UserId },

    /// A file or directory has been permanently removed (trash emptied).
    FilePermanentlyDeleted { file_id: FileId, user_id: UserId },

    /// A file or directory has been renamed and/or moved to a new parent.
    FileMoved {
        file_id: FileId,
        user_id: UserId,
        /// `None` when the item is only renamed (parent unchanged).
        new_parent_id: Option<FileId>,
        old_name: String,
        new_name: String,
    },

    /// A soft-deleted node has been restored from the trash.
    FileRestored { file_id: FileId, user_id: UserId },

    // ── Share lifecycle ────────────────────────────────────────────────────
    /// A new share link has been created for a file or directory.
    FileShared { file_id: FileId, share_id: ShareId, created_by: UserId },

    /// A share link has been revoked.
    ShareRevoked { share_id: ShareId, revoked_by: UserId },

    // ── User lifecycle ─────────────────────────────────────────────────────
    /// A new user account has been created (after successful registration).
    UserRegistered { user_id: UserId },

    /// A user's system-level role has changed.
    UserRoleChanged { user_id: UserId, old_role: Role, new_role: Role },

    /// A user account has been deactivated.
    UserDeactivated { user_id: UserId },

    // ── Space lifecycle ────────────────────────────────────────────────────
    /// A new collaborative or personal space has been created.
    SpaceCreated { space_id: SpaceId, owner_id: UserId },

    /// A collaborator has been added to a space.
    SpaceMemberAdded { space_id: SpaceId, user_id: UserId, role: Role, added_by: UserId },

    /// A collaborator has been removed from a space.
    SpaceMemberRemoved { space_id: SpaceId, user_id: UserId, removed_by: UserId },

    // ── Background tasks ───────────────────────────────────────────────────
    /// A background task has changed state.
    TaskStatusChanged {
        task_id: TaskId,
        user_id: Option<UserId>,
        /// Human-readable task type label (e.g. `"thumbnail_generation"`).
        kind: String,
        /// New status as a string (e.g. `"completed"`).
        status: String,
    },
}