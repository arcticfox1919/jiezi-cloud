//! Multi-backend storage manager.
//!
//! [`StorageManager`] holds a collection of [`StorageBackend`] implementations
//! and provides a facade that orchestrates reads and writes across them.
//!
//! # Design note
//!
//! No backend type is "primary".  The priority ordering is simply used to
//! determine which backend to read from first in [`get_from_any`].  A
//! deployment with only S3 backends, only WebDAV backends, or any mix is
//! fully supported — local filesystem storage is never assumed.
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
    models::backend::ReplicationPolicy,
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

    // ── ReplicationPolicy execution ───────────────────────────────────────────

    /// Write `data` under `key` to backends selected by `policy`.
    ///
    /// | Policy                    | Behaviour                                          |
    /// |---------------------------|----------------------------------------------------|
    /// | [`ReplicationPolicy::All`]      | Same as [`put_to_all`]                    |
    /// | [`ReplicationPolicy::MinN`]     | First `n` backends by priority order      |
    /// | [`ReplicationPolicy::Specific`] | Same as [`put_to`] with the listed IDs    |
    ///
    /// Returns one result per selected backend.  Backends not targeted by the
    /// policy are not written to and do not appear in the result set.
    ///
    /// If a `MinN` policy requests more backends than are registered, the write
    /// proceeds with however many are available (and a warning is logged).
    ///
    /// [`put_to_all`]: StorageManager::put_to_all
    /// [`put_to`]: StorageManager::put_to
    pub async fn put_with_policy(
        &self,
        key: &str,
        data: Bytes,
        policy: &ReplicationPolicy,
    ) -> Vec<(BackendId, AppResult<()>)> {
        match policy {
            ReplicationPolicy::All => self.put_to_all(key, data).await,

            ReplicationPolicy::MinN { n } => {
                let ids: Vec<BackendId> = {
                    let backends = self.backends.read().await;
                    let available = backends.len();
                    if *n > available {
                        warn!(
                            requested = n,
                            available,
                            "MinN replication policy requested more backends than available; \
                             writing to all {available} backends"
                        );
                    }
                    backends.iter().take(*n).map(|b| b.backend_id().clone()).collect()
                };
                self.put_to(key, data, &ids).await
            }

            ReplicationPolicy::Specific { backend_ids } => {
                self.put_to(key, data, backend_ids).await
            }
        }
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
