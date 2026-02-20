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
    ) -> AppResult<FileNode> {
        let parent = self.require_model(parent_id).await?;

        if self.sibling_exists(&parent.id, name, None).await? {
            return Err(AppError::Conflict(format!("name '{name}' already exists")));
        }

        let now = Utc::now();
        let id = FileId::new();
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
}

// ─── Test helpers ─────────────────────────────────────────────────────────────

#[cfg(test)]
pub mod test_helpers {
    use sea_orm::{ConnectionTrait, Database, DatabaseConnection, Schema, Statement};

    /// Create an in-memory SQLite database pre-populated with the VFS schema.
    ///
    /// Uses [`Schema::create_table_from_entity`] so there is no dependency on
    /// the migration crate — schema changes in entities are picked up
    /// automatically.
    ///
    /// Foreign-key enforcement is disabled for test databases so tests can use
    /// synthetic IDs without needing to pre-populate the `spaces` table.
    pub async fn create_test_db() -> DatabaseConnection {
        let db = Database::connect("sqlite::memory:")
            .await
            .expect("in-memory SQLite pool");

        // Disable FK checks so tests can use synthetic space/owner IDs without
        // pre-populating the `spaces` table.
        db.execute(Statement::from_string(
            db.get_database_backend(),
            "PRAGMA foreign_keys = OFF".to_owned(),
        ))
        .await
        .expect("disable foreign keys");

        let backend = db.get_database_backend();
        let schema = Schema::new(backend);

        db.execute(
            backend.build(&schema.create_table_from_entity(crate::entities::spaces::Entity)),
        )
        .await
        .expect("create spaces table");

        db.execute(
            backend.build(&schema.create_table_from_entity(crate::entities::file_nodes::Entity)),
        )
        .await
        .expect("create file_nodes table");

        db.execute(
            backend.build(
                &schema.create_table_from_entity(crate::entities::file_node_paths::Entity),
            ),
        )
        .await
        .expect("create file_node_paths table");

        db
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::{test_helpers::create_test_db, *};
    use jiezi_cloud_core::models::file::NodeType;

    // ── Helpers ───────────────────────────────────────────────────────────────

    async fn make_repo() -> FileNodeRepository {
        FileNodeRepository::new(create_test_db().await)
    }

    async fn make_root(repo: &FileNodeRepository) -> FileNode {
        let space_id = SpaceId::new();
        let owner_id = UserId::new();
        repo.create_root(space_id, owner_id).await.unwrap()
    }

    // ── TDD 4.2 — Directory operations ───────────────────────────────────────

    // TDD 4.2-1: create a root directory
    #[tokio::test]
    async fn test_create_root_directory() {
        let repo = make_repo().await;
        let root = make_root(&repo).await;

        assert_eq!(root.name, "/");
        assert!(root.is_directory());
        assert!(root.parent_id.is_none());
        assert!(!root.is_deleted());
    }

    // TDD 4.2-2: create a child directory
    #[tokio::test]
    async fn test_create_child_directory() {
        let repo = make_repo().await;
        let root = make_root(&repo).await;
        let owner = root.owner_id;

        let docs = repo.create_directory(&root.id, "documents", owner).await.unwrap();

        assert_eq!(docs.name, "documents");
        assert!(docs.is_directory());
        assert_eq!(docs.parent_id, Some(root.id));
        assert_eq!(docs.space_id, root.space_id);
    }

    // TDD 4.2-3: create nested directories three levels deep
    #[tokio::test]
    async fn test_nested_directories() {
        let repo = make_repo().await;
        let root = make_root(&repo).await;
        let owner = root.owner_id;

        let a = repo.create_directory(&root.id, "a", owner).await.unwrap();
        let b = repo.create_directory(&a.id, "b", owner).await.unwrap();
        let c = repo.create_directory(&b.id, "c", owner).await.unwrap();

        assert_eq!(c.parent_id, Some(b.id));
        assert_eq!(b.parent_id, Some(a.id));
        assert_eq!(a.parent_id, Some(root.id));
    }

    // TDD 4.2-4: list_children returns only immediate children, not deeper nodes
    #[tokio::test]
    async fn test_list_children_immediate_only() {
        let repo = make_repo().await;
        let root = make_root(&repo).await;
        let owner = root.owner_id;

        let page = PageRequest::new(1, 20);
        repo.create_directory(&root.id, "a", owner).await.unwrap();
        repo.create_directory(&root.id, "b", owner).await.unwrap();
        let a = repo.create_directory(&root.id, "c", owner).await.unwrap();
        // Child of a — should NOT appear when listing root.
        repo.create_directory(&a.id, "deep", owner).await.unwrap();

        let resp = repo.list_children(&root.id, &page).await.unwrap();
        assert_eq!(resp.total, 3);
        assert_eq!(resp.items.len(), 3);
    }

    // TDD 4.2-5: list_children is sorted by name
    #[tokio::test]
    async fn test_list_children_sorted() {
        let repo = make_repo().await;
        let root = make_root(&repo).await;
        let owner = root.owner_id;
        let page = PageRequest::new(1, 20);

        repo.create_directory(&root.id, "zebra", owner).await.unwrap();
        repo.create_directory(&root.id, "alpha", owner).await.unwrap();
        repo.create_directory(&root.id, "middle", owner).await.unwrap();

        let resp = repo.list_children(&root.id, &page).await.unwrap();
        let names: Vec<_> = resp.items.iter().map(|n| n.name.as_str()).collect();
        assert_eq!(names, ["alpha", "middle", "zebra"]);
    }

    // TDD 4.2-6: list_children paginates correctly
    #[tokio::test]
    async fn test_list_children_pagination() {
        let repo = make_repo().await;
        let root = make_root(&repo).await;
        let owner = root.owner_id;

        for i in 0..5u32 {
            repo.create_directory(&root.id, &format!("dir{i:02}"), owner).await.unwrap();
        }

        let page1 = repo.list_children(&root.id, &PageRequest::new(1, 3)).await.unwrap();
        let page2 = repo.list_children(&root.id, &PageRequest::new(2, 3)).await.unwrap();

        assert_eq!(page1.total, 5);
        assert_eq!(page1.items.len(), 3);
        assert_eq!(page2.items.len(), 2);
    }

    // TDD 4.2-7: same-name directory conflict returns Conflict error
    #[tokio::test]
    async fn test_create_directory_name_conflict() {
        let repo = make_repo().await;
        let root = make_root(&repo).await;
        let owner = root.owner_id;

        repo.create_directory(&root.id, "docs", owner).await.unwrap();
        let err = repo.create_directory(&root.id, "docs", owner).await.unwrap_err();
        assert!(matches!(err, AppError::Conflict(_)), "expected Conflict, got {err:?}");
    }

    // TDD 4.2-8: get_node returns the correct node
    #[tokio::test]
    async fn test_get_node() {
        let repo = make_repo().await;
        let root = make_root(&repo).await;
        let owner = root.owner_id;

        let dir = repo.create_directory(&root.id, "photos", owner).await.unwrap();
        let fetched = repo.get(&dir.id).await.unwrap();

        assert_eq!(fetched.id, dir.id);
        assert_eq!(fetched.name, "photos");
    }

    // TDD 4.2-9: get_node returns NotFound for unknown ID
    #[tokio::test]
    async fn test_get_nonexistent_node() {
        let repo = make_repo().await;
        let err = repo.get(&FileId::new()).await.unwrap_err();
        assert!(matches!(err, AppError::NotFound(_)));
    }

    // TDD 4.3 — File record operations ────────────────────────────────────────

    // TDD 4.3-1: create a file record and verify fields
    #[tokio::test]
    async fn test_create_file_record() {
        let repo = make_repo().await;
        let root = make_root(&repo).await;
        let owner = root.owner_id;

        let file = repo
            .create_file(&root.id, "photo.jpg", 1024, Some("abc123".to_owned()), Some("image/jpeg".to_owned()), owner)
            .await
            .unwrap();

        assert_eq!(file.name, "photo.jpg");
        assert!(matches!(file.node_type, NodeType::File));
        assert_eq!(file.size, 1024);
        assert_eq!(file.content_hash.as_deref(), Some("abc123"));
        assert_eq!(file.mime_type.as_deref(), Some("image/jpeg"));
    }

    // TDD 4.3-2: duplicate file name returns Conflict
    #[tokio::test]
    async fn test_create_file_name_conflict() {
        let repo = make_repo().await;
        let root = make_root(&repo).await;
        let owner = root.owner_id;

        repo.create_file(&root.id, "report.pdf", 512, None, None, owner).await.unwrap();
        let err = repo.create_file(&root.id, "report.pdf", 512, None, None, owner).await.unwrap_err();
        assert!(matches!(err, AppError::Conflict(_)));
    }

    // ── TDD 4.4 — Rename and move ─────────────────────────────────────────────

    // TDD 4.4-1: rename a file
    #[tokio::test]
    async fn test_rename_file() {
        let repo = make_repo().await;
        let root = make_root(&repo).await;
        let owner = root.owner_id;

        let f = repo.create_file(&root.id, "old.txt", 0, None, None, owner).await.unwrap();
        let renamed = repo.rename(&f.id, "new.txt").await.unwrap();

        assert_eq!(renamed.name, "new.txt");
        assert_eq!(renamed.id, f.id);
    }

    // TDD 4.4-2: rename conflict returns Conflict
    #[tokio::test]
    async fn test_rename_conflict() {
        let repo = make_repo().await;
        let root = make_root(&repo).await;
        let owner = root.owner_id;

        let _f1 = repo.create_file(&root.id, "a.txt", 0, None, None, owner).await.unwrap();
        let f2 = repo.create_file(&root.id, "b.txt", 0, None, None, owner).await.unwrap();

        let err = repo.rename(&f2.id, "a.txt").await.unwrap_err();
        assert!(matches!(err, AppError::Conflict(_)));
    }

    // TDD 4.4-3: move a file to another directory
    #[tokio::test]
    async fn test_move_file() {
        let repo = make_repo().await;
        let root = make_root(&repo).await;
        let owner = root.owner_id;

        let dir_a = repo.create_directory(&root.id, "a", owner).await.unwrap();
        let dir_b = repo.create_directory(&root.id, "b", owner).await.unwrap();
        let f = repo.create_file(&dir_a.id, "file.txt", 0, None, None, owner).await.unwrap();

        let moved = repo.move_node(&f.id, &dir_b.id).await.unwrap();

        assert_eq!(moved.parent_id, Some(dir_b.id));

        // File should now appear in b's children.
        let children_b = repo.list_children(&dir_b.id, &PageRequest::new(1, 20)).await.unwrap();
        assert_eq!(children_b.items.len(), 1);

        // File should no longer appear in a's children.
        let children_a = repo.list_children(&dir_a.id, &PageRequest::new(1, 20)).await.unwrap();
        assert_eq!(children_a.items.len(), 0);
    }

    // TDD 4.4-4: move into own descendant returns Validation error (cycle)
    #[tokio::test]
    async fn test_move_cycle_prevention() {
        let repo = make_repo().await;
        let root = make_root(&repo).await;
        let owner = root.owner_id;

        let parent = repo.create_directory(&root.id, "parent", owner).await.unwrap();
        let child = repo.create_directory(&parent.id, "child", owner).await.unwrap();

        let err = repo.move_node(&parent.id, &child.id).await.unwrap_err();
        assert!(matches!(err, AppError::Validation(_)), "expected Validation, got {err:?}");
    }

    // TDD 4.4-5: copy a file
    #[tokio::test]
    async fn test_copy_file() {
        let repo = make_repo().await;
        let root = make_root(&repo).await;
        let owner = root.owner_id;

        let dir_a = repo.create_directory(&root.id, "src", owner).await.unwrap();
        let dir_b = repo.create_directory(&root.id, "dst", owner).await.unwrap();
        let f = repo.create_file(&dir_a.id, "doc.txt", 256, Some("hash1".to_owned()), None, owner).await.unwrap();

        let copied = repo.copy_node(&f.id, &dir_b.id, owner).await.unwrap();

        // Different ID but same data.
        assert_ne!(copied.id, f.id);
        assert_eq!(copied.name, f.name);
        assert_eq!(copied.content_hash, f.content_hash);
        assert_eq!(copied.size, f.size);
        // Both directories should have one child each.
        let a_children = repo.list_children(&dir_a.id, &PageRequest::new(1, 20)).await.unwrap();
        let b_children = repo.list_children(&dir_b.id, &PageRequest::new(1, 20)).await.unwrap();
        assert_eq!(a_children.items.len(), 1);
        assert_eq!(b_children.items.len(), 1);
    }

    // ── TDD 4.5 — Soft delete and trash ───────────────────────────────────────

    // TDD 4.5-1: deleted node does not appear in live children
    #[tokio::test]
    async fn test_deleted_node_excluded_from_children() {
        let repo = make_repo().await;
        let root = make_root(&repo).await;
        let owner = root.owner_id;

        let f = repo.create_file(&root.id, "data.bin", 0, None, None, owner).await.unwrap();
        let page = PageRequest::new(1, 20);

        assert_eq!(repo.list_children(&root.id, &page).await.unwrap().total, 1);

        repo.soft_delete(&f.id).await.unwrap();

        assert_eq!(repo.list_children(&root.id, &page).await.unwrap().total, 0);
    }

    // TDD 4.5-2: trash list contains the deleted node
    #[tokio::test]
    async fn test_trash_contains_deleted_node() {
        let repo = make_repo().await;
        let root = make_root(&repo).await;
        let owner = root.owner_id;

        let f = repo.create_file(&root.id, "trash_me.txt", 0, None, None, owner).await.unwrap();
        repo.soft_delete(&f.id).await.unwrap();

        let trash = repo.list_trash(&owner).await.unwrap();
        assert_eq!(trash.len(), 1);
        assert_eq!(trash[0].id, f.id);
    }

    // TDD 4.5-3: deleting a directory also marks its descendants as deleted
    #[tokio::test]
    async fn test_delete_directory_cascades_to_children() {
        let repo = make_repo().await;
        let root = make_root(&repo).await;
        let owner = root.owner_id;

        let dir = repo.create_directory(&root.id, "folder", owner).await.unwrap();
        let child = repo.create_file(&dir.id, "file.txt", 0, None, None, owner).await.unwrap();

        repo.soft_delete(&dir.id).await.unwrap();

        // Both folder and its child should be marked deleted and appear in trash.
        let trash = repo.list_trash(&owner).await.unwrap();
        let trash_ids: Vec<_> = trash.iter().map(|n| n.id).collect();
        assert!(trash_ids.contains(&dir.id), "folder should be in trash");
        assert!(trash_ids.contains(&child.id), "child file should be in trash");

        // Deleted nodes should not appear in live child listing.
        let page = PageRequest::new(1, 20);
        let live_root_children = repo.list_children(&root.id, &page).await.unwrap();
        assert!(!live_root_children.items.iter().any(|n| n.id == dir.id));
    }

    // TDD 4.5-4: restore brings the node back to live children
    #[tokio::test]
    async fn test_restore_node() {
        let repo = make_repo().await;
        let root = make_root(&repo).await;
        let owner = root.owner_id;
        let page = PageRequest::new(1, 20);

        let f = repo.create_file(&root.id, "restore_me.txt", 0, None, None, owner).await.unwrap();
        repo.soft_delete(&f.id).await.unwrap();

        assert_eq!(repo.list_children(&root.id, &page).await.unwrap().total, 0);

        repo.restore(&f.id).await.unwrap();

        assert_eq!(repo.list_children(&root.id, &page).await.unwrap().total, 1);
    }

    // TDD 4.5-5: restore on a non-deleted node returns NotFound
    #[tokio::test]
    async fn test_restore_non_deleted_returns_not_found() {
        let repo = make_repo().await;
        let root = make_root(&repo).await;
        let owner = root.owner_id;

        let f = repo.create_file(&root.id, "live.txt", 0, None, None, owner).await.unwrap();
        let err = repo.restore(&f.id).await.unwrap_err();
        assert!(matches!(err, AppError::NotFound(_)));
    }

    // TDD 4.5-6: permanent_delete removes the node entirely
    #[tokio::test]
    async fn test_permanent_delete() {
        let repo = make_repo().await;
        let root = make_root(&repo).await;
        let owner = root.owner_id;

        let f = repo.create_file(&root.id, "gone.txt", 0, None, None, owner).await.unwrap();
        repo.soft_delete(&f.id).await.unwrap();
        repo.permanent_delete(&f.id).await.unwrap();

        let trash = repo.list_trash(&owner).await.unwrap();
        assert!(trash.is_empty());
    }

    // TDD 4.5-7: closure table reflects correct depth after nesting
    #[tokio::test]
    async fn test_closure_table_depths() {
        use crate::entities::file_node_paths;
        use sea_orm::ColumnTrait;

        let db = create_test_db().await;
        let repo = FileNodeRepository::new(db.clone());

        let root = make_root(&repo).await;
        let owner = root.owner_id;
        let child = repo.create_directory(&root.id, "child", owner).await.unwrap();
        let grandchild = repo.create_directory(&child.id, "grandchild", owner).await.unwrap();

        // Root should be ancestor of grandchild at depth 2.
        let path = file_node_paths::Entity::find()
            .filter(
                file_node_paths::Column::AncestorId.eq(root.id.to_string())
                    .and(file_node_paths::Column::DescendantId.eq(grandchild.id.to_string())),
            )
            .one(&db)
            .await
            .unwrap()
            .expect("path row should exist");

        assert_eq!(path.depth, 2);
    }
}
