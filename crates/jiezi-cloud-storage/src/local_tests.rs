// Tests for LocalFsBackend.
//
// Declared from local.rs as:
//     #[cfg(test)]
//     #[path = "local_tests.rs"]
//     mod tests;
//
// `use super::*` gives access to private items (chunk_path) and all
// crate-local imports.

use super::*;
use jiezi_cloud_core::traits::storage::StorageBackend;

/// Create a backend in a temporary directory that is removed after the test.
fn make_backend(tmp: &tempfile::TempDir) -> LocalFsBackend {
    LocalFsBackend::new("test-backend", tmp.path())
}

/// Helper: derive the canonical key for a byte slice.
fn key_for(data: &[u8]) -> String {
    let hash = sha256_hex(data);
    let prefix = &hash[..2];
    format!("{prefix}/{hash}")
}

// ── put_chunk / get_chunk ────────────────────────────────────────────────

#[tokio::test]
async fn test_put_and_get_round_trip() {
    let tmp = tempfile::tempdir().unwrap();
    let backend = make_backend(&tmp);

    let data = Bytes::from_static(b"hello, local storage!");
    let key = key_for(&data);

    backend.put_chunk(&key, data.clone()).await.unwrap();
    let retrieved = backend.get_chunk(&key).await.unwrap();
    assert_eq!(retrieved, data);
}

#[tokio::test]
async fn test_put_is_idempotent() {
    let tmp = tempfile::tempdir().unwrap();
    let backend = make_backend(&tmp);

    let data = Bytes::from_static(b"idempotent write");
    let key = key_for(&data);

    backend.put_chunk(&key, data.clone()).await.unwrap();
    // Second write must succeed without error
    backend.put_chunk(&key, data.clone()).await.unwrap();

    let retrieved = backend.get_chunk(&key).await.unwrap();
    assert_eq!(retrieved, data);
}

#[tokio::test]
async fn test_get_nonexistent_returns_not_found() {
    let tmp = tempfile::tempdir().unwrap();
    let backend = make_backend(&tmp);

    let hash = "a".repeat(64);
    let key = format!("aa/{hash}");
    let err = backend.get_chunk(&key).await.unwrap_err();
    assert!(
        matches!(err, AppError::NotFound(_)),
        "expected NotFound, got: {err:?}"
    );
}

// ── exists ───────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_exists_after_put() {
    let tmp = tempfile::tempdir().unwrap();
    let backend = make_backend(&tmp);

    let data = Bytes::from_static(b"existence check");
    let key = key_for(&data);

    assert!(!backend.exists(&key).await.unwrap());
    backend.put_chunk(&key, data).await.unwrap();
    assert!(backend.exists(&key).await.unwrap());
}

// ── delete_chunk ─────────────────────────────────────────────────────────

#[tokio::test]
async fn test_delete_removes_chunk() {
    let tmp = tempfile::tempdir().unwrap();
    let backend = make_backend(&tmp);

    let data = Bytes::from_static(b"to be deleted");
    let key = key_for(&data);

    backend.put_chunk(&key, data).await.unwrap();
    assert!(backend.exists(&key).await.unwrap());

    backend.delete_chunk(&key).await.unwrap();
    assert!(!backend.exists(&key).await.unwrap());
}

#[tokio::test]
async fn test_delete_nonexistent_is_noop() {
    let tmp = tempfile::tempdir().unwrap();
    let backend = make_backend(&tmp);

    let hash = "b".repeat(64);
    let key = format!("bb/{hash}");
    // Must not return an error
    backend.delete_chunk(&key).await.unwrap();
}

// ── get_chunk_range ──────────────────────────────────────────────────────

#[tokio::test]
async fn test_range_read_correct_slice() {
    let tmp = tempfile::tempdir().unwrap();
    let backend = make_backend(&tmp);

    let data = Bytes::from_static(b"0123456789abcdef");
    let key = key_for(&data);
    backend.put_chunk(&key, data).await.unwrap();

    let slice = backend.get_chunk_range(&key, 4..8).await.unwrap();
    assert_eq!(&slice[..], b"4567");
}

