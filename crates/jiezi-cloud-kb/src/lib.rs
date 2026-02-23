//! # jiezi-cloud-kb
//!
//! Knowledge-base layer for Jiezi Cloud.  This crate sits **above** the VFS
//! and storage layers and treats every `.md` file as a potential knowledge
//! entry.
//!
//! ## Responsibilities
//!
//! - Index Markdown files uploaded to the VFS into `kb_notes`.
//! - Parse `[[WikiLink]]` syntax and maintain the `kb_backlinks` directed graph.
//! - Re-index notes when their content changes (via domain events).
//! - Clean up metadata when VFS files are permanently deleted.
//! - Expose blog-publishing fields (`slug`, `published_at`) on the same record.
//!
//! ## What this crate does NOT do
//!
//! - It never stores Markdown bytes — those live in VFS + object storage.
//! - It does not know about HTTP routing or Actix-web.
//! - It does not own vector embeddings (that is `jiezi-cloud-ai`).
//!
//! ## Architecture boundary
//!
//! ```text
//! ┌─────────────────────────────────┐
//! │  jiezi-cloud-kb  (this crate)   │
//! │  depends on core *traits* only  │
//! ├─────────────────────────────────┤
//! │  jiezi-cloud-vfs  (VfsService)  │
//! │  jiezi-cloud-search             │
//! │  jiezi-cloud-storage            │  ← reached via trait objects, not directly
//! └─────────────────────────────────┘
//! ```

pub mod backlink;
pub mod consts;
pub mod entities;
pub mod indexer;
pub mod repository;
pub mod service;

pub use service::KbServiceImpl;
