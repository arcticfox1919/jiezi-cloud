//! Integration tests for SHA-256 hashing utilities.

use std::io::Cursor;

use jiezi_cloud_storage::hashing::{sha256_hex, sha256_hex_stream};

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
