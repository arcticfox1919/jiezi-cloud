//! Fast startup consistency check executed before the server accepts traffic.
//!
//! # What it checks
//!
//! ## SQLite
//!
//! | Check | Statement | What it detects |
//! |-------|-----------|----------------|
//! | Page scan | `PRAGMA quick_check(100)` | B-tree corruption, free-list errors (first 100 pages) |
//! | Journal mode | `PRAGMA journal_mode` | Warn if not WAL |
//! | Synchronous | `PRAGMA synchronous` | Warn if below FULL (2) |
//!
//! `PRAGMA quick_check(N)` inspects only the first N pages, so it completes
//! in milliseconds even on large databases.  For a full scan (slow) use
//! `PRAGMA integrity_check` out-of-band during maintenance windows.
//!
//! ## PostgreSQL
//!
//! | Check | Statement | What it detects |
//! |-------|-----------|----------------|
//! | Connectivity | `SELECT version()` | Connection pool works, logs server version |
//! | Active connections | `pg_stat_activity` | Warn if near `max_connections` |
//!
//! ## MySQL / MariaDB
//!
//! | Check | Statement | What it detects |
//! |-------|-----------|----------------|
//! | Connectivity | `SELECT VERSION()` | Connection pool works, logs server version |
//! | Thread count | `SHOW GLOBAL STATUS LIKE 'Threads_connected'` | Warn if high |
//!
//! # Usage
//!
//! ```rust,no_run
//! # use sea_orm::DatabaseConnection;
//! # async fn example(db: DatabaseConnection) {
//! let result = jiezi_cloud_integrity::startup::run(&db).await;
//! for w in &result.warnings { tracing::warn!("{w}"); }
//! if !result.is_ok() {
//!     for e in &result.errors { tracing::error!("{e}"); }
//!     std::process::exit(1);
//! }
//! # }
//! ```

use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement};
use tracing::info;

// ── Public types ──────────────────────────────────────────────────────────────

/// The outcome of the startup consistency check.
#[derive(Debug)]
pub struct CheckResult {
    /// `true` if no errors were found (warnings are non-fatal).
    pub passed: bool,
    /// Non-fatal observations.  Always log these; do not abort on them.
    pub warnings: Vec<String>,
    /// Fatal problems.  Abort startup if any are present.
    pub errors: Vec<String>,
}

