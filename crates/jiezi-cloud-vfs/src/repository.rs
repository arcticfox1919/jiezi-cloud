//! SeaORM repository for file node CRUD and directory-tree queries.
//!
//! # Design
//!
//! [`FileNodeRepository`] wraps a shared [`DatabaseConnection`] and provides all
//! the database operations needed by [`crate::service::VfsServiceImpl`].
//!
//! Directory-tree relationships are maintained in the `file_node_paths` closure
//! table via the helpers in [`crate::tree`].  All tree mutations happen in the
//! same logical call (no background tasks), so callers see a consistent view.

use std::collections::VecDeque;

use chrono::Utc;
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, DatabaseConnection, EntityTrait, PaginatorTrait,
    QueryFilter, QueryOrder, QuerySelect,
};

use jiezi_cloud_core::{
    error::{AppError, AppResult},
    models::file::FileNode,
    types::{FileId, PageRequest, PageResponse, SpaceId, UserId},
};

use crate::{
    entities::file_nodes,
    metadata::{model_to_file_node, new_directory_active_model},
    tree,
};

// ─── FileNodeRepository ───────────────────────────────────────────────────────

/// Database access layer for [`FileNode`] records.
#[derive(Clone)]
pub struct FileNodeRepository {
    db: DatabaseConnection,
}

