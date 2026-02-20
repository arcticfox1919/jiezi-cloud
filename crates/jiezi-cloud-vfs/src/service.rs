//! Concrete implementation of [`jiezi_cloud_core::traits::vfs::VfsService`].
//!
//! [`VfsServiceImpl`] is a thin delegating wrapper around
//! [`crate::repository::FileNodeRepository`] that satisfies the `VfsService`
//! trait.  All business logic lives in the repository layer so it can be
//! tested without the trait boundary.

use async_trait::async_trait;

use jiezi_cloud_core::{
    error::AppResult,
    models::file::FileNode,
    traits::vfs::VfsService,
    types::{FileId, PageRequest, PageResponse, SpaceId, UserId},
};

use crate::repository::FileNodeRepository;

// ─── VfsServiceImpl ───────────────────────────────────────────────────────────

/// Production implementation of [`VfsService`].
///
/// Cheap to clone (the inner [`FileNodeRepository`] holds a reference-counted
/// database connection).
#[derive(Clone)]
pub struct VfsServiceImpl {
    repo: FileNodeRepository,
}

impl VfsServiceImpl {
    /// Construct a new service instance.
    pub fn new(repo: FileNodeRepository) -> Self {
        Self { repo }
    }

    /// Create the root directory node for a brand-new space.
    ///
    /// This entry point is **not** part of the [`VfsService`] trait because it
    /// is called once at space-creation time, before any child nodes exist.
    pub async fn create_root_space(
        &self,
        space_id: SpaceId,
        owner: &UserId,
    ) -> AppResult<FileNode> {
        self.repo.create_root(space_id, *owner).await
    }

    /// List soft-deleted nodes owned by `owner`.
    pub async fn list_trash(&self, owner: &UserId) -> AppResult<Vec<FileNode>> {
        self.repo.list_trash(owner).await
    }
}

#[async_trait]
impl VfsService for VfsServiceImpl {
    async fn create_directory(
        &self,
        parent_id: &FileId,
        name: &str,
        owner: &UserId,
    ) -> AppResult<FileNode> {
        self.repo.create_directory(parent_id, name, *owner).await
    }

    async fn get_node(&self, id: &FileId) -> AppResult<FileNode> {
        self.repo.get(id).await
    }

    async fn list_children(
        &self,
        parent_id: &FileId,
        page: &PageRequest,
    ) -> AppResult<PageResponse<FileNode>> {
        self.repo.list_children(parent_id, page).await
    }

    async fn rename(&self, id: &FileId, new_name: &str) -> AppResult<FileNode> {
        self.repo.rename(id, new_name).await
    }

    async fn move_node(&self, id: &FileId, new_parent: &FileId) -> AppResult<FileNode> {
        self.repo.move_node(id, new_parent).await
    }

    async fn soft_delete(&self, id: &FileId) -> AppResult<()> {
        self.repo.soft_delete(id).await
    }

    async fn restore(&self, id: &FileId) -> AppResult<FileNode> {
        self.repo.restore(id).await
    }

    async fn permanent_delete(&self, id: &FileId) -> AppResult<()> {
        self.repo.permanent_delete(id).await
    }

    async fn list_trash(&self, owner: &UserId) -> AppResult<Vec<FileNode>> {
        self.repo.list_trash(owner).await
    }

    async fn copy_node(
        &self,
        id: &FileId,
        new_parent: &FileId,
        owner: &UserId,
    ) -> AppResult<FileNode> {
        self.repo.copy_node(id, new_parent, *owner).await
    }

    async fn create_file_record(
        &self,
        parent_id: &FileId,
        name: &str,
        size: u64,
        content_hash: Option<String>,
        mime_type: Option<String>,
        owner: &UserId,
    ) -> AppResult<FileNode> {
        self.repo
            .create_file(parent_id, name, size, content_hash, mime_type, *owner)
            .await
    }
}
