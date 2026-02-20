//! Virtual file system module for Jiezi Cloud.
//!
//! Implements [`jiezi_cloud_core::traits::vfs::VfsService`] backed by SQLite /
//! PostgreSQL using a **closure table** for efficient directory-tree traversal.
//!
//! # Module layout
//!
//! | Module | Purpose |
//! |--------|---------|
//! | `entities` | SeaORM table models (used only at the repository boundary) |
//! | `metadata` | Conversions between DB models and `jiezi_cloud_core` domain types |
//! | `tree` | Closure-table insert / move / delete helpers |
//! | `repository` | All database I/O (`FileNodeRepository`) |
//! | `service` | Trait implementation (`VfsServiceImpl`) |
//! | `trash` | Reserved for future trash-expiry / accounting logic |

pub mod entities;
pub mod metadata;
pub mod repository;
pub mod service;
pub mod trash;
pub mod tree;

// ─── Public re-exports ────────────────────────────────────────────────────────

pub use repository::FileNodeRepository;
pub use service::VfsServiceImpl;
