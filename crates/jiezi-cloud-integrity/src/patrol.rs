//! Background chunk integrity patrol — Phase 5.
//!
//! This module is a **planned stub**.  The implementation is tracked as
//! TODO(Phase 5 — step 6) in `jiezi-cloud-server/src/main.rs`.
//!
//! # Design
//!
//! The patrol is a low-priority async task that walks `storage.local_root` and
//! recomputes SHA-256 for every stored chunk file, comparing the result against
//! the filename (which IS the expected hash under content-addressed storage).
//!
//! ```text
//! for each file in data/objects/**:
//!     actual_hash  = sha256(file_contents)
//!     expected_key = file_stem              // e.g. "ab/abcdef1234…"
//!     if actual_hash != expected_key:
//!         emit tracing::error!
//!         insert into corrupt_chunks table
//!         if multi-backend replication is configured → re-fetch from replica
//! ```
//!
//! # Why it matters
//!
//! Layers 1–4 (WAL, two-phase commit, startup check, hot-backup) protect
//! against *sudden* failures.  Patrol addresses a different threat: **silent
//! bit-rot** — gradual hardware-induced corruption that accumulates over months
//! while all backups quietly archive the damaged data.  Without periodic
//! verification, a corrupted chunk may go undetected until a user downloads the
//! file and notices garbled output.
//!
//! # Tuning parameters (to be added to `StorageConfig` in Phase 5)
//!
//! | Parameter | Suggested default | Purpose |
//! |-----------|------------------|---------|
//! | `patrol_concurrency` | 2 | Parallel hash tasks (keep I/O gentle) |
//! | `patrol_sleep_ms` | 50 | Sleep between files (reduce I/O pressure) |
//! | `patrol_full_cycle_hours` | 168 (1 week) | Target time to scan entire object store |

// TODO(Phase 5 — step 6a): Add `corrupt_chunks` migration.
//   Table schema:
//     id           UUID PRIMARY KEY
//     chunk_key    TEXT NOT NULL   -- the content-addressed key (also the path)
//     detected_at  TIMESTAMPTZ NOT NULL
//     resolved_at  TIMESTAMPTZ     -- NULL until repaired
//     backend_id   TEXT NOT NULL   -- which storage backend reported this

// TODO(Phase 5 — step 6b): Implement `start_background_task`.
//   Signature:
//     pub fn start_background_task(
//         storage_root: PathBuf,
//         db: DatabaseConnection,
//     ) -> tokio::task::JoinHandle<()>
//   The task should use `tokio::fs::read_dir` + a semaphore for concurrency
//   control, and yield between files with `tokio::time::sleep`.

// TODO(Phase 5 — step 6c): Implement `verify_catalog_consistency`.
//   Cross-check that every (file_id, chunk_key) row in the `file_chunks` DB
//   table has a corresponding file on disk.  Run this at startup (called from
//   `startup::run`) so that incomplete restores are caught immediately.
//   Signature:
//     pub async fn verify_catalog_consistency(
//         storage_root: &Path,
//         db: &DatabaseConnection,
//     ) -> Vec<String>   // list of missing chunk keys

// TODO(Phase 5 — step 6d): Expose /api/v1/admin/integrity HTTP endpoint.
//   Returns JSON: { last_patrol_at, chunks_scanned, corrupt_count, pending_repairs }
//   Requires admin role (RBAC Owner or Admin).
