//! Multi-backend storage manager.
//!
//! [`StorageManager`] holds a collection of [`StorageBackend`] implementations
//! and provides a facade that orchestrates reads and writes across them.
//!
//! # Write strategies
//!
//! | Method | Behaviour |
//! |--------|-----------|
//! | [`put_to_all`] | Write to **every** backend concurrently. |
//! | [`put_to`] | Write only to the listed `backend_ids`. |
//!
//! Both methods return a `Vec` of `(BackendId, AppResult<()>)` tuples so the
//! caller can inspect which backends succeeded and handle partial failures
//! according to its replication policy.
//!
//! # Read strategy
//!
//! [`get_from_any`] tries backends in **priority order** (lowest
//! `priority` value first) and returns the first successful response.
//!
//! # Runtime configuration
//!
//! Backends can be added with [`add_backend`] or removed with
//! [`remove_backend`] while the manager is live.  An `RwLock` protects the
//! backend list so that concurrent reads are not blocked.
//!
//! [`put_to_all`]: StorageManager::put_to_all
//! [`put_to`]: StorageManager::put_to
//! [`get_from_any`]: StorageManager::get_from_any
//! [`add_backend`]: StorageManager::add_backend
//! [`remove_backend`]: StorageManager::remove_backend

use std::ops::Range;
use std::sync::Arc;

use bytes::Bytes;
use tokio::sync::RwLock;
use tracing::{debug, warn};

use jiezi_cloud_core::{
    error::{AppError, AppResult},
    traits::storage::StorageBackend,
    types::{BackendId, HealthStatus},
};

// ─── StorageManager ───────────────────────────────────────────────────────────

/// Orchestrates reads and writes across multiple [`StorageBackend`]s.
pub struct StorageManager {
    /// Backends ordered by `priority` (ascending — lowest = highest priority).
    backends: RwLock<Vec<Arc<dyn StorageBackend>>>,
}

impl StorageManager {
    /// Create an empty [`StorageManager`].
    pub fn new() -> Self {
        Self {
            backends: RwLock::new(Vec::new()),
        }
    }

    /// Create a [`StorageManager`] pre-loaded with a list of backends.
    ///
    /// Backends are stored in the order provided; callers should sort by
    /// priority before passing them in.
    pub fn with_backends(backends: Vec<Arc<dyn StorageBackend>>) -> Self {
        Self {
            backends: RwLock::new(backends),
        }
    }

    // ── Backend management ────────────────────────────────────────────────────

    /// Append a backend to the end of the priority list.
    pub async fn add_backend(&self, backend: Arc<dyn StorageBackend>) {
        let mut backends = self.backends.write().await;
        backends.push(backend);
    }

    /// Remove the backend with the given ID, if present.
    ///
    /// Returns `true` if a backend was removed.
    pub async fn remove_backend(&self, id: &BackendId) -> bool {
        let mut backends = self.backends.write().await;
        let before = backends.len();
        backends.retain(|b| b.backend_id() != id);
        backends.len() < before
    }

    /// Return a snapshot of all registered backend IDs (in priority order).
    pub async fn backend_ids(&self) -> Vec<BackendId> {
        self.backends
            .read()
            .await
            .iter()
            .map(|b| b.backend_id().clone())
            .collect()
    }

    // ── Write paths ───────────────────────────────────────────────────────────

    /// Write `data` under `key` to **all** registered backends concurrently.
    ///
    /// Returns one result per backend.  The caller decides how many failures
    /// are acceptable based on its [`ReplicationPolicy`].
    ///
    /// [`ReplicationPolicy`]: jiezi_cloud_core::models::backend::ReplicationPolicy
    pub async fn put_to_all(
        &self,
        key: &str,
        data: Bytes,
    ) -> Vec<(BackendId, AppResult<()>)> {
        let backends: Vec<Arc<dyn StorageBackend>> =
            self.backends.read().await.clone();

        let mut handles = Vec::with_capacity(backends.len());
        for backend in backends {
            let key = key.to_owned();
            let data = data.clone();
            handles.push(tokio::spawn(async move {
                let id = backend.backend_id().clone();
                let result = backend.put_chunk(&key, data).await;
                (id, result)
            }));
        }

        let mut results = Vec::with_capacity(handles.len());
        for h in handles {
            match h.await {
                Ok(r) => results.push(r),
                Err(join_err) => {
                    warn!(%join_err, "backend write task panicked");
                    // We don't know which backend panicked without extra bookkeeping;
                    // record a generic internal error.
                    results.push((
                        BackendId::new("unknown"),
                        Err(AppError::Internal(format!(
                            "write task panicked: {join_err}"
                        ))),
                    ));
                }
            }
        }
        results
    }

