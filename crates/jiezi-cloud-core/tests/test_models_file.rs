use jiezi_cloud_core::models::file::{FileMetadata, FileNode, FileNodeBuilder, NodeType};
use jiezi_cloud_core::types::{SpaceId, UserId};

fn make_file_node() -> FileNode {
    FileNodeBuilder::new()
        .space_id(SpaceId::new())
        .owner_id(UserId::new())
        .name("report.pdf")
        .node_type(NodeType::File)
        .size(1_024)
        .mime_type("application/pdf")
        .build()
}

#[test]
fn test_builder_creates_valid_node() {
    let n = make_file_node();
    assert_eq!(n.name, "report.pdf");
    assert_eq!(n.size, 1_024);
    assert_eq!(n.node_type, NodeType::File);
    assert!(n.deleted_at.is_none());
}

#[test]
fn test_builder_directory_flags() {
    let n = FileNodeBuilder::new()
        .space_id(SpaceId::new())
        .owner_id(UserId::new())
        .name("archive")
        .node_type(NodeType::Directory)
        .build();

    assert!(n.is_directory());
    assert!(!n.is_deleted());
}

#[test]
fn test_builder_generates_id_when_not_provided() {
    let a = make_file_node();
    let b = make_file_node();
    assert_ne!(a.id, b.id, "auto-generated IDs must be unique");
}

#[test]
fn test_file_node_serde_round_trip() {
    let node = make_file_node();
    let json = serde_json::to_string(&node).expect("serialize");
    let back: FileNode = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(node.id, back.id);
    assert_eq!(node.name, back.name);
    assert_eq!(node.size, back.size);
}

#[test]
fn test_file_metadata_defaults() {
    let meta = FileMetadata::default();
    assert!(meta.tags.is_empty());
    assert!(meta.extra.is_empty());
    assert!(!meta.is_indexed);
    assert!(!meta.has_thumbnail);
}

#[test]
fn test_node_type_serde() {
    for nt in [NodeType::File, NodeType::Directory, NodeType::Symlink] {
        let json = serde_json::to_string(&nt).expect("serialize node type");
        let back: NodeType = serde_json::from_str(&json).expect("deserialize node type");
        assert_eq!(nt, back);
    }
}
