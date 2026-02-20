//! Closure-table helpers for the virtual file system directory tree.
//!
//! # Algorithm overview
//!
//! The `file_node_paths` table stores every (ancestor, descendant, depth) triple,
//! including the self-reference (node, node, 0).  This enables:
//!
//! * Single-query subtree retrieval (`WHERE ancestor_id = ?`)
//! * Efficient cycle detection (`WHERE ancestor_id = X AND descendant_id = Y`)
//! * O(1) "is X an ancestor of Y?" check
//!
//! All functions here operate on the `file_node_paths` table and accept a
//! shared [`DatabaseConnection`] so they can participate in calling code's
//! transactions.

use sea_orm::{ActiveModelTrait, ActiveValue::Set, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};

use jiezi_cloud_core::{error::{AppError, AppResult}, types::FileId};

use crate::entities::file_node_paths;

// ─── Insertion ───────────────────────────────────────────────────────────────

/// Insert closure-table entries for a newly created node.
///
/// Creates:
/// - `(node_id, node_id, 0)` — the self-reference row.
/// - For every ancestor `a` of `parent_id` (at depth `d` in `parent_id`'s
///   ancestor chain), adds `(a, node_id, d + 1)`.
pub async fn insert_node_paths(
    conn: &DatabaseConnection,
    node_id: &FileId,
    parent_id: Option<&FileId>,
) -> AppResult<()> {
    // Self-reference.
    file_node_paths::ActiveModel {
        ancestor_id:   Set(node_id.to_string()),
        descendant_id: Set(node_id.to_string()),
        depth:         Set(0),
    }
    .insert(conn)
    .await
    .map_err(|e| AppError::Database(e.to_string()))?;

    // Inherit all ancestor paths from the parent.
    if let Some(pid) = parent_id {
        // Rows where descendant_id = parent_id give all ancestors of parent.
        let parent_ancestors = file_node_paths::Entity::find()
            .filter(file_node_paths::Column::DescendantId.eq(pid.to_string()))
            .all(conn)
            .await
            .map_err(|e| AppError::Database(e.to_string()))?;

        for anc in parent_ancestors {
            file_node_paths::ActiveModel {
                ancestor_id:   Set(anc.ancestor_id.clone()),
                descendant_id: Set(node_id.to_string()),
                depth:         Set(anc.depth + 1),
            }
            .insert(conn)
            .await
            .map_err(|e| AppError::Database(e.to_string()))?;
        }
    }

    Ok(())
}

// ─── Queries ─────────────────────────────────────────────────────────────────

/// Return the string IDs of every node in the subtree rooted at `root_id`,
/// including `root_id` itself (depth = 0 row).
pub async fn subtree_ids(
    conn: &DatabaseConnection,
    root_id: &FileId,
) -> AppResult<Vec<String>> {
    let rows = file_node_paths::Entity::find()
        .filter(file_node_paths::Column::AncestorId.eq(root_id.to_string()))
        .all(conn)
        .await
        .map_err(|e| AppError::Database(e.to_string()))?;

    Ok(rows.into_iter().map(|r| r.descendant_id).collect())
}

/// Return `true` if `potential_descendant` is within the subtree of
/// `potential_ancestor` (or equal to it).
///
/// Used for **cycle prevention** before moving a node.
pub async fn is_descendant_of(
    conn: &DatabaseConnection,
    potential_descendant: &FileId,
    potential_ancestor: &FileId,
) -> AppResult<bool> {
    use sea_orm::PaginatorTrait;

    let count = file_node_paths::Entity::find()
        .filter(
            file_node_paths::Column::AncestorId.eq(potential_ancestor.to_string())
                .and(file_node_paths::Column::DescendantId.eq(potential_descendant.to_string())),
        )
        .count(conn)
        .await
        .map_err(|e| AppError::Database(e.to_string()))?;

    Ok(count > 0)
}

// ─── Deletion ────────────────────────────────────────────────────────────────

/// Remove all closure-table rows where the descendant is in the subtree of
/// `root_id`.
///
/// Called before permanently deleting a subtree.
pub async fn delete_subtree_paths(
    conn: &DatabaseConnection,
    root_id: &FileId,
) -> AppResult<()> {
    let subtree = subtree_ids(conn, root_id).await?;
    if subtree.is_empty() {
        return Ok(());
    }

    file_node_paths::Entity::delete_many()
        .filter(file_node_paths::Column::DescendantId.is_in(subtree))
        .exec(conn)
        .await
        .map_err(|e| AppError::Database(e.to_string()))?;

    Ok(())
}

// ─── Move ────────────────────────────────────────────────────────────────────

/// Move the subtree rooted at `node_id` under `new_parent_id`.
///
/// # Algorithm
///
/// 1. Collect the string IDs of the entire subtree (`node_id` + all descendants).
/// 2. Delete all paths where **descendant ∈ subtree** AND **ancestor ∉ subtree**
///    (i.e. the external-ancestor → subtree-node links from the old location).
/// 3. For every ancestor of `new_parent_id` (including itself) and every
///    direct descendant path starting from `node_id`, insert a new path row.
///
/// Step 3 formula: for each outer row `(A, new_parent, d_outer)` and each
/// inner row `(node_id, D, d_inner)`, insert `(A, D, d_outer + d_inner + 1)`.
pub async fn move_subtree(
    conn: &DatabaseConnection,
    node_id: &FileId,
    new_parent_id: &FileId,
) -> AppResult<()> {
    let subtree = subtree_ids(conn, node_id).await?;

    // Step 2: remove links from outside the subtree into the subtree.
    file_node_paths::Entity::delete_many()
        .filter(
            file_node_paths::Column::DescendantId.is_in(subtree.clone())
                .and(file_node_paths::Column::AncestorId.is_not_in(subtree.clone())),
        )
        .exec(conn)
        .await
        .map_err(|e| AppError::Database(e.to_string()))?;

    // Step 3a: Collect all ancestor paths of the new parent (including self).
    // These are rows where descendant_id = new_parent_id.
    let outer_paths = file_node_paths::Entity::find()
        .filter(file_node_paths::Column::DescendantId.eq(new_parent_id.to_string()))
        .all(conn)
        .await
        .map_err(|e| AppError::Database(e.to_string()))?;

    // Step 3b: Collect all paths rooted at node_id (ancestor = node_id).
    // This gives (node_id, each_descendant, depth_from_node).
    let inner_paths = file_node_paths::Entity::find()
        .filter(file_node_paths::Column::AncestorId.eq(node_id.to_string()))
        .all(conn)
        .await
        .map_err(|e| AppError::Database(e.to_string()))?;

    // Step 3c: Cross-join outer × inner and insert new path rows.
    for outer in &outer_paths {
        for inner in &inner_paths {
            file_node_paths::ActiveModel {
                ancestor_id:   Set(outer.ancestor_id.clone()),
                descendant_id: Set(inner.descendant_id.clone()),
                depth:         Set(outer.depth + inner.depth + 1),
            }
            .insert(conn)
            .await
            .map_err(|e| AppError::Database(e.to_string()))?;
        }
    }

    Ok(())
}
