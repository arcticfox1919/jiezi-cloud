//! Storage backends and chunking utilities for Jiezi Cloud.
//!
//! # Design principle
//!
//! Jiezi Cloud is a **document management system**, not a self-hosted NAS.
//! **Local filesystem storage is entirely optional.**  A deployment may run
//! with zero local storage, relying entirely on third-party backends (S3,
//! WebDAV, etc.).  All backends are first-class; none is "primary".
//!
//! # Available backends
//!
//! | Backend | Driver constant | Notes |
//! |---------|----------------|-------|
//! | [`LocalFsBackend`] | `local_fs` | Useful for self-hosted / single-node; not required |
//! | [`S3Backend`] | `s3_compatible` | AWS S3, MinIO, R2, B2, Aliyun OSS (S3-mode) … |
//! | [`WebDavBackend`] | `webdav` | Nextcloud, ownCloud, any RFC 4918 server |
//!
//! [`StorageManager`] orchestrates multi-backend replication and read-path
//! fallback across whichever backends the operator has configured.
//!
//! # Missing pieces (TODO)
//!
//! - `TODO(upload-service)`: an upload service that chains
//!   `FastCdcChunker` → `StorageManager::put_with_policy` → persists
//!   `file_chunks` + `chunk_locations` rows.
//! - `TODO(download-service)`: a download service that reads `file_chunks`,
//!   looks up `chunk_locations`, calls `StorageManager::get_from_any`, and
//!   reassembles the byte stream.
//! - `TODO(backend-repository)`: ORM entity + CRUD repository for
//!   `BackendConfig` / `storage_backend_configs` table.
//! - `TODO(put-with-policy)`: `StorageManager::put_with_policy` that
//!   executes a `ReplicationPolicy` (All / MinN / Specific) instead of
//!   requiring the caller to choose `put_to_all` vs `put_to` manually.

pub mod chunking;
pub mod hashing;
pub mod local;
pub mod manager;
pub mod s3;
pub mod webdav;

// Convenience re-exports
pub use local::LocalFsBackend;
pub use manager::StorageManager;
pub use s3::S3Backend;
pub use webdav::WebDavBackend;
