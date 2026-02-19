//! Cache provider trait.

use std::time::Duration;

use async_trait::async_trait;
use serde::{de::DeserializeOwned, Serialize};

use crate::error::AppResult;

/// Generic, async cache provider contract.
///
/// The key space is flat (plain strings).  Callers are responsible for
/// namespacing keys to avoid collisions, e.g. `"user:42:profile"`.
///
/// # Concrete implementations
///
/// - **`MokaCache`** — in-process, high-performance LRU/W-TinyLFU cache
///   backed by the `moka` crate.  Used in single-node deployments.
/// - **`RedisCache`** *(future)* — Redis-backed cache for cluster deployments.
///
/// # Value encoding
///
/// Values are serialised to JSON before storage and deserialised on retrieval
/// so that the underlying cache store is always opaque to callers.
#[async_trait]
pub trait CacheProvider: Send + Sync {
    /// Fetch a cached value.
    ///
    /// Returns `Ok(None)` on a cache miss (key not found or expired).
    ///
    /// # Errors
    ///
    /// - [`AppError::Serialization`] if the stored bytes cannot be deserialised.
    /// - [`AppError::Internal`] on backend errors.
    async fn get<T>(&self, key: &str) -> AppResult<Option<T>>
    where
        T: DeserializeOwned + Send;

    /// Store a value with a time-to-live.
    ///
    /// Overwrites any existing entry at `key`.
    ///
    /// # Errors
    ///
    /// - [`AppError::Serialization`] if the value cannot be serialised.
    /// - [`AppError::Internal`] on backend errors.
    async fn set<T>(&self, key: &str, value: &T, ttl: Duration) -> AppResult<()>
    where
        T: Serialize + Send + Sync;

    /// Invalidate a single cache entry.
    ///
    /// This is a no-op if the key does not exist.
    ///
    /// # Errors
    ///
    /// - [`AppError::Internal`] on backend errors.
    async fn delete(&self, key: &str) -> AppResult<()>;

    /// Invalidate all entries whose keys start with `prefix`.
    ///
    /// Useful for bulk-invalidation when a resource (e.g. a space) changes.
    ///
    /// # Errors
    ///
    /// - [`AppError::Internal`] on backend errors.
    async fn invalidate_prefix(&self, prefix: &str) -> AppResult<()>;
}
