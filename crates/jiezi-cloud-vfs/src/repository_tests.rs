// Tests for FileNodeRepository.
//
// Declared from repository.rs as:
//     #[cfg(test)]
//     #[path = "repository_tests.rs"]
//     mod tests;
//
// `use super::*` brings all crate-visible items from repository.rs into scope.

// ─── Test helpers ─────────────────────────────────────────────────────────────

pub mod test_helpers {
    use sea_orm::{ConnectionTrait, Database, DatabaseConnection, Schema, Statement};

    /// Create an in-memory SQLite database pre-populated with the VFS schema.
    ///
    /// Uses [`Schema::create_table_from_entity`] so there is no dependency on
    /// the migration crate �?schema changes in entities are picked up
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

use super::*;
use test_helpers::create_test_db;
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

// ── TDD 4.2 �?Directory operations ───────────────────────────────────────

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
    // Child of a �?should NOT appear when listing root.
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

// TDD 4.3 �?File record operations ────────────────────────────────────────

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

    repo.create_file(&root.id, "report.pdf", 512, None, None, owner, None).await.unwrap();
    let err = repo.create_file(&root.id, "report.pdf", 512, None, None, owner, None).await.unwrap_err();
    assert!(matches!(err, AppError::Conflict(_)));
}

// ── TDD 4.4 �?Rename and move ─────────────────────────────────────────────

// TDD 4.4-1: rename a file
#[tokio::test]
async fn test_rename_file() {
    let repo = make_repo().await;
    let root = make_root(&repo).await;
    let owner = root.owner_id;

    let f = repo.create_file(&root.id, "old.txt", 0, None, None, owner, None).await.unwrap();
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

    let _f1 = repo.create_file(&root.id, "a.txt", 0, None, None, owner, None).await.unwrap();
    let f2 = repo.create_file(&root.id, "b.txt", 0, None, None, owner, None).await.unwrap();

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
    let f = repo.create_file(&dir_a.id, "file.txt", 0, None, None, owner, None).await.unwrap();

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

// ── TDD 4.5 �?Soft delete and trash ───────────────────────────────────────

// TDD 4.5-1: deleted node does not appear in live children
#[tokio::test]
async fn test_deleted_node_excluded_from_children() {
    let repo = make_repo().await;
    let root = make_root(&repo).await;
    let owner = root.owner_id;

    let f = repo.create_file(&root.id, "data.bin", 0, None, None, owner, None).await.unwrap();
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

    let f = repo.create_file(&root.id, "trash_me.txt", 0, None, None, owner, None).await.unwrap();
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
    let child = repo.create_file(&dir.id, "file.txt", 0, None, None, owner, None).await.unwrap();

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

    let f = repo.create_file(&root.id, "restore_me.txt", 0, None, None, owner, None).await.unwrap();
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

    let f = repo.create_file(&root.id, "live.txt", 0, None, None, owner, None).await.unwrap();
    let err = repo.restore(&f.id).await.unwrap_err();
    assert!(matches!(err, AppError::NotFound(_)));
}

// TDD 4.5-6: permanent_delete removes the node entirely
#[tokio::test]
async fn test_permanent_delete() {
    let repo = make_repo().await;
    let root = make_root(&repo).await;
    let owner = root.owner_id;

    let f = repo.create_file(&root.id, "gone.txt", 0, None, None, owner, None).await.unwrap();
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
