//! Storage backends and chunking utilities for Jiezi Cloud.
//!
//! Implements [`jiezi_cloud_core::traits::storage::StorageBackend`] for:
//!
//! - **Local filesystem** (`local`) — atomic writes, read-time SHA-256 integrity.
//! - **S3-compatible** (`s3`) — AWS S3, MinIO, Cloudflare R2, Backblaze B2 …
//! - **WebDAV** (`webdav`) — Nextcloud, ownCloud, any RFC 4918 server.
//!
//! The [`manager`] module provides [`StorageManager`] which orchestrates
//! multi-backend replication and read-path fallback.

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
