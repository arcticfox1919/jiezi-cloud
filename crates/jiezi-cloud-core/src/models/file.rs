//! File and directory node models for the virtual file system.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::types::{FileId, SpaceId, UserId};

// ─── NodeType ─────────────────────────────────────────────────────────────────

/// The kind of a virtual file system node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum NodeType {
    /// A regular file with binary content.
    File,
    /// A container that holds other nodes.
    Directory,
    /// A symbolic link pointing to another node.
    Symlink,
}

// ─── FileNode ─────────────────────────────────────────────────────────────────

/// A node in the virtual file system — either a file or a directory.
///
/// File content is stored separately by the storage backend; this struct
/// holds only the metadata that lives in the database.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
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
    /// MIME type detected at upload time (e.g. `"image/jpeg"`).
    pub mime_type: Option<String>,
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
#[derive(Debug, Clone, Default, Serialize, Deserialize, utoipa::ToSchema)]
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
    mime_type: Option<String>,
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

    pub fn mime_type(mut self, mime_type: impl Into<String>) -> Self {
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
