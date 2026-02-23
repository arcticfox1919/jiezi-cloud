//! # jiezi-cloud-core
//!
//! The foundational vocabulary of the Jiezi Cloud platform.
//!
//! This crate contains **zero business logic** and **zero I/O**. It defines:
//!
//! - [`error`] — unified error type [`AppError`] and result alias [`AppResult`].
//! - [`types`] — newtype domain IDs, pagination helpers and primitive value types.
//! - [`models`] — pure data structures for users, files, chunks, spaces, shares
//!   and background tasks.
//! - [`traits`] — async trait interfaces that form the contracts between all
//!   other crates in the workspace.
//! - [`events`] — domain events published over the internal in-process event bus.
//!
//! # Dependency policy
//!
//! Every other crate in the workspace depends on `jiezi-cloud-core`.
//! `jiezi-cloud-core` itself has **no dependency on any other workspace crate**.
//!
//! Business crates (`jiezi-cloud-auth`, `jiezi-cloud-vfs`, …) MUST NOT depend
//! on each other directly.  Cross-module communication happens exclusively via
//! the trait interfaces and events defined here.

pub mod error;
pub mod events;
pub mod models;
pub mod protocol;
pub mod traits;
pub mod types;

// ─── Top-level convenience re-exports ────────────────────────────────────────

pub use error::{AppError, AppResult};
pub use types::{
    Action, BackendId, ChunkId, FileId, HealthStatus, KbNoteId, PageRequest, PageResponse,
    ResourceRef, ShareId, SpaceId, TaskId, UserId,
};
