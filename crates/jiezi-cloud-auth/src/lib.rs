//! Authentication and authorization module for Jiezi Cloud.
//!
//! Implements [`jiezi_cloud_core::traits::auth::AuthService`] using
//! Argon2 password hashing, JWT token management and an RBAC permission engine.
//!
//! # Database backends
//!
//! Select a backend at **compile time** via Cargo feature flags:
//!
//! | Cargo feature | Database  |
//! |---------------|-----------|
//! | `db-sqlite`   | SQLite 3  |
//! | `db-postgres` | PostgreSQL|
//! | `db-mysql`    | MySQL     |
//!
//! The `db-sqlite` feature is enabled by default for local development.
//! Pass a connection URL at runtime via `sea_orm::Database::connect`.

pub mod entities;
pub mod jwt;
pub mod password;
pub mod rbac;
pub mod repository;
pub mod service;

// ─── Public re-exports ────────────────────────────────────────────────────────

pub use service::AuthServiceImpl;
