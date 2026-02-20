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
pub mod m20260220_000003_create_spaces;
pub mod m20260220_000004_create_file_nodes;
pub mod m20260220_000005_create_file_node_paths;
pub mod m20260220_000006_create_storage_backend_configs;
pub mod m20260220_000007_create_chunk_locations;
pub mod m20260220_000008_create_file_chunks;

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
            Box::new(m20260220_000003_create_spaces::Migration),
            Box::new(m20260220_000004_create_file_nodes::Migration),
            Box::new(m20260220_000005_create_file_node_paths::Migration),
            Box::new(m20260220_000006_create_storage_backend_configs::Migration),
            Box::new(m20260220_000007_create_chunk_locations::Migration),
            Box::new(m20260220_000008_create_file_chunks::Migration),
        ]
    }
}
