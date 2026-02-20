//! Database schema migrations for Jiezi Cloud.
//!
//! Uses `sea-orm-migration` so that the same Rust migration code generates
//! correct DDL for **SQLite**, **PostgreSQL** and **MySQL**.  The database
//! backend is chosen at *compile time* via Cargo feature flags:
//!
//! | Feature        | Backend    | When to use                       |
//! |----------------|------------|-----------------------------------|
//! | `db-sqlite`    | SQLite     | Development (default)             |
//! | `db-postgres`  | PostgreSQL | Production (recommended)          |
//! | `db-mysql`     | MySQL      | Production (alternative)          |
//!
//! # Running migrations
//!
//! ```rust,no_run
//! use jiezi_cloud_migration::Migrator;
//! use sea_orm_migration::MigratorTrait;
//!
//! // `db` is a `sea_orm::DatabaseConnection` obtained at startup.
//! // Migrator::up(&db, None).await?;
//! ```

pub mod m20260220_000001_create_users;
pub mod m20260220_000002_create_refresh_tokens;

use sea_orm_migration::prelude::*;

/// The single entry point used by the server binary to apply all pending
/// schema migrations at startup.
pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![
            Box::new(m20260220_000001_create_users::Migration),
            Box::new(m20260220_000002_create_refresh_tokens::Migration),
        ]
    }
}