#[tokio::test]
async fn test_range_full_file() {
    let tmp = tempfile::tempdir().unwrap();
    let backend = make_backend(&tmp);

    let data = Bytes::from_static(b"full range");
    let key = key_for(&data);
    backend.put_chunk(&key, data.clone()).await.unwrap();

    let slice = backend.get_chunk_range(&key, 0..data.len() as u64).await.unwrap();
    assert_eq!(slice, data);
}

#[tokio::test]
async fn test_range_out_of_bounds_returns_error() {
    let tmp = tempfile::tempdir().unwrap();
    let backend = make_backend(&tmp);

    let data = Bytes::from_static(b"short");
    let key = key_for(&data);
    backend.put_chunk(&key, data).await.unwrap();

    let err = backend.get_chunk_range(&key, 0..100).await.unwrap_err();
    assert!(
        matches!(err, AppError::Validation(_)),
        "out-of-bounds range must return Validation error, got: {err:?}"
    );
}

// ── path validation ──────────────────────────────────────────────────────

#[tokio::test]
async fn test_key_with_path_traversal_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    let backend = make_backend(&tmp);

    let bad_keys = [
        "../escape",
        "ab/../etc/passwd",
        "/absolute",
        "C:\\windows",
        "sub/./file",
    ];

    for key in bad_keys {
        let err = backend.put_chunk(key, Bytes::from_static(b"x")).await.unwrap_err();
        assert!(
            matches!(err, AppError::Validation(_)),
            "key '{key}' must be rejected with Validation error, got: {err:?}"
        );
    }
}

#[tokio::test]
async fn test_empty_key_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    let backend = make_backend(&tmp);
    let err = backend.put_chunk("", Bytes::from_static(b"x")).await.unwrap_err();
    assert!(matches!(err, AppError::Validation(_)));
}

// ── integrity check ──────────────────────────────────────────────────────

#[tokio::test]
async fn test_get_detects_corrupted_file() {
    let tmp = tempfile::tempdir().unwrap();
    let backend = make_backend(&tmp);

    let data = Bytes::from_static(b"original content");
    let key = key_for(&data);
    backend.put_chunk(&key, data.clone()).await.unwrap();

    // Corrupt the file on disk by overwriting with different content.
    // chunk_path() is a private method — accessible only from this sibling file.
    let on_disk = backend.chunk_path(&key);
    tokio::fs::write(&on_disk, b"CORRUPTED DATA!!!").await.unwrap();

    let err = backend.get_chunk(&key).await.unwrap_err();
    assert!(
        matches!(err, AppError::Storage(_)),
        "corrupted chunk must return Storage error, got: {err:?}"
    );
}

// ── health_check ─────────────────────────────────────────────────────────

#[tokio::test]
async fn test_health_check_new_backend() {
    let tmp = tempfile::tempdir().unwrap();
    // Pre-create root so health_check can find it
    tokio::fs::create_dir_all(tmp.path()).await.unwrap();
    let backend = make_backend(&tmp);
    // health_check should succeed on a valid directory
    let status = backend.health_check().await.unwrap();
    assert!(status.is_healthy(), "new backend must be healthy");
}

#[tokio::test]
async fn test_health_check_missing_root() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("does_not_exist");
    let backend = LocalFsBackend::new("ghost", root);
    let status = backend.health_check().await.unwrap();
    assert!(
        !status.is_healthy(),
        "backend with missing root must not be healthy"
    );
}

// ── concurrent write ─────────────────────────────────────────────────────

#[tokio::test]
async fn test_concurrent_writes_same_key() {
    use std::sync::Arc;

    let tmp = tempfile::tempdir().unwrap();
    let backend = Arc::new(make_backend(&tmp));

    let data = Bytes::from_static(b"concurrent");
    let key = key_for(&data);

    let mut handles = Vec::new();
    for _ in 0..8 {
        let b = Arc::clone(&backend);
        let k = key.clone();
        let d = data.clone();
        handles.push(tokio::spawn(async move { b.put_chunk(&k, d).await }));
    }

    for h in handles {
        h.await.unwrap().unwrap();
    }

    // Final state must be correct, readable, and untampered
    let retrieved = backend.get_chunk(&key).await.unwrap();
    assert_eq!(retrieved, data);
}
