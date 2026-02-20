//! Integration tests for `StorageManager` — multi-backend orchestration.

use std::collections::HashMap;
use std::ops::Range;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use async_trait::async_trait;
use bytes::Bytes;

use jiezi_cloud_core::{
    error::{AppError, AppResult},
    traits::storage::StorageBackend,
    types::{BackendId, HealthStatus},
};
use jiezi_cloud_storage::StorageManager;

// ── Minimal in-memory stub ────────────────────────────────────────────────

#[derive(Debug)]
struct MemBackend {
    id: BackendId,
    data: tokio::sync::RwLock<HashMap<String, Bytes>>,
    put_count: Arc<AtomicUsize>,
    healthy: bool,
}

impl MemBackend {
    fn new(id: &str) -> Arc<Self> {
        Arc::new(Self {
            id: BackendId::new(id),
            data: Default::default(),
            put_count: Arc::new(AtomicUsize::new(0)),
            healthy: true,
        })
    }

    fn new_unhealthy(id: &str) -> Arc<Self> {
        Arc::new(Self {
            id: BackendId::new(id),
            data: Default::default(),
            put_count: Arc::new(AtomicUsize::new(0)),
            healthy: false,
        })
    }
}

#[async_trait]
impl StorageBackend for MemBackend {
    fn backend_id(&self) -> &BackendId { &self.id }

    async fn put_chunk(&self, key: &str, data: Bytes) -> AppResult<()> {
        self.put_count.fetch_add(1, Ordering::SeqCst);
        self.data.write().await.insert(key.to_owned(), data);
        Ok(())
    }

    async fn get_chunk(&self, key: &str) -> AppResult<Bytes> {
        self.data
            .read()
            .await
            .get(key)
            .cloned()
            .ok_or_else(|| AppError::NotFound(key.to_owned()))
    }

    async fn get_chunk_range(&self, key: &str, range: Range<u64>) -> AppResult<Bytes> {
        let bytes = self.get_chunk(key).await?;
        let start = range.start as usize;
        let end = (range.end as usize).min(bytes.len());
        Ok(bytes.slice(start..end))
    }

    async fn delete_chunk(&self, key: &str) -> AppResult<()> {
        self.data.write().await.remove(key);
        Ok(())
    }

    async fn exists(&self, key: &str) -> AppResult<bool> {
        Ok(self.data.read().await.contains_key(key))
    }

