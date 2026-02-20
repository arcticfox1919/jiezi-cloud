//! Conversion helpers between SeaORM persistence models and domain types.
//!
//! Only the repository layer should use these helpers directly; all other
//! code works with `jiezi_cloud_core` domain types.

use chrono::Utc;
use sea_orm::ActiveValue::Set;

use jiezi_cloud_core::{
    error::{AppError, AppResult},
    models::file::{FileMetadata, FileNode, NodeType},
    types::{FileId, SpaceId, UserId},
};

use crate::entities::file_nodes;

// ─── NodeType ────────────────────────────────────────────────────────────────

/// Encode a [`NodeType`] as the stored string discriminant.
pub fn node_type_to_str(nt: NodeType) -> &'static str {
    match nt {
        NodeType::File => "file",
        NodeType::Directory => "directory",
        NodeType::Symlink => "symlink",
    }
}

/// Decode a stored string discriminant back into a [`NodeType`].
pub fn str_to_node_type(s: &str) -> AppResult<NodeType> {
    match s {
        "file" => Ok(NodeType::File),
        "directory" => Ok(NodeType::Directory),
        "symlink" => Ok(NodeType::Symlink),
        other => Err(AppError::Internal(format!("unknown node_type discriminant: '{other}'"))),
    }
}

// ─── Model → Domain ───────────────────────────────────────────────────────────

/// Convert a raw database row into the domain [`FileNode`].
pub fn model_to_file_node(m: file_nodes::Model) -> AppResult<FileNode> {
    let parse_file_id = |s: &str, field: &str| -> AppResult<FileId> {
        s.parse().map_err(|e| AppError::Database(format!("{field}: {e}")))
    };

    let metadata: FileMetadata = serde_json::from_str(&m.metadata_json)
        .map_err(|e| AppError::Serialization(e.to_string()))?;

    Ok(FileNode {
        id: parse_file_id(&m.id, "id")?,
        parent_id: m
            .parent_id
            .as_deref()
            .map(|s| s.parse::<FileId>())
            .transpose()
            .map_err(|e| AppError::Database(format!("parent_id: {e}")))?,
        space_id: m
            .space_id
            .parse::<SpaceId>()
            .map_err(|e| AppError::Database(format!("space_id: {e}")))?,
        owner_id: m
            .owner_id
            .parse::<UserId>()
            .map_err(|e| AppError::Database(format!("owner_id: {e}")))?,
        name: m.name,
        node_type: str_to_node_type(&m.node_type)?,
        size: m.size as u64,
        mime_type: m.mime_type,
        content_hash: m.content_hash,
        created_at: m.created_at,
        updated_at: m.updated_at,
        deleted_at: m.deleted_at,
        metadata,
    })
}

// ─── Active model builders ────────────────────────────────────────────────────

/// Construct a [`file_nodes::ActiveModel`] for a new directory node.
pub fn new_directory_active_model(
    id: FileId,
    parent_id: Option<FileId>,
    space_id: SpaceId,
    owner_id: UserId,
    name: &str,
) -> file_nodes::ActiveModel {
    let now = Utc::now();
    file_nodes::ActiveModel {
        id: Set(id.to_string()),
        parent_id: Set(parent_id.map(|p| p.to_string())),
        space_id: Set(space_id.to_string()),
        owner_id: Set(owner_id.to_string()),
        name: Set(name.to_owned()),
        node_type: Set("directory".to_owned()),
        size: Set(0),
        mime_type: Set(None),
        content_hash: Set(None),
        metadata_json: Set("{}".to_owned()),
        created_at: Set(now),
        updated_at: Set(now),
        deleted_at: Set(None),
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_node_type_round_trip() {
        for nt in [NodeType::File, NodeType::Directory, NodeType::Symlink] {
            let s = node_type_to_str(nt);
            assert_eq!(str_to_node_type(s).unwrap(), nt);
        }
    }

    #[test]
    fn test_unknown_node_type_returns_error() {
        let err = str_to_node_type("unknown").unwrap_err();
        assert!(matches!(err, AppError::Internal(_)));
    }

    #[test]
    fn test_new_directory_active_model_fields() {
        let id = FileId::new();
        let space_id = SpaceId::new();
        let owner_id = UserId::new();
        let am = new_directory_active_model(id, None, space_id, owner_id, "docs");

        use sea_orm::ActiveValue;
        assert!(matches!(am.parent_id, ActiveValue::Set(None)));
        assert!(matches!(am.node_type, ActiveValue::Set(ref s) if s == "directory"));
        assert!(matches!(am.size, ActiveValue::Set(0)));
        assert!(matches!(am.deleted_at, ActiveValue::Set(None)));
    }
}
