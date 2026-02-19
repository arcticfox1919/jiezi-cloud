//! File and directory node models for the virtual file system.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::types::{FileId, SpaceId, UserId};

// ─── NodeType ─────────────────────────────────────────────────────────────────

/// The kind of a virtual file system node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeType {
    /// A regular file with binary content.
    File,
    /// A container that holds other nodes.
    Directory,
    /// A symbolic link pointing to another node.
    Symlink,
}

/// MIME type (e.g. `"image/jpeg"`, `"application/pdf"`).
pub type MimeType = String;

// ─── FileNode ─────────────────────────────────────────────────────────────────

/// A node in the virtual file system — either a file or a directory.
///
/// File content is stored separately by the storage backend; this struct
/// holds only the metadata that lives in the database.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileNode {
    pub id: FileId,
    /// `None` only for the root node of a space.
    pub parent_id: Option<FileId>,
    pub space_id: SpaceId,
    pub owner_id: UserId,
    pub name: String,
    pub node_type: NodeType,
    /// File size in bytes.  Zero for directories.
    pub size: u64,
    /// MIME type detected at upload time.
    pub mime_type: Option<MimeType>,
    /// SHA-256 hex digest of the full file content.
    pub content_hash: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// Set when the node has been soft-deleted (moved to trash).
    pub deleted_at: Option<DateTime<Utc>>,
    pub metadata: FileMetadata,
}

impl FileNode {
    /// Return `true` if this node is in the trash (soft-deleted).
    pub fn is_deleted(&self) -> bool {
        self.deleted_at.is_some()
    }

    /// Return `true` if this node is a directory.
    pub fn is_directory(&self) -> bool {
        matches!(self.node_type, NodeType::Directory)
    }
}

// ─── FileMetadata ─────────────────────────────────────────────────────────────

/// Extended, optional metadata attached to a file node.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FileMetadata {
    /// User-defined tags for filtering and organisation.
    #[serde(default)]
    pub tags: Vec<String>,

    /// Arbitrary key-value pairs sourced from upload headers or EXIF data.
    #[serde(default)]
    pub extra: HashMap<String, serde_json::Value>,

    /// Whether the file content has been submitted to the search index.
    #[serde(default)]
    pub is_indexed: bool,

    /// Whether a thumbnail has been generated for this file.
    #[serde(default)]
    pub has_thumbnail: bool,
}

// ─── FileNodeBuilder ──────────────────────────────────────────────────────────

/// Fluent builder for constructing a [`FileNode`].
///
/// # Example
///
/// ```
/// # use jiezi_cloud_core::models::file::{FileNodeBuilder, NodeType};
/// # use jiezi_cloud_core::types::{SpaceId, UserId};
/// let node = FileNodeBuilder::new()
///     .space_id(SpaceId::new())
///     .owner_id(UserId::new())
///     .name("documents")
///     .node_type(NodeType::Directory)
///     .build();
/// ```
#[derive(Debug, Default)]
pub struct FileNodeBuilder {
    id: Option<FileId>,
    parent_id: Option<FileId>,
    space_id: Option<SpaceId>,
    owner_id: Option<UserId>,
    name: Option<String>,
    node_type: Option<NodeType>,
    size: u64,
    mime_type: Option<MimeType>,
    content_hash: Option<String>,
    metadata: FileMetadata,
}

impl FileNodeBuilder {
    /// Create a new builder with default values.
    pub fn new() -> Self {
        Self::default()
    }

    pub fn id(mut self, id: FileId) -> Self {
        self.id = Some(id);
        self
    }

    pub fn parent_id(mut self, parent_id: FileId) -> Self {
        self.parent_id = Some(parent_id);
        self
    }

    pub fn space_id(mut self, space_id: SpaceId) -> Self {
        self.space_id = Some(space_id);
        self
    }

    pub fn owner_id(mut self, owner_id: UserId) -> Self {
        self.owner_id = Some(owner_id);
        self
    }

    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    pub fn node_type(mut self, node_type: NodeType) -> Self {
        self.node_type = Some(node_type);
        self
    }

    pub fn size(mut self, size: u64) -> Self {
        self.size = size;
        self
    }

    pub fn mime_type(mut self, mime_type: impl Into<MimeType>) -> Self {
        self.mime_type = Some(mime_type.into());
        self
    }

    pub fn content_hash(mut self, hash: impl Into<String>) -> Self {
        self.content_hash = Some(hash.into());
        self
    }

    pub fn metadata(mut self, metadata: FileMetadata) -> Self {
        self.metadata = metadata;
        self
    }

    /// Build the [`FileNode`].
    ///
    /// # Panics
    ///
    /// Panics if `space_id`, `owner_id`, `name` or `node_type` have not been set.
    pub fn build(self) -> FileNode {
        let now = Utc::now();
        FileNode {
            id: self.id.unwrap_or_default(),
            parent_id: self.parent_id,
            space_id: self.space_id.expect("space_id is required"),
            owner_id: self.owner_id.expect("owner_id is required"),
            name: self.name.expect("name is required"),
            node_type: self.node_type.expect("node_type is required"),
            size: self.size,
            mime_type: self.mime_type,
            content_hash: self.content_hash,
            created_at: now,
            updated_at: now,
            deleted_at: None,
            metadata: self.metadata,
        }
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{SpaceId, UserId};

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
}
