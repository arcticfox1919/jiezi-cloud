//! Content-Defined Chunking (CDC) using the FastCDC algorithm.
//!
//! FastCDC splits a byte stream into variable-size chunks whose boundaries
//! are determined by the content itself (via a rolling hash).  Compared to
//! fixed-size splitting, CDC means that inserting bytes near the beginning
//! of a file only affects a small number of boundary decisions near the
//! insertion point — the rest of the chunks remain stable.  This property
//! is critical for efficient incremental sync and deduplication.
//!
//! # Usage
//!
//! ```
//! use jiezi_cloud_storage::chunking::FastCdcChunker;
//!
//! let chunker = FastCdcChunker::default(); // 512 B / 4 KiB / 16 KiB
//! let chunks = chunker.chunk(b"hello world, this is some test data");
//! for c in &chunks {
//!     println!("offset={} size={} hash={}", c.offset, c.size, c.hash);
//! }
//! ```

use bytes::Bytes;
use fastcdc::v2020::FastCDC;

use crate::hashing::sha256_hex;

/// Parameters for the FastCDC algorithm.
///
/// All three values are in bytes.  The FastCDC algorithm will try to produce
/// chunks close to `avg_size`, never smaller than `min_size` (except for the
/// last chunk), and never larger than `max_size`.
#[derive(Debug, Clone, Copy)]
pub struct FastCdcChunker {
    /// Minimum chunk size in bytes.
    pub min_size: u32,
    /// Target (average) chunk size in bytes.
    pub avg_size: u32,
    /// Maximum chunk size in bytes.
    pub max_size: u32,
}

impl Default for FastCdcChunker {
    /// Returns a chunker tuned for general-purpose file storage:
    /// min 512 B, avg 4 KiB, max 16 KiB.
    fn default() -> Self {
        Self {
            min_size: 512,
            avg_size: 4_096,
            max_size: 16_384,
        }
    }
}

/// A single content-defined chunk produced by [`FastCdcChunker::chunk`].
#[derive(Debug, Clone)]
pub struct ChunkInfo {
    /// SHA-256 hex digest of [`ChunkInfo::data`].
    pub hash: String,
    /// Byte offset of this chunk within the original input.
    pub offset: u64,
    /// Size of this chunk in bytes.
    pub size: u32,
    /// Raw bytes of this chunk.
    pub data: Bytes,
}

impl FastCdcChunker {
    /// Create a new chunker with explicit size parameters.
    pub fn new(min_size: u32, avg_size: u32, max_size: u32) -> Self {
        Self { min_size, avg_size, max_size }
    }

    /// Split `data` into a list of content-defined chunks.
    ///
    /// Each returned [`ChunkInfo`] owns a copy of its bytes.  The chunks are
    /// non-overlapping, contiguous, and collectively cover all of `data`.
    ///
    /// # Determinism
    ///
    /// Given the same `data` and the same chunker parameters, the result is
    /// always identical.
    pub fn chunk(&self, data: &[u8]) -> Vec<ChunkInfo> {
        if data.is_empty() {
            return Vec::new();
        }

        FastCDC::new(data, self.min_size, self.avg_size, self.max_size)
            .map(|c| {
                let chunk_data = Bytes::copy_from_slice(&data[c.offset..c.offset + c.length]);
                let hash = sha256_hex(&chunk_data);
                ChunkInfo {
                    hash,
                    offset: c.offset as u64,
                    size: c.length as u32,
                    data: chunk_data,
                }
            })
            .collect()
    }
}
