//! Integration tests for VFS metadata type conversions.

use jiezi_cloud_vfs::metadata::{new_directory_active_model, node_type_to_str, str_to_node_type};
use jiezi_cloud_core::{
    error::AppError,
    models::file::NodeType,
    types::{FileId, SpaceId, UserId},
};
use sea_orm::ActiveValue;

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

    assert!(matches!(am.parent_id, ActiveValue::Set(None)));
    assert!(matches!(am.node_type, ActiveValue::Set(ref s) if s == "directory"));
    assert!(matches!(am.size, ActiveValue::Set(0)));
    assert!(matches!(am.deleted_at, ActiveValue::Set(None)));
}