impl CheckResult {
    /// Returns `true` when the database is safe to use (no errors).
    pub fn is_ok(&self) -> bool {
        self.passed && self.errors.is_empty()
    }
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Run the startup consistency check against the live database connection.
///
/// This function never panics.  All database errors are captured and returned
/// as entries in [`CheckResult::errors`].
pub async fn run(db: &DatabaseConnection) -> CheckResult {
    let mut warnings: Vec<String> = Vec::new();
    let mut errors: Vec<String> = Vec::new();

    match db.get_database_backend() {
        DbBackend::Sqlite => {
            check_sqlite_integrity(db, &mut errors).await;
            check_sqlite_journal_mode(db, &mut warnings).await;
            check_sqlite_synchronous(db, &mut warnings).await;
        }
        DbBackend::Postgres => {
            check_postgres_version(db, &mut errors).await;
            check_postgres_connections(db, &mut warnings).await;
        }
        DbBackend::MySql => {
            check_mysql_version(db, &mut errors).await;
            check_mysql_threads(db, &mut warnings).await;
        }
    }

    // TODO(Phase 5 — step 6): After the chunk patrol module is implemented,
    // call patrol::verify_catalog_consistency(&db) here to cross-check that
    // every chunk referenced in `file_chunks` actually exists on disk.  This
    // detects catalog/filesystem divergence caused by partial restores or
    // manual file deletions.  Run it in a background task (not inline here)
    // to avoid delaying startup on large datasets.

    let passed = errors.is_empty();
    CheckResult { passed, warnings, errors }
}

// ── PostgreSQL checks ──────────────────────────────────────────────────────────────

/// `SELECT version()` — verify connectivity and log server version.
async fn check_postgres_version(db: &DatabaseConnection, errors: &mut Vec<String>) {
    let stmt = Statement::from_string(
        DbBackend::Postgres,
        "SELECT version() AS ver".to_owned(),
    );
    match db.query_one(stmt).await {
        Ok(Some(row)) => {
            if let Ok(ver) = row.try_get::<String>("", "ver") {
                info!(version = %ver, "PostgreSQL server version");
            }
        }
        Ok(None) => errors.push("SELECT version() returned no rows".to_owned()),
        Err(e) => errors.push(format!("PostgreSQL connectivity check failed: {e}")),
    }
}

/// Query `pg_stat_activity` — warn if active connection count is high.
///
/// A connection count above ~80 % of `max_connections` (default 100) means the
/// pool is under pressure.  This is a warning, not an error.
async fn check_postgres_connections(db: &DatabaseConnection, warnings: &mut Vec<String>) {
    let stmt = Statement::from_string(
        DbBackend::Postgres,
        "SELECT count(*) AS cnt FROM pg_stat_activity WHERE state = 'active'".to_owned(),
    );
    match db.query_one(stmt).await {
        Ok(Some(row)) => {
            if let Ok(cnt) = row.try_get::<i64>("", "cnt") {
                info!(active_connections = cnt, "PostgreSQL active connection count");
                if cnt > 80 {
                    warnings.push(format!(
                        "PostgreSQL has {cnt} active connections. \
                         If max_connections is 100 (default), the pool is near capacity. \
                         Consider increasing max_connections or adding a connection pooler \
                         (e.g. PgBouncer)."
                    ));
                }
            }
        }
        Ok(None) | Err(_) => {
            // Non-fatal; pg_stat_activity may require elevated privileges.
        }
    }
}

// ── MySQL checks ──────────────────────────────────────────────────────────────────

/// `SELECT VERSION()` — verify connectivity and log server version.
async fn check_mysql_version(db: &DatabaseConnection, errors: &mut Vec<String>) {
    let stmt = Statement::from_string(
        DbBackend::MySql,
        "SELECT VERSION() AS ver".to_owned(),
    );
    match db.query_one(stmt).await {
        Ok(Some(row)) => {
            if let Ok(ver) = row.try_get::<String>("", "ver") {
                info!(version = %ver, "MySQL server version");
            }
        }
        Ok(None) => errors.push("SELECT VERSION() returned no rows".to_owned()),
        Err(e) => errors.push(format!("MySQL connectivity check failed: {e}")),
    }
}

/// `SHOW GLOBAL STATUS LIKE 'Threads_connected'` — warn if thread count is high.
async fn check_mysql_threads(db: &DatabaseConnection, warnings: &mut Vec<String>) {
    let stmt = Statement::from_string(
        DbBackend::MySql,
        "SHOW GLOBAL STATUS LIKE 'Threads_connected'".to_owned(),
    );
    match db.query_one(stmt).await {
        Ok(Some(row)) => {
            // Result columns are 'Variable_name' and 'Value' (both strings).
            if let Ok(val) = row.try_get::<String>("", "Value") {
                if let Ok(count) = val.parse::<i64>() {
                    info!(threads_connected = count, "MySQL connected thread count");
                    if count > 100 {
                        warnings.push(format!(
                            "MySQL has {count} connected threads. \
                             Check max_connections setting and consider a connection pooler."
                        ));
                    }
                }
            }
        }
        Ok(None) | Err(_) => {}
    }
}

// ── SQLite checks ─────────────────────────────────────────────────────────────

/// `PRAGMA quick_check(100)` — fast page-level corruption scan.
async fn check_sqlite_integrity(db: &DatabaseConnection, errors: &mut Vec<String>) {
    let stmt = Statement::from_string(
        DbBackend::Sqlite,
        // Check the first 100 pages; enough to catch common corruption early.
        // For a full scan (slow on large DBs) use PRAGMA integrity_check.
        "PRAGMA quick_check(100)".to_owned(),
    );

    match db.query_all(stmt).await {
        Ok(rows) => {
            let issues: Vec<String> = rows
                .into_iter()
                .filter_map(|row| {
                    row.try_get::<String>("", "integrity_check").ok()
                })
                .filter(|v| v != "ok")
                .collect();

            if issues.is_empty() {
                info!("SQLite quick_check: ok");
            } else {
                for issue in issues {
                    errors.push(format!("SQLite quick_check: {issue}"));
                }
            }
        }
        Err(e) => {
            errors.push(format!("SQLite quick_check failed to execute: {e}"));
        }
    }
}

/// `PRAGMA journal_mode` — warn if not WAL.
async fn check_sqlite_journal_mode(db: &DatabaseConnection, warnings: &mut Vec<String>) {
    let stmt = Statement::from_string(DbBackend::Sqlite, "PRAGMA journal_mode".to_owned());
    match db.query_one(stmt).await {
        Ok(Some(row)) => {
            if let Ok(mode) = row.try_get::<String>("", "journal_mode") {
                if mode == "wal" {
                    info!("SQLite journal_mode: wal (crash-safe)");
                } else {
                    warnings.push(format!(
                        "SQLite journal_mode is '{mode}', not 'wal'. \
                         Set 'PRAGMA journal_mode=WAL' after opening the connection \
                         pool to protect against mid-write corruption on power failure."
                    ));
                }
            }
        }
        Ok(None) => warnings.push("PRAGMA journal_mode returned no rows".to_owned()),
        Err(e) => warnings.push(format!("could not read journal_mode: {e}")),
    }
}

/// `PRAGMA synchronous` — warn if below FULL (2).
///
/// | Value | Meaning |
/// |-------|---------|
/// | 0 OFF | No fsync — fastest, but data can be lost on OS crash |
/// | 1 NORMAL | fsync at critical moments (safe with WAL) |
/// | 2 FULL | fsync on every commit — safest, ~10 % slower |
/// | 3 EXTRA | fsync on more operations — not needed with WAL |
async fn check_sqlite_synchronous(db: &DatabaseConnection, warnings: &mut Vec<String>) {
    let stmt = Statement::from_string(DbBackend::Sqlite, "PRAGMA synchronous".to_owned());
    match db.query_one(stmt).await {
        Ok(Some(row)) => {
            // SQLite returns synchronous as an integer.
            if let Ok(level) = row.try_get::<i32>("", "synchronous") {
                match level {
                    2 | 3 => info!("SQLite synchronous: {} (full fsync, safe)", level),
                    1 => warnings.push(
                        "SQLite synchronous=NORMAL (1). Consider PRAGMA synchronous=FULL \
                         to guarantee commit durability on all platforms."
                            .to_owned(),
                    ),
                    _ => warnings.push(format!(
                        "SQLite synchronous={level} (OFF). This risks data loss on OS crash. \
                         Set PRAGMA synchronous=FULL after opening the pool."
                    )),
                }
            }
        }
        Ok(None) => {}
        Err(e) => warnings.push(format!("could not read synchronous pragma: {e}")),
    }
}
