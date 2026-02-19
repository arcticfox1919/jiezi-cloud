//! Multi-device synchronization module for Jiezi Cloud.
//!
//! Implements incremental delta sync with CRDT-based conflict resolution.

pub mod conflict;
pub mod delta;
pub mod engine;
