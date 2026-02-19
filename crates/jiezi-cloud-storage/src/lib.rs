//! Storage backends and chunking utilities for Jiezi Cloud.
//!
//! Implements [`jiezi_cloud_core::traits::storage::StorageBackend`] for
//! local filesystem storage.  S3/MinIO and WebDAV backends are planned.

pub mod chunking;
pub mod hashing;
pub mod local;