    async fn health_check(&self) -> AppResult<HealthStatus> {
        if self.healthy {
            Ok(HealthStatus::Healthy)
        } else {
            Ok(HealthStatus::Unhealthy { reason: "stub unhealthy".into() })
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────

#[tokio::test]
async fn put_to_all_writes_to_every_backend() {
    let b1 = MemBackend::new("b1");
    let b2 = MemBackend::new("b2");
    let b3 = MemBackend::new("b3");

    let mgr = StorageManager::with_backends(vec![
        b1.clone() as Arc<dyn StorageBackend>,
        b2.clone() as Arc<dyn StorageBackend>,
        b3.clone() as Arc<dyn StorageBackend>,
    ]);

    let data = Bytes::from_static(b"hello");
    let results = mgr.put_to_all("ab/key", data).await;

    assert_eq!(results.len(), 3);
    for (_, r) in &results {
        assert!(r.is_ok(), "expected Ok but got {r:?}");
    }
    assert_eq!(b1.put_count.load(Ordering::SeqCst), 1);
    assert_eq!(b2.put_count.load(Ordering::SeqCst), 1);
    assert_eq!(b3.put_count.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn put_to_writes_only_to_selected_backends() {
    let b1 = MemBackend::new("b1");
    let b2 = MemBackend::new("b2");
    let b3 = MemBackend::new("b3");

    let mgr = StorageManager::with_backends(vec![
        b1.clone() as Arc<dyn StorageBackend>,
        b2.clone() as Arc<dyn StorageBackend>,
        b3.clone() as Arc<dyn StorageBackend>,
    ]);

    let data = Bytes::from_static(b"hello");
    let targets = vec![BackendId::new("b1"), BackendId::new("b3")];
    let results = mgr.put_to("ab/key", data, &targets).await;

    assert_eq!(results.len(), 2);
    assert_eq!(b1.put_count.load(Ordering::SeqCst), 1, "b1 should be written");
    assert_eq!(b2.put_count.load(Ordering::SeqCst), 0, "b2 should be skipped");
    assert_eq!(b3.put_count.load(Ordering::SeqCst), 1, "b3 should be written");
}

#[tokio::test]
async fn get_from_any_returns_first_available() {
    let b1 = MemBackend::new("b1"); // has the chunk
    let b2 = MemBackend::new("b2"); // does NOT have the chunk

    b1.put_chunk("ab/key", Bytes::from_static(b"data"))
        .await
        .unwrap();

    let mgr = StorageManager::with_backends(vec![
        b1.clone() as Arc<dyn StorageBackend>,
        b2.clone() as Arc<dyn StorageBackend>,
    ]);

    let bytes = mgr.get_from_any("ab/key").await.unwrap();
    assert_eq!(bytes, Bytes::from_static(b"data"));
}

#[tokio::test]
async fn get_from_any_falls_back_if_primary_missing() {
    let b1 = MemBackend::new("b1"); // does NOT have the chunk
    let b2 = MemBackend::new("b2"); // HAS the chunk

    b2.put_chunk("ab/key", Bytes::from_static(b"fallback"))
        .await
        .unwrap();

    let mgr = StorageManager::with_backends(vec![
        b1 as Arc<dyn StorageBackend>,
        b2 as Arc<dyn StorageBackend>,
    ]);

    let bytes = mgr.get_from_any("ab/key").await.unwrap();
    assert_eq!(bytes, Bytes::from_static(b"fallback"));
}

#[tokio::test]
async fn get_from_any_returns_not_found_when_no_backend_has_chunk() {
    let b1 = MemBackend::new("b1");
    let mgr = StorageManager::with_backends(vec![b1 as Arc<dyn StorageBackend>]);

    let err = mgr.get_from_any("ab/missing").await.unwrap_err();
    assert!(matches!(err, AppError::NotFound(_)), "expected NotFound, got {err:?}");
}

#[tokio::test]
async fn health_check_all_reports_per_backend() {
    let healthy = MemBackend::new("healthy");
    let sick = MemBackend::new_unhealthy("sick");

    let mgr = StorageManager::with_backends(vec![
        healthy as Arc<dyn StorageBackend>,
        sick as Arc<dyn StorageBackend>,
    ]);

    let statuses = mgr.health_check_all().await;
    assert_eq!(statuses.len(), 2);
    let healthy_status = statuses.iter().find(|(id, _)| id.as_str() == "healthy").unwrap();
    let sick_status = statuses.iter().find(|(id, _)| id.as_str() == "sick").unwrap();
    assert!(healthy_status.1.is_healthy());
    assert!(!sick_status.1.is_healthy());
}

#[tokio::test]
async fn add_and_remove_backend() {
    let mgr = StorageManager::new();
    assert_eq!(mgr.backend_ids().await.len(), 0);

    let b1 = MemBackend::new("b1");
    mgr.add_backend(b1 as Arc<dyn StorageBackend>).await;
    assert_eq!(mgr.backend_ids().await.len(), 1);

    let removed = mgr.remove_backend(&BackendId::new("b1")).await;
    assert!(removed);
    assert_eq!(mgr.backend_ids().await.len(), 0);

    // Removing a non-existent backend returns false.
    let removed_again = mgr.remove_backend(&BackendId::new("b1")).await;
    assert!(!removed_again);
}

#[tokio::test]
async fn empty_manager_returns_storage_error() {
    let mgr = StorageManager::new();
    let err = mgr.get_from_any("ab/key").await.unwrap_err();
    assert!(matches!(err, AppError::Storage(_)));
}