    /// Write `data` under `key` to only the backends named in `backend_ids`.
    ///
    /// Backends not in the slice are skipped entirely.
    pub async fn put_to(
        &self,
        key: &str,
        data: Bytes,
        backend_ids: &[BackendId],
    ) -> Vec<(BackendId, AppResult<()>)> {
        let backends: Vec<Arc<dyn StorageBackend>> = {
            let all = self.backends.read().await;
            all.iter()
                .filter(|b| backend_ids.contains(b.backend_id()))
                .cloned()
                .collect()
        };

        let mut handles = Vec::with_capacity(backends.len());
        for backend in backends {
            let key = key.to_owned();
            let data = data.clone();
            handles.push(tokio::spawn(async move {
                let id = backend.backend_id().clone();
                let result = backend.put_chunk(&key, data).await;
                (id, result)
            }));
        }

        let mut results = Vec::with_capacity(handles.len());
        for h in handles {
            match h.await {
                Ok(r) => results.push(r),
                Err(join_err) => {
                    warn!(%join_err, "backend write task panicked");
                    results.push((
                        BackendId::new("unknown"),
                        Err(AppError::Internal(format!(
                            "write task panicked: {join_err}"
                        ))),
                    ));
                }
            }
        }
        results
    }

    // ── Read path ─────────────────────────────────────────────────────────────

    /// Retrieve a chunk by trying backends in priority order.
    ///
    /// Returns the content from the first backend that has it.  Logs a warning
    /// for each backend that returns an error before eventually succeeding.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::NotFound`] if no backend has the chunk.
    /// Returns [`AppError::Storage`] if all backends returned errors.
    pub async fn get_from_any(&self, key: &str) -> AppResult<Bytes> {
        let backends: Vec<Arc<dyn StorageBackend>> =
            self.backends.read().await.clone();

        if backends.is_empty() {
            return Err(AppError::Storage(
                "no backends registered in StorageManager".to_owned(),
            ));
        }

        let mut last_err: Option<AppError> = None;

        for backend in &backends {
            match backend.get_chunk(key).await {
                Ok(bytes) => {
                    debug!(
                        backend = %backend.backend_id(),
                        key,
                        bytes = bytes.len(),
                        "chunk read from backend"
                    );
                    return Ok(bytes);
                }
                Err(AppError::NotFound(_)) => {
                    debug!(backend = %backend.backend_id(), key, "chunk not found on backend");
                    last_err = Some(AppError::NotFound(format!("chunk not found: {key}")));
                }
                Err(e) => {
                    warn!(
                        backend = %backend.backend_id(),
                        key,
                        error = %e,
                        "backend error reading chunk; trying next backend"
                    );
                    last_err = Some(e);
                }
            }
        }

        Err(last_err.unwrap_or_else(|| {
            AppError::NotFound(format!("chunk not found on any backend: {key}"))
        }))
    }

    /// Retrieve a byte range of a chunk, trying backends in priority order.
    pub async fn get_range_from_any(
        &self,
        key: &str,
        range: Range<u64>,
    ) -> AppResult<Bytes> {
        let backends: Vec<Arc<dyn StorageBackend>> =
            self.backends.read().await.clone();

        if backends.is_empty() {
            return Err(AppError::Storage(
                "no backends registered in StorageManager".to_owned(),
            ));
        }

        let mut last_err: Option<AppError> = None;

        for backend in &backends {
            match backend.get_chunk_range(key, range.clone()).await {
                Ok(bytes) => return Ok(bytes),
                Err(AppError::NotFound(_)) => {
                    last_err = Some(AppError::NotFound(format!("chunk not found: {key}")));
                }
                Err(e) => {
                    warn!(
                        backend = %backend.backend_id(),
                        key, error = %e,
                        "backend range read error; trying next"
                    );
                    last_err = Some(e);
                }
            }
        }

        Err(last_err.unwrap_or_else(|| {
            AppError::NotFound(format!("chunk not found on any backend: {key}"))
        }))
    }

    // ── Health checks ─────────────────────────────────────────────────────────

    /// Run [`health_check`](StorageBackend::health_check) on all backends
    /// concurrently and return a `Vec` of `(BackendId, HealthStatus)`.
    pub async fn health_check_all(&self) -> Vec<(BackendId, HealthStatus)> {
        let backends: Vec<Arc<dyn StorageBackend>> =
            self.backends.read().await.clone();

        let mut handles = Vec::with_capacity(backends.len());
        for backend in backends {
            handles.push(tokio::spawn(async move {
                let id = backend.backend_id().clone();
                let status = backend.health_check().await.unwrap_or_else(|e| {
                    HealthStatus::Unhealthy {
                        reason: e.to_string(),
                    }
                });
                (id, status)
            }));
        }

        let mut results = Vec::with_capacity(handles.len());
        for h in handles {
            match h.await {
                Ok(r) => results.push(r),
                Err(join_err) => {
                    warn!(%join_err, "health check task panicked");
                    results.push((
                        BackendId::new("unknown"),
                        HealthStatus::Unhealthy {
                            reason: format!("health check task panicked: {join_err}"),
                        },
                    ));
                }
            }
        }
        results
    }
}

impl Default for StorageManager {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for StorageManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StorageManager").finish_non_exhaustive()
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::ops::Range;

    use async_trait::async_trait;
    use bytes::Bytes;

    use jiezi_cloud_core::{
        error::AppResult,
        types::{BackendId, HealthStatus},
    };
    use jiezi_cloud_core::traits::storage::StorageBackend;

    // ── Minimal in-memory stub ────────────────────────────────────────

    #[derive(Debug)]
    struct MemBackend {
        id: BackendId,
        data: tokio::sync::RwLock<std::collections::HashMap<String, Bytes>>,
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

    // ── Tests ─────────────────────────────────────────────────────────

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
        let b1 = HealthStatus::Healthy;
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

        let _ = b1; // suppress unused
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
}
