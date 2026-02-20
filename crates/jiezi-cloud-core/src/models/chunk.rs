//! Content-defined chunk models used by the storage and upload subsystems.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::types::{BackendId, FileId};

// ─── ChunkInfo ────────────────────────────────────────────────────────────────

/// Metadata describing a single content-defined chunk produced by the CDC
/// chunking algorithm.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChunkInfo {
    /// SHA-256 hex digest of this chunk's content (used as the dedup key).
    pub hash: String,
    /// Size of this chunk in bytes.
    pub size: u32,
    /// Byte offset of this chunk within the original file.
    pub offset: u64,
    /// Zero-based position of this chunk in the file's sequence.
    pub index: u32,
}

// ─── ChunkRef ─────────────────────────────────────────────────────────────────

/// Links a file to one of its content chunks.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChunkRef {
    /// SHA-256 hash identifying the deduplicated chunk.
    pub chunk_hash: String,
    /// The file node this chunk belongs to.
    pub file_id: FileId,
    /// Zero-based position of this chunk within the file's sequence.
    pub sequence: u32,
}

// ─── StorageRef ───────────────────────────────────────────────────────────────

/// Points to the physical location of a chunk in a storage backend.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageRef {
    /// SHA-256 hash of the chunk (dedup key).
    pub chunk_hash: String,
    /// Which physical backend holds this chunk.
    pub backend_id: BackendId,
    /// Storage key within the backend (e.g. `"ab/abcdef1234…"`).
    pub key: String,
    /// Size of the stored object in bytes.
    pub size: u64,
}

// ─── UploadSession ────────────────────────────────────────────────────────────

/// Tracks an in-progress multipart upload session.
///
/// Created when a client starts a chunked upload and deleted once assembly
/// completes or the session expires.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UploadSession {
    /// Unique session token returned to the client.
    pub upload_id: String,
    /// The file node being assembled.
    pub file_id: FileId,
    /// Total expected file size in bytes.
    pub total_size: u64,
    /// Number of chunks successfully received and persisted.
    pub completed_chunks: u32,
    /// Total number of chunks expected.
    pub total_chunks: u32,
    /// When the session was opened.
    pub created_at: DateTime<Utc>,
}

impl UploadSession {
    /// Return `true` when all expected chunks have been received.
    pub fn is_complete(&self) -> bool {
        self.completed_chunks == self.total_chunks
    }

    /// Fraction of chunks received, in the range `[0.0, 1.0]`.
    pub fn progress(&self) -> f64 {
        if self.total_chunks == 0 {
            return 1.0;
        }
        f64::from(self.completed_chunks) / f64::from(self.total_chunks)
    }
}
