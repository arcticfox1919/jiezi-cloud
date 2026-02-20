//! SHA-256 hashing helpers and chunk-key validation.
//!
//! All chunks in the content-addressable store are keyed by the lowercase
//! hexadecimal SHA-256 digest of their raw bytes.  Two helpers are provided:
//!
//! - [`sha256_hex`] — synchronous, operates on an in-memory slice.
//! - [`sha256_hex_stream`] — async, reads from any [`tokio::io::AsyncRead`] in
//!   64 KiB chunks so that memory usage stays constant even for large files.
//!
//! A shared [`validate_key`] function guards every backend implementation
//! against path-traversal attacks.

use sha2::{Digest, Sha256};
use tokio::io::AsyncReadExt;

use jiezi_cloud_core::error::{AppError, AppResult};

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

// ─── Key validation ───────────────────────────────────────────────────────────

/// Validate that a chunk key is safe to use as a relative path component.
///
/// Rejects keys that:
/// - Are empty.
/// - Are absolute (start with `/`, `\`, or a Windows drive like `C:`).
/// - Contain path traversal segments (`..` or `.`).
/// - Contain backslashes (always use `/` as separator).
///
/// # Errors
///
/// Returns [`AppError::Validation`] for any of the invalid cases above.
pub fn validate_key(key: &str) -> AppResult<()> {
    if key.is_empty() {
        return Err(AppError::Validation("chunk key must not be empty".to_owned()));
    }
    if key.starts_with('/') || key.starts_with('\\') {
        return Err(AppError::Validation(format!(
            "chunk key must be relative, got: {key}"
        )));
    }
    // Windows drive letter e.g. "C:" or "C:\"
    if key.len() >= 2
        && key.chars().nth(1) == Some(':')
        && key.chars().next().map_or(false, |c| c.is_ascii_alphabetic())
    {
        return Err(AppError::Validation(format!(
            "chunk key must be relative, got: {key}"
        )));
    }
    if key.contains('\\') {
        return Err(AppError::Validation(format!(
            "chunk key must use '/' separators, got: {key}"
        )));
    }
    for component in key.split('/') {
        if component == ".." || component == "." {
            return Err(AppError::Validation(format!(
                "chunk key contains path traversal: {key}"
            )));
        }
    }
    Ok(())
}
