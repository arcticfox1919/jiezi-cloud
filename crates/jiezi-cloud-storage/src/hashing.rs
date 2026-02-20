//! SHA-256 hashing helpers for chunk content addressing.
//!
//! All chunks in the content-addressable store are keyed by the lowercase
//! hexadecimal SHA-256 digest of their raw bytes.  Two helpers are provided:
//!
//! - [`sha256_hex`] — synchronous, operates on an in-memory slice.
//! - [`sha256_hex_stream`] — async, reads from any [`tokio::io::AsyncRead`] in
//!   64 KiB chunks so that memory usage stays constant even for large files.

use sha2::{Digest, Sha256};
use tokio::io::AsyncReadExt;

use jiezi_cloud_core::error::AppResult;

const READ_BUF_SIZE: usize = 65_536; // 64 KiB

/// Compute the SHA-256 digest of `data` and return a lowercase hex string.
///
/// # Example
///
/// ```
/// use jiezi_cloud_storage::hashing::sha256_hex;
/// let hash = sha256_hex(b"");
/// assert_eq!(hash, "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
/// ```
pub fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}

/// Compute the SHA-256 digest of all bytes produced by `reader`.
///
/// Reads in 64 KiB chunks, keeping memory usage constant regardless of
/// the total stream length.
///
/// # Errors
///
/// Returns [`AppError::Storage`] if the underlying read fails.
pub async fn sha256_hex_stream(
    mut reader: impl tokio::io::AsyncRead + Unpin,
) -> AppResult<String> {
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; READ_BUF_SIZE];

    loop {
        let n = reader.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }

    Ok(hex::encode(hasher.finalize()))
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    // Known test vectors from NIST FIPS 180-4.

    #[test]
    fn test_sha256_empty_input() {
        let hash = sha256_hex(b"");
        assert_eq!(
            hash,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn test_sha256_non_empty_format() {
        // Verify that any non-empty input produces a well-formed 64-char lowercase hex string.
        let hash = sha256_hex(b"abc");
        assert_eq!(hash.len(), 64, "SHA-256 hex must be exactly 64 characters");
        assert!(
            hash.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()),
            "hash must be lowercase hexadecimal"
        );
    }

    #[test]
    fn test_sha256_deterministic() {
        let a = sha256_hex(b"hello world");
        let b = sha256_hex(b"hello world");
        assert_eq!(a, b, "same input must always produce same hash");
    }

    #[test]
    fn test_sha256_different_inputs() {
        let h1 = sha256_hex(b"foo");
        let h2 = sha256_hex(b"bar");
        assert_ne!(h1, h2, "different inputs must produce different hashes");
    }

    #[test]
    fn test_sha256_output_format() {
        let hash = sha256_hex(b"test data");
        assert_eq!(hash.len(), 64, "SHA-256 hex must be exactly 64 characters");
        assert!(
            hash.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()),
            "hash must be lowercase hexadecimal"
        );
    }

    #[tokio::test]
    async fn test_stream_matches_batch() {
        let data = b"stream vs batch consistency check";
        let batch = sha256_hex(data);

        let cursor = Cursor::new(data);
        let stream = sha256_hex_stream(cursor).await.expect("stream hash");

        assert_eq!(batch, stream, "stream and batch hashes must match");
    }

    #[tokio::test]
    async fn test_stream_empty_reader() {
        let cursor = Cursor::new(b"" as &[u8]);
        let hash = sha256_hex_stream(cursor).await.expect("empty stream hash");
        assert_eq!(
            hash,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[tokio::test]
    async fn test_stream_large_data() {
        // 300 KiB — crosses multiple read-buffer boundaries
        let data: Vec<u8> = (0u8..=255).cycle().take(300 * 1024).collect();
        let batch = sha256_hex(&data);
        let cursor = Cursor::new(data.clone());
        let stream = sha256_hex_stream(cursor).await.expect("large stream hash");
        assert_eq!(batch, stream);
    }
}
