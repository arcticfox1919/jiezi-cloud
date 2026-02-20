//! Database durability, hot-backup, and integrity-checking services.
//!
//! # Five-layer durability model
//!
//! | Layer | Mechanism | Protects against |
//! |-------|-----------|-----------------|
//! | 1 | SQLite WAL + `synchronous=FULL` | Power cut / OS crash mid-write |
//! | 2 | Application two-phase commit | Partial upload leaving DB inconsistent |
//! | 3 | [`startup::run`] fast check | Corruption detected before serving traffic |
//! | 4 | [`backup::start_background_task`] | Disk hardware failure, deleted DB file |
//! | 5 | Background chunk patrol (Phase 5) | Silent bit-rot in stored objects |
//!
//! ## Layer 1 — SQLite durability pragmas
//!
//! Before the server opens the database pool, it MUST apply these pragmas.
//! The recommended call site is immediately after `sea_orm::Database::connect`.
//!
//! ```text
//! PRAGMA journal_mode = WAL;      -- Crash-safe; power cut cannot corrupt the DB.
//! PRAGMA synchronous  = FULL;     -- fsync before every commit returns.
//! PRAGMA foreign_keys = ON;       -- Enforce referential integrity at DB level.
//! PRAGMA busy_timeout = 5000;     -- Wait up to 5 s if DB file is locked.
//! ```
//!
//! These are **not** set automatically by this crate because SeaORM's connection
//! pool does not expose a post-connect hook in a stable API.  Wire them up in
//! `jiezi-cloud-server/src/main.rs` (Phase 5) via a raw `execute` call.
//!
//! ## Layer 4 — Backup service
//!
//! Uses SQLite's `VACUUM INTO 'path'` command (SQLite ≥ 3.27, 2019-02-08) to
//! create a consistent, compacted snapshot of the live database without
//! blocking active readers or writers.  The snapshot is a standard SQLite file.
//!
//! ```rust,no_run
//! use jiezi_cloud_integrity::backup;
//! # use sea_orm::DatabaseConnection;
//! # use jiezi_cloud_config::DatabaseBackupConfig;
//! # use std::path::PathBuf;
//! # async fn example(db: DatabaseConnection, cfg: DatabaseBackupConfig) {
//! // One-shot backup:
//! let path = backup::run_once(&db, &cfg).await.expect("backup failed");
//!
//! // Spawn background task (runs forever):
//! let _handle = backup::start_background_task(db, cfg);
//! # }
//! ```

pub mod backup;
pub mod startup;

/// TODO(Phase 5): Background chunk integrity patrol.
///
/// This module will implement a low-priority async task that continuously
/// walks `data/objects/` and recomputes SHA-256 for every stored chunk,
/// comparing against the filename (which IS the expected hash).
///
/// Activation plan:
/// 1. Add `patrol::start_background_task(storage_root, db)` call in `main.rs`
///    after the connection pool is established.
/// 2. The task records any mismatches in a new `corrupt_chunks` DB table and
///    emits `tracing::error!` events for immediate operator alerting.
/// 3. If a multi-backend replication policy is configured, trigger automatic
///    re-fetch from a healthy replica for the corrupted chunk key.
/// 4. Expose a `/api/v1/admin/integrity` endpoint (read-only) so operators
///    can query patrol status without grepping logs.
pub mod patrol;
