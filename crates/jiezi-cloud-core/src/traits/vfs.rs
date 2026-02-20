//! Virtual file system service trait.

use async_trait::async_trait;

use crate::error::AppResult;
use crate::models::file::FileNode;
use crate::types::{FileId, PageRequest, PageResponse, UserId};

/// Virtual file system service contract.
///
/// Manages the metadata tree (directories and files) independently of where
/// raw bytes are physically stored.  All file content I/O is handled by
/// [`crate::traits::storage::StorageBackend`].
///
/// # Directory tree implementation note
///
/// The concrete implementation in `jiezi-cloud-vfs` uses a **closure table**
/// (ancestor–descendant pairs) in the database so that arbitrary-depth
/// subtree queries run in a single SQL join.
#[async_trait]
pub trait VfsService: Send + Sync {
    /// Create an empty directory under `parent_id`.
    ///
    /// # Errors
    ///
    /// - [`AppError::NotFound`] if `parent_id` does not exist.
    /// - [`AppError::Conflict`] if a child with `name` already exists in the parent.
    /// - [`AppError::Forbidden`] if the caller lacks write permission on `parent_id`.
    async fn create_directory(
        &self,
        parent_id: &FileId,
        name: &str,
        owner: &UserId,
    ) -> AppResult<FileNode>;

    /// Retrieve a single node by its ID.
    ///
    /// # Errors
    ///
    /// - [`AppError::NotFound`] if no node with `id` exists (or it is deleted).
    async fn get_node(&self, id: &FileId) -> AppResult<FileNode>;

    /// List immediate children of a directory, paginated.
    ///
    /// Deleted nodes are excluded unless the caller queries the trash explicitly.
    ///
    /// # Errors
    ///
    /// - [`AppError::NotFound`] if `parent_id` does not exist.
    async fn list_children(
        &self,
        parent_id: &FileId,
        page: &PageRequest,
    ) -> AppResult<PageResponse<FileNode>>;

    /// Rename a node in place, without changing its parent.
    ///
    /// # Errors
    ///
    /// - [`AppError::NotFound`] if `id` does not exist.
    /// - [`AppError::Conflict`] if a sibling with `new_name` already exists.
    async fn rename(&self, id: &FileId, new_name: &str) -> AppResult<FileNode>;

    /// Move a node to a different parent directory.
    ///
    /// # Errors
    ///
    /// - [`AppError::NotFound`] if either `id` or `new_parent` does not exist.
    /// - [`AppError::Conflict`] if a sibling with the same name already exists.
    /// - [`AppError::Validation`] if `new_parent` is a descendant of `id`
    ///   (cycle prevention).
    async fn move_node(&self, id: &FileId, new_parent: &FileId) -> AppResult<FileNode>;

    /// Soft-delete a node by moving it to the trash.
    ///
    /// All descendants of a directory are also soft-deleted.
    ///
    /// # Errors
    ///
    /// - [`AppError::NotFound`] if `id` does not exist.
    async fn soft_delete(&self, id: &FileId) -> AppResult<()>;

    /// Restore a soft-deleted node to its original location.
    ///
    /// # Errors
    ///
    /// - [`AppError::NotFound`] if `id` does not exist or is not in the trash.
    /// - [`AppError::Conflict`] if a live node with the same name already
    ///   occupies the original parent.
    async fn restore(&self, id: &FileId) -> AppResult<FileNode>;

    /// Permanently delete a node and all its descendants.
    ///
    /// This operation is irreversible.  The caller is responsible for ensuring
    /// that any associated storage chunks are also garbage-collected.
    ///
    /// # Errors
    ///
    /// - [`AppError::NotFound`] if `id` does not exist.
    async fn permanent_delete(&self, id: &FileId) -> AppResult<()>;

    /// Return all soft-deleted nodes owned by `owner`.
    ///
    /// Equivalent to a per-user "trash can" view.
    async fn list_trash(&self, owner: &UserId) -> AppResult<Vec<FileNode>>;

    /// Copy a node (and its subtree for directories) under `new_parent`.
    ///
    /// Returns the root of the copied subtree.  New IDs are assigned to all
    /// copied nodes; content chunks are not duplicated (shared via reference).
    ///
    /// # Errors
    ///
    /// - [`AppError::NotFound`] if `id` or `new_parent` does not exist.
    /// - [`AppError::Conflict`] if a sibling with the same name already exists.
    async fn copy_node(
        &self,
        id: &FileId,
        new_parent: &FileId,
        owner: &UserId,
    ) -> AppResult<FileNode>;

    /// Create a file record in the VFS metadata tree.
    ///
    /// This is called by the **upload subsystem** once raw bytes have been
    /// safely persisted to storage backends.  Generic VFS clients (directory
    /// browsing, rename, etc.) should not call this directly.
    ///
    /// # Errors
    ///
    /// - [`AppError::NotFound`] if `parent_id` does not exist.
    /// - [`AppError::Conflict`] if a child named `name` already exists.
    async fn create_file_record(
        &self,
        parent_id: &FileId,
        name: &str,
        size: u64,
        content_hash: Option<String>,
        mime_type: Option<String>,
        owner: &UserId,
    ) -> AppResult<FileNode>;
}
