//! S3-compatible storage backend.
//!
//! Uses the [`object_store`] crate which supports AWS S3, MinIO, Cloudflare R2,
//! Backblaze B2, Wasabi and any other service that speaks the S3 API.
//!
//! # Construction
//!
//! Build an `S3Backend` from an already-configured
//! [`object_store::ObjectStore`] implementation:
//!
//! ```rust,no_run
//! use object_store::aws::AmazonS3Builder;
//! use jiezi_cloud_storage::s3::S3Backend;
//! use jiezi_cloud_core::types::BackendId;
//!
//! let store = AmazonS3Builder::new()
//!     .with_bucket_name("my-bucket")
//!     .with_region("us-east-1")
//!     .with_access_key_id("AKIA…")
//!     .with_secret_access_key("secret")
//!     // For S3-compatible services, override the endpoint:
//!     // .with_endpoint("https://s3.example.com")
//!     .build()
//!     .expect("valid S3 config");
//!
//! let backend = S3Backend::new(BackendId::new("primary-s3"), store, None);
//! ```

use std::ops::Range;
use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use object_store::{path::Path as ObjectPath, ObjectStore};
use tracing::{debug, instrument};

use jiezi_cloud_core::{
    error::{AppError, AppResult},
    types::{BackendId, HealthStatus},
};

use crate::hashing::validate_key;

/// Storage backend backed by any S3-compatible object-storage service.
pub struct S3Backend {
    id: BackendId,
    store: Arc<dyn ObjectStore>,
    /// Optional prefix prepended to every object key (e.g. `"jiezi-cloud/"`).
    prefix: Option<String>,
}

impl S3Backend {
    /// Create an [`S3Backend`] wrapping the provided [`ObjectStore`].
    ///
    /// `prefix` — if supplied, every chunk key will be stored as
    /// `"{prefix}{key}"` inside the bucket.  Should end with `/` if set.
    pub fn new(
        id: BackendId,
        store: impl ObjectStore + 'static,
        prefix: Option<String>,
    ) -> Self {
        Self {
            id,
            store: Arc::new(store),
            prefix,
        }
    }

    /// Build the [`ObjectPath`] for a given chunk key.
    fn object_path(&self, key: &str) -> ObjectPath {
        match &self.prefix {
            Some(p) => ObjectPath::from(format!("{p}{key}")),
            None => ObjectPath::from(key),
        }
    }
}

impl std::fmt::Debug for S3Backend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("S3Backend")
            .field("id", &self.id)
            .field("prefix", &self.prefix)
            .finish_non_exhaustive()
    }
}

#[async_trait]
impl jiezi_cloud_core::traits::storage::StorageBackend for S3Backend {
    fn backend_id(&self) -> &BackendId {
        &self.id
    }

    #[instrument(skip(self, data), fields(backend = %self.id, key, bytes = data.len()))]
    async fn put_chunk(&self, key: &str, data: Bytes) -> AppResult<()> {
        validate_key(key)?;
        let path = self.object_path(key);
        debug!(%path, bytes = data.len(), "S3 PUT");
        self.store
            .put(&path, data.into())
            .await
            .map_err(|e| AppError::Storage(format!("S3 PUT failed for key={key}: {e}")))?;
        Ok(())
    }

    #[instrument(skip(self), fields(backend = %self.id, key))]
    async fn get_chunk(&self, key: &str) -> AppResult<Bytes> {
        validate_key(key)?;
        let path = self.object_path(key);
        debug!(%path, "S3 GET");
        let result = self.store.get(&path).await.map_err(|e| match e {
            object_store::Error::NotFound { .. } => {
                AppError::NotFound(format!("chunk not found: {key}"))
            }
            other => AppError::Storage(format!("S3 GET failed for key={key}: {other}")),
        })?;
        let bytes = result
            .bytes()
            .await
            .map_err(|e| AppError::Storage(format!("S3 body read failed for key={key}: {e}")))?;
        Ok(bytes)
    }

    #[instrument(skip(self), fields(backend = %self.id, key, ?range))]
    async fn get_chunk_range(&self, key: &str, range: Range<u64>) -> AppResult<Bytes> {
        validate_key(key)?;
        // object_store expects Range<usize>
        let os_range = (range.start as usize)..(range.end as usize);
        let path = self.object_path(key);
        debug!(%path, ?os_range, "S3 GET range");
        let bytes = self
            .store
            .get_range(&path, os_range)
            .await
            .map_err(|e| match e {
                object_store::Error::NotFound { .. } => {
                    AppError::NotFound(format!("chunk not found: {key}"))
                }
                other => AppError::Storage(format!(
                    "S3 GET range failed for key={key}: {other}"
                )),
            })?;
        Ok(bytes)
    }

    #[instrument(skip(self), fields(backend = %self.id, key))]
    async fn delete_chunk(&self, key: &str) -> AppResult<()> {
        validate_key(key)?;
        let path = self.object_path(key);
        debug!(%path, "S3 DELETE");
        match self.store.delete(&path).await {
            Ok(()) => Ok(()),
            // A missing chunk is not an error for deletes (idempotent).
            Err(object_store::Error::NotFound { .. }) => Ok(()),
            Err(e) => Err(AppError::Storage(format!(
                "S3 DELETE failed for key={key}: {e}"
            ))),
        }
    }

    #[instrument(skip(self), fields(backend = %self.id, key))]
    async fn exists(&self, key: &str) -> AppResult<bool> {
        validate_key(key)?;
        let path = self.object_path(key);
        match self.store.head(&path).await {
            Ok(_) => Ok(true),
            Err(object_store::Error::NotFound { .. }) => Ok(false),
            Err(e) => Err(AppError::Storage(format!(
                "S3 HEAD failed for key={key}: {e}"
            ))),
        }
    }

    async fn health_check(&self) -> AppResult<HealthStatus> {
        // list_with_delimiter returns a Result (not a stream), so no extra
        // dependency is needed.  A successful call confirms bucket connectivity.
        // An empty prefix lists the bucket root; we don't care about the result.
        match self.store.list_with_delimiter(None).await {
            Ok(_) => Ok(HealthStatus::Healthy),
            Err(e) => Ok(HealthStatus::Unhealthy {
                reason: format!("S3 connectivity check failed: {e}"),
            }),
        }
    }
}