impl FileNodeRepository {
    /// Construct a repository backed by `db`.
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }

    // ── Private helpers ───────────────────────────────────────────────────────

    fn db(&self) -> &DatabaseConnection {
        &self.db
    }

    /// Fetch the raw model by ID; returns `AppError::NotFound` when absent.
    async fn require_model(&self, id: &FileId) -> AppResult<file_nodes::Model> {
        file_nodes::Entity::find_by_id(id.to_string())
            .one(self.db())
            .await
            .map_err(|e| AppError::Database(e.to_string()))?
            .ok_or_else(|| AppError::NotFound(format!("file node {id}")))
    }

    /// Return `true` if a **live** child node with `name` exists under `parent_id`.
    async fn sibling_exists(&self, parent_id_str: &str, name: &str, exclude_id: Option<&FileId>) -> AppResult<bool> {
        let mut q = file_nodes::Entity::find()
            .filter(
                file_nodes::Column::ParentId.eq(parent_id_str)
                    .and(file_nodes::Column::Name.eq(name))
                    .and(file_nodes::Column::DeletedAt.is_null()),
            );
        if let Some(ex) = exclude_id {
            q = q.filter(file_nodes::Column::Id.ne(ex.to_string()));
        }
        Ok(q.count(self.db()).await.map_err(|e| AppError::Database(e.to_string()))? > 0)
    }

    // ── Public API ────────────────────────────────────────────────────────────

    // ── 4.2 — Directory operations ────────────────────────────────────────────

    /// Create the root directory for a new space (no parent).
    pub async fn create_root(&self, space_id: SpaceId, owner_id: UserId) -> AppResult<FileNode> {
        let id = FileId::new();
        let am = new_directory_active_model(id, None, space_id, owner_id, "/");
        let m = am.insert(self.db()).await.map_err(|e| AppError::Database(e.to_string()))?;
        tree::insert_node_paths(self.db(), &id, None).await?;
        model_to_file_node(m)
    }

    /// Create a child directory under `parent_id`.
    ///
    /// # Errors
    ///
    /// - [`AppError::NotFound`] if `parent_id` does not exist.
    /// - [`AppError::Conflict`] if a live sibling named `name` already exists.
    pub async fn create_directory(
        &self,
        parent_id: &FileId,
        name: &str,
        owner_id: UserId,
    ) -> AppResult<FileNode> {
        let parent = self.require_model(parent_id).await?;

        if self.sibling_exists(&parent.id, name, None).await? {
            return Err(AppError::Conflict(format!("name '{name}' already exists")));
        }

        let id = FileId::new();
        let space_id: SpaceId = parent
            .space_id
            .parse()
            .map_err(|e: uuid::Error| AppError::Database(e.to_string()))?;
        let am = new_directory_active_model(id, Some(*parent_id), space_id, owner_id, name);
        let m = am.insert(self.db()).await.map_err(|e| AppError::Database(e.to_string()))?;
        tree::insert_node_paths(self.db(), &id, Some(parent_id)).await?;
        model_to_file_node(m)
    }

    /// List live (non-deleted) immediate children of `parent_id`, paginated.
    pub async fn list_children(
        &self,
        parent_id: &FileId,
        page: &PageRequest,
    ) -> AppResult<PageResponse<FileNode>> {
        // Verify parent exists.
        self.require_model(parent_id).await?;

        let base_filter = file_nodes::Column::ParentId.eq(parent_id.to_string())
            .and(file_nodes::Column::DeletedAt.is_null());

        let total = file_nodes::Entity::find()
            .filter(base_filter.clone())
            .count(self.db())
            .await
            .map_err(|e| AppError::Database(e.to_string()))?;

        let rows = file_nodes::Entity::find()
            .filter(base_filter)
            .order_by_asc(file_nodes::Column::Name)
            .offset(page.offset())
            .limit(page.limit())
            .all(self.db())
            .await
            .map_err(|e| AppError::Database(e.to_string()))?;

        let items: Vec<FileNode> = rows.into_iter().map(model_to_file_node).collect::<AppResult<_>>()?;

        Ok(PageResponse::new(items, total, page.page, page.per_page))
    }

    // ── 4.3 — File record operations ──────────────────────────────────────────

    /// Create a file record under `parent_id`.
    ///
    /// # Errors
    ///
    /// - [`AppError::Conflict`] if a live sibling named `name` already exists.
    pub async fn create_file(
        &self,
        parent_id: &FileId,
        name: &str,
        size: u64,
        content_hash: Option<String>,
        mime_type: Option<String>,
        owner_id: UserId,
        file_id: Option<FileId>,
    ) -> AppResult<FileNode> {
        let parent = self.require_model(parent_id).await?;

        if self.sibling_exists(&parent.id, name, None).await? {
            return Err(AppError::Conflict(format!("name '{name}' already exists")));
        }

        let now = Utc::now();
        let id = file_id.unwrap_or_else(FileId::new);
        let am = file_nodes::ActiveModel {
            id:            Set(id.to_string()),
            parent_id:     Set(Some(parent_id.to_string())),
            space_id:      Set(parent.space_id.clone()),
            owner_id:      Set(owner_id.to_string()),
            name:          Set(name.to_owned()),
            node_type:     Set("file".to_owned()),
            size:          Set(size as i64),
            mime_type:     Set(mime_type),
            content_hash:  Set(content_hash),
            metadata_json: Set("{}".to_owned()),
            created_at:    Set(now),
            updated_at:    Set(now),
            deleted_at:    Set(None),
        };
        let m = am.insert(self.db()).await.map_err(|e| AppError::Database(e.to_string()))?;
        tree::insert_node_paths(self.db(), &id, Some(parent_id)).await?;
        model_to_file_node(m)
    }

    // ── 4.2 — Single node retrieval ───────────────────────────────────────────

    /// Retrieve a single node by its ID.
    pub async fn get(&self, id: &FileId) -> AppResult<FileNode> {
        let m = self.require_model(id).await?;
        model_to_file_node(m)
    }

    // ── 4.4 — Rename and move ─────────────────────────────────────────────────

    /// Rename a node in place (parent unchanged).
    pub async fn rename(&self, id: &FileId, new_name: &str) -> AppResult<FileNode> {
        let m = self.require_model(id).await?;

        // Conflict check among siblings.
        if let Some(ref pid) = m.parent_id {
            if self.sibling_exists(pid, new_name, Some(id)).await? {
                return Err(AppError::Conflict(format!("name '{new_name}' already exists")));
            }
        }

        let mut am: file_nodes::ActiveModel = m.into();
        am.name = Set(new_name.to_owned());
        am.updated_at = Set(Utc::now());
        let updated = am.update(self.db()).await.map_err(|e| AppError::Database(e.to_string()))?;
        model_to_file_node(updated)
    }

    /// Move a node (and its subtree) under `new_parent_id`.
    ///
    /// # Errors
    ///
    /// - [`AppError::Validation`] if `new_parent_id` is a descendant of `id` (cycle).
    /// - [`AppError::Conflict`] if a live sibling with the same name exists at the target.
    pub async fn move_node(&self, id: &FileId, new_parent_id: &FileId) -> AppResult<FileNode> {
        let m = self.require_model(id).await?;
        // Verify new parent exists.
        self.require_model(new_parent_id).await?;

        // Cycle prevention: new_parent must not be within the subtree of id.
        if tree::is_descendant_of(self.db(), new_parent_id, id).await? {
            return Err(AppError::Validation(
                "cannot move a node into its own descendant".to_owned(),
            ));
        }

        // Name conflict at destination.
        if self.sibling_exists(&new_parent_id.to_string(), &m.name, None).await? {
            return Err(AppError::Conflict(format!(
                "name '{}' already exists in target directory",
                m.name
            )));
        }

        // Rewrite the closure table first.
        tree::move_subtree(self.db(), id, new_parent_id).await?;

        // Update parent_id on the node row.
        let mut am: file_nodes::ActiveModel = m.into();
        am.parent_id = Set(Some(new_parent_id.to_string()));
        am.updated_at = Set(Utc::now());
        let updated = am.update(self.db()).await.map_err(|e| AppError::Database(e.to_string()))?;
        model_to_file_node(updated)
    }

    // ── 4.5 — Soft delete & restore ───────────────────────────────────────────

    /// Soft-delete a node and every node in its subtree.
    pub async fn soft_delete(&self, id: &FileId) -> AppResult<()> {
        // Confirm the node exists before computing subtree.
        self.require_model(id).await?;

        let subtree = tree::subtree_ids(self.db(), id).await?;
        let now = Utc::now();

        for node_id_str in subtree {
            let row = file_nodes::Entity::find_by_id(node_id_str)
                .one(self.db())
                .await
                .map_err(|e| AppError::Database(e.to_string()))?;
            if let Some(row) = row {
                let mut am: file_nodes::ActiveModel = row.into();
                am.deleted_at = Set(Some(now));
                am.updated_at = Set(now);
                am.update(self.db()).await.map_err(|e| AppError::Database(e.to_string()))?;
            }
        }

        Ok(())
    }

    /// Restore a soft-deleted node and all its descendants.
    ///
    /// # Errors
    ///
    /// - [`AppError::NotFound`] if the node does not exist or is not deleted.
    pub async fn restore(&self, id: &FileId) -> AppResult<FileNode> {
        let m = file_nodes::Entity::find_by_id(id.to_string())
            .one(self.db())
            .await
            .map_err(|e| AppError::Database(e.to_string()))?
            .ok_or_else(|| AppError::NotFound(format!("file node {id}")))?;

        if m.deleted_at.is_none() {
            return Err(AppError::NotFound(format!("file node {id} is not in the trash")));
        }

        let subtree = tree::subtree_ids(self.db(), id).await?;
        let now = Utc::now();

        for node_id_str in subtree {
            let row = file_nodes::Entity::find_by_id(node_id_str)
                .one(self.db())
                .await
                .map_err(|e| AppError::Database(e.to_string()))?;
            if let Some(row) = row {
                let mut am: file_nodes::ActiveModel = row.into();
                am.deleted_at = Set(None);
                am.updated_at = Set(now);
                am.update(self.db()).await.map_err(|e| AppError::Database(e.to_string()))?;
            }
        }

        self.get(id).await
    }

    /// Permanently delete a node and its subtree from the database.
    ///
    /// This is **irreversible**.  The caller must ensure storage chunks are
    /// garbage-collected separately.
    pub async fn permanent_delete(&self, id: &FileId) -> AppResult<()> {
        let subtree = tree::subtree_ids(self.db(), id).await?;
        if subtree.is_empty() {
            self.require_model(id).await?; // surface NotFound
        }

        // Remove closure-table entries first (no FK cascade needed).
        tree::delete_subtree_paths(self.db(), id).await?;

        // Remove the node rows.
        file_nodes::Entity::delete_many()
            .filter(file_nodes::Column::Id.is_in(subtree))
            .exec(self.db())
            .await
            .map_err(|e| AppError::Database(e.to_string()))?;

        Ok(())
    }

    /// List trashed (soft-deleted) nodes owned by `owner_id`.
    pub async fn list_trash(&self, owner_id: &UserId) -> AppResult<Vec<FileNode>> {
        let rows = file_nodes::Entity::find()
            .filter(
                file_nodes::Column::OwnerId.eq(owner_id.to_string())
                    .and(file_nodes::Column::DeletedAt.is_not_null()),
            )
            .all(self.db())
            .await
            .map_err(|e| AppError::Database(e.to_string()))?;

        rows.into_iter().map(model_to_file_node).collect()
    }

    // ── 4.4 — Copy ────────────────────────────────────────────────────────────

    /// Copy a node (and its subtree for directories) under `new_parent_id`.
    ///
    /// Uses an iterative BFS queue to avoid async recursion.  New node IDs are
    /// generated for every copied node; chunk content is referenced, not duplicated
    /// (the same `content_hash` points to the shared storage object).
    pub async fn copy_node(
        &self,
        src_id: &FileId,
        new_parent_id: &FileId,
        new_owner: UserId,
    ) -> AppResult<FileNode> {
        let src = self.require_model(src_id).await?;
        let dst_parent = self.require_model(new_parent_id).await?;

        // Conflict check at destination.
        if self.sibling_exists(&dst_parent.id, &src.name, None).await? {
            return Err(AppError::Conflict(format!(
                "name '{}' already exists in target directory",
                src.name
            )));
        }

        // BFS work queue: (source_node_id, destination_parent_id)
        let mut queue: VecDeque<(FileId, FileId)> = VecDeque::new();
        queue.push_back((*src_id, *new_parent_id));

        let mut root_node: Option<FileNode> = None;
        let now = Utc::now();

        while let Some((src_node_id, dst_parent_id)) = queue.pop_front() {
            let src_m = file_nodes::Entity::find_by_id(src_node_id.to_string())
                .one(self.db())
                .await
                .map_err(|e| AppError::Database(e.to_string()))?
                .ok_or_else(|| AppError::NotFound(format!("source node {src_node_id}")))?;

            let dst_parent_m = file_nodes::Entity::find_by_id(dst_parent_id.to_string())
                .one(self.db())
                .await
                .map_err(|e| AppError::Database(e.to_string()))?
                .ok_or_else(|| AppError::NotFound(format!("dest parent {dst_parent_id}")))?;

            let new_id = FileId::new();
            let am = file_nodes::ActiveModel {
                id:            Set(new_id.to_string()),
                parent_id:     Set(Some(dst_parent_id.to_string())),
                space_id:      Set(dst_parent_m.space_id.clone()),
                owner_id:      Set(new_owner.to_string()),
                name:          Set(src_m.name.clone()),
                node_type:     Set(src_m.node_type.clone()),
                size:          Set(src_m.size),
                mime_type:     Set(src_m.mime_type.clone()),
                content_hash:  Set(src_m.content_hash.clone()),
                metadata_json: Set(src_m.metadata_json.clone()),
                created_at:    Set(now),
                updated_at:    Set(now),
                deleted_at:    Set(None),
            };
            let new_m = am.insert(self.db()).await.map_err(|e| AppError::Database(e.to_string()))?;
            tree::insert_node_paths(self.db(), &new_id, Some(&dst_parent_id)).await?;

            let new_node = model_to_file_node(new_m)?;
            if root_node.is_none() {
                root_node = Some(new_node.clone());
            }

            // If this is a directory, enqueue its live children.
            if src_m.node_type == "directory" {
                let children = file_nodes::Entity::find()
                    .filter(
                        file_nodes::Column::ParentId.eq(src_node_id.to_string())
                            .and(file_nodes::Column::DeletedAt.is_null()),
                    )
                    .all(self.db())
                    .await
                    .map_err(|e| AppError::Database(e.to_string()))?;

                for child in children {
                    let child_id: FileId = child
                        .id
                        .parse()
                        .map_err(|e: uuid::Error| AppError::Database(e.to_string()))?;
                    queue.push_back((child_id, new_id));
                }
            }
        }

        root_node.ok_or_else(|| AppError::Internal("copy produced no root node".to_owned()))
    }

    /// Create a personal root directory for `owner_id` (no space, no parent).
    ///
    /// A synthetic [`SpaceId`] is derived from the owner's ID so that every
    /// user has a unique, stable space UUID without requiring a separate
    /// `spaces` lookup.
    pub async fn create_root_for_user(&self, owner_id: UserId) -> AppResult<FileNode> {
        // Derive a stable space_id from the user_id (deterministic UUID v5-style).
        let space_id: SpaceId = owner_id
            .to_string()
            .parse()
            .map_err(|e: uuid::Error| AppError::Database(e.to_string()))?;
        let id = FileId::new();
        let am = new_directory_active_model(id, None, space_id, owner_id, "/");
        let m = am.insert(self.db()).await.map_err(|e| AppError::Database(e.to_string()))?;
        tree::insert_node_paths(self.db(), &id, None).await?;
        model_to_file_node(m)
    }

    /// Return all root nodes (nodes with `parent_id = NULL`) owned by `owner_id`.
    pub async fn list_roots(&self, owner_id: &UserId) -> AppResult<Vec<FileNode>> {
        let rows = file_nodes::Entity::find()
            .filter(
                file_nodes::Column::OwnerId
                    .eq(owner_id.to_string())
                    .and(file_nodes::Column::ParentId.is_null())
                    .and(file_nodes::Column::DeletedAt.is_null()),
            )
            .all(self.db())
            .await
            .map_err(|e| AppError::Database(e.to_string()))?;

        rows.into_iter().map(model_to_file_node).collect()
    }

    /// Return the first non-deleted file node with the given `content_hash`,
    /// or `None` if no such node exists.
    ///
    /// Used by the upload pipeline's deduplication fast-path.
    pub async fn find_by_content_hash(
        &self,
        content_hash: &str,
    ) -> AppResult<Option<FileNode>> {
        let maybe = file_nodes::Entity::find()
            .filter(
                file_nodes::Column::ContentHash
                    .eq(content_hash)
                    .and(file_nodes::Column::DeletedAt.is_null()),
            )
            .one(self.db())
            .await
            .map_err(|e| AppError::Database(e.to_string()))?;

        maybe.map(model_to_file_node).transpose()
    }
}

#[cfg(test)]
#[path = "repository_tests.rs"]
mod tests;