//! Trait interfaces that define the contract between Jiezi Cloud modules.
//!
//! # Architecture
//!
//! All inter-module communication goes through these traits.  This means:
//!
//! - Modules can be tested in isolation with mock implementations.
//! - The concrete implementations live in separate crates and are never
//!   directly referenced by callers.
//! - In the future, any trait can be backed by an RPC call instead of a local
//!   function call without changing callers.

pub mod auth;
pub mod cache;
pub mod search;
pub mod storage;
pub mod vfs;
