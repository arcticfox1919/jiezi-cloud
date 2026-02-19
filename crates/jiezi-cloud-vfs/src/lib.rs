//! Virtual file system module for Jiezi Cloud.
//!
//! Implements [`jiezi_cloud_core::traits::vfs::VfsService`] backed by SQLite/
//! PostgreSQL using a closure table for efficient directory tree traversal.

pub mod metadata;
pub mod repository;
pub mod service;
pub mod trash;
pub mod tree;
