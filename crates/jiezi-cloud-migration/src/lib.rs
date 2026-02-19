//! Database migration management for Jiezi Cloud.
//!
//! Wraps `sqlx` migrate facilities and provides a unified entry point for
//! applying migrations against SQLite (development) and PostgreSQL (production).
