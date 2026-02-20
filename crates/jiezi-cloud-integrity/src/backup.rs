//! Automatic database hot-backup for all three supported backends.
//!
//! | Backend    | Mechanism                              | Output format                       |
//! |------------|----------------------------------------|-------------------------------------|
//! | SQLite     | `VACUUM INTO 'path'` (SQLite ≥ 3.27)  | `.db` — standard SQLite file        |
//! | PostgreSQL | `pg_dump --format=custom` subprocess   | `.pgdump` — compressed binary       |
//! | MySQL      | `mysqldump --single-transaction` subprocess | `.sql` — plain SQL dump        |
//!
//! # SQLite — `VACUUM INTO`
//!
//! Built into SQLite; no external tools required.  Creates a consistent,
//! compacted snapshot of the live database **without blocking writers**.
//! The output file is a plain SQLite database — open it with any SQLite tool.
//!
//! # PostgreSQL — `pg_dump`
//!
//! Spawns `pg_dump` (must be on `PATH`) with `--format=custom` (compressed
//! binary).  The full connection URI is passed directly via `--dbname=<uri>` —
//! no credential parsing is done in Rust.
//!
//! Restore: `pg_restore --dbname=<uri> jiezi-meta-<ts>.pgdump`
//!
//! Install: `sudo apt install postgresql-client` (Debian/Ubuntu) or
//!          `brew install libpq` (macOS)
//!
//! # MySQL / MariaDB — `mysqldump`
//!
//! Spawns `mysqldump --single-transaction` (must be on `PATH`).  The flag
//! starts an InnoDB consistent read snapshot without locking any tables.
//! Credentials are extracted from the MySQL URI.
//!
//! Restore: `mysql -u<user> -p<pass> -h<host> <db> < jiezi-meta-<ts>.sql`
//!
//! Install: `sudo apt install mysql-client` (Debian/Ubuntu) or
//!          `brew install mysql-client` (macOS)
//!
//! # Backup rotation
//!
//! Files are named `jiezi-meta-YYYY-MM-DDTHH-MM-SS.{ext}`.
//! ISO-8601 timestamps sort lexicographically == chronologically.
//! After each run the oldest files (same extension) are deleted, keeping only
//! the newest `keep_count` entries.
//!
//! # Production recommendation
//!
//! Point `database.backup.dir` at a **different physical device** than the
//! primary database file (e.g. USB drive, NAS, or network share).
//! Override via: `JIEZI__DATABASE__BACKUP__DIR=/mnt/nas/jiezi-db`

use std::path::{Path, PathBuf};
use std::process::Stdio;

use chrono::Utc;
use jiezi_cloud_config::DatabaseConfig;
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement};
use thiserror::Error;
use tokio::time::{Duration, interval};
use tracing::{error, info, warn};

// ── Error type ────────────────────────────────────────────────────────────────────

#[derive(Debug, Error)]
pub enum BackupError {
    #[error("backup path is invalid: {0}")]
    InvalidPath(String),

    #[error("cannot parse database URL: {0}")]
    InvalidUrl(String),

    #[error("database error during VACUUM INTO: {0}")]
    Db(#[from] sea_orm::DbErr),

    #[error("I/O error during backup: {0}")]
    Io(#[from] std::io::Error),

    /// Returned when `pg_dump` / `mysqldump` is not found on PATH, or exits
    /// with a non-zero status code.
    #[error("external backup tool '{tool}' failed: {error}")]
    ExternalTool { tool: String, error: String },
}

// ── Public API ────────────────────────────────────────────────────────────────────

/// Run one backup immediately and return the path of the created file.
///
/// Dispatches to the backend-specific implementation based on the active
/// database driver:
///
/// - SQLite     → `VACUUM INTO`    → `jiezi-meta-<ts>.db`
/// - PostgreSQL → `pg_dump`        → `jiezi-meta-<ts>.pgdump`
/// - MySQL      → `mysqldump`      → `jiezi-meta-<ts>.sql`
///
/// The backup directory is created automatically if it does not exist.
///
/// # Errors
///
/// - [`BackupError::ExternalTool`] if `pg_dump` / `mysqldump` is not on `PATH`
///   or exits with a non-zero status.
/// - [`BackupError::InvalidUrl`] if the MySQL connection URL cannot be parsed.
/// - [`BackupError::Io`] on filesystem errors.
/// - [`BackupError::Db`] on SQLite `VACUUM INTO` failure.
pub async fn run_once(
    db: &DatabaseConnection,
    db_config: &DatabaseConfig,
) -> Result<PathBuf, BackupError> {
    let config = &db_config.backup;
    let dir = ensure_dir(&config.dir)?;
    let timestamp = Utc::now().format("%Y-%m-%dT%H-%M-%S").to_string();

    match db.get_database_backend() {
        DbBackend::Sqlite => {
            let path = dir.join(format!("jiezi-meta-{timestamp}.db"));
            backup_sqlite(db, &path).await?;
            info!(path = %path.display(), "SQLite VACUUM INTO backup completed");
            rotate_old_backups(&dir, config.keep_count, "db")?;
            Ok(path)
        }
        DbBackend::Postgres => {
            let path = dir.join(format!("jiezi-meta-{timestamp}.pgdump"));
            backup_postgres(&db_config.url, &path).await?;
            info!(path = %path.display(), "PostgreSQL pg_dump backup completed");
            rotate_old_backups(&dir, config.keep_count, "pgdump")?;
            Ok(path)
        }
        DbBackend::MySql => {
            let path = dir.join(format!("jiezi-meta-{timestamp}.sql"));
            backup_mysql(&db_config.url, &path).await?;
            info!(path = %path.display(), "MySQL mysqldump backup completed");
            rotate_old_backups(&dir, config.keep_count, "sql")?;
            Ok(path)
        }
    }
}

/// Spawn a background Tokio task that calls [`run_once`] on the configured
/// interval.
///
/// The task runs until the process exits.  The returned `JoinHandle` can be
/// stored (to abort the task) or simply dropped.
///
/// # Usage in `main.rs` (Phase 5 — step 5)
///
/// ```rust,no_run
/// # use sea_orm::DatabaseConnection;
/// # use jiezi_cloud_config::DatabaseConfig;
/// # async fn example(db: DatabaseConnection, db_config: DatabaseConfig) {
/// if db_config.backup.enabled {
///     jiezi_cloud_integrity::backup::start_background_task(db, db_config);
/// }
/// # }
/// ```
pub fn start_background_task(
    db: DatabaseConnection,
    db_config: DatabaseConfig,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let period = Duration::from_secs(db_config.backup.interval_minutes * 60);
        let mut ticker = interval(period);
        // Skip the first tick so we don't backup at t=0 (DB just opened).
        ticker.tick().await;

        loop {
            ticker.tick().await;
            match run_once(&db, &db_config).await {
                Ok(path) => info!(path = %path.display(), "scheduled backup ok"),
                Err(e) => error!(error = %e, "scheduled backup FAILED — investigate immediately!"),
            }
        }
    })
}

// ── Backend implementations ────────────────────────────────────────────────────────────

/// SQLite: `VACUUM INTO path` — built-in, no external tool needed.
async fn backup_sqlite(db: &DatabaseConnection, backup_path: &Path) -> Result<(), BackupError> {
    let path_str = backup_path.to_string_lossy();
    // Single-quote characters cannot appear in the SQL literal.
    if path_str.contains('\'') {
        return Err(BackupError::InvalidPath(
            "backup path must not contain a single quote character (')".into(),
        ));
    }
    // Forward slashes work on all platforms inside SQLite's VFS layer.
    let sql = format!("VACUUM INTO '{}'", path_str.replace('\\', "/"));
    db.execute(Statement::from_string(DbBackend::Sqlite, sql)).await?;
    Ok(())
}

/// PostgreSQL: spawn `pg_dump --format=custom`.
///
/// `pg_dump` natively accepts PostgreSQL URIs via `--dbname=<uri>`.
/// No credential parsing is performed in Rust — the URI is forwarded verbatim.
///
/// Prerequisites:
/// - `pg_dump` must be installed and on the system `PATH`.
/// - The version of `pg_dump` should match the server's major version.
/// - If the URL uses a password, ensure it is in the URL or in `~/.pgpass`.
async fn backup_postgres(db_url: &str, backup_path: &Path) -> Result<(), BackupError> {
    let status = tokio::process::Command::new("pg_dump")
        .arg("--format=custom")           // compressed binary; smaller than plain SQL
        .arg("--no-password")             // password must be in the URL or ~/.pgpass
        .arg(format!("--dbname={db_url}"))
        .arg(format!("--file={}", backup_path.display()))
        .status()
        .await
        .map_err(|e| BackupError::ExternalTool {
            tool: "pg_dump".into(),
            error: format!(
                "{e} — ensure 'pg_dump' is installed and on PATH \
                 (Debian/Ubuntu: postgresql-client, macOS: brew install libpq)"
            ),
        })?;

    if !status.success() {
        return Err(BackupError::ExternalTool {
            tool: "pg_dump".into(),
            error: format!("exited with status {status}"),
        });
    }
    Ok(())
}

/// MySQL / MariaDB: spawn `mysqldump --single-transaction`.
///
/// `--single-transaction` opens an InnoDB consistent read snapshot so no
/// tables are locked during the dump.  This flag is ignored for MyISAM tables
/// (rare in modern setups); for those, a brief read lock is unavoidable.
///
/// Prerequisites:
/// - `mysqldump` must be installed and on the system `PATH`.
/// - The MySQL user must have SELECT, SHOW VIEW, TRIGGER, LOCK TABLES privileges.
async fn backup_mysql(db_url: &str, backup_path: &Path) -> Result<(), BackupError> {
    // Parse mysql://user:pass@host:port/dbname
    let parsed = ::url::Url::parse(db_url)
        .map_err(|e| BackupError::InvalidUrl(e.to_string()))?;

    let host = parsed.host_str().unwrap_or("127.0.0.1");
    let port = parsed.port().unwrap_or(3306);
    let user = parsed.username();
    let password = parsed.password().unwrap_or("");
    let dbname = parsed.path().trim_start_matches('/');

    if dbname.is_empty() {
        return Err(BackupError::InvalidUrl(
            "MySQL URL must include a database name: mysql://user:pass@host/dbname".into(),
        ));
    }

    // Redirect mysqldump stdout directly to the backup file.
    let output_file = std::fs::File::create(backup_path)?;

    let status = tokio::process::Command::new("mysqldump")
        .arg("--single-transaction")     // InnoDB consistent snapshot, no table locks
        .arg("--routines")               // include stored procedures / functions
        .arg("--triggers")               // include triggers
        .arg("--set-gtid-purged=OFF")    // avoid GTID errors in replication setups
        .arg(format!("--host={host}"))
        .arg(format!("--port={port}"))
        .arg(format!("--user={user}"))
        .arg(format!("--password={password}"))
        .arg(dbname)
        .stdout(Stdio::from(output_file))
        .status()
        .await
        .map_err(|e| BackupError::ExternalTool {
            tool: "mysqldump".into(),
            error: format!(
                "{e} — ensure 'mysqldump' is installed and on PATH \
                 (Debian/Ubuntu: mysql-client, macOS: brew install mysql-client)"
            ),
        })?;

    if !status.success() {
        return Err(BackupError::ExternalTool {
            tool: "mysqldump".into(),
            error: format!("exited with status {status}"),
        });
    }
    Ok(())
}

// ── Helpers ────────────────────────────────────────────────────────────────────

/// Resolve the backup directory (relative → absolute) and create it if needed.
fn ensure_dir(dir: &Path) -> Result<PathBuf, BackupError> {
    let abs = if dir.is_absolute() {
        dir.to_owned()
    } else {
        std::env::current_dir()?.join(dir)
    };
    std::fs::create_dir_all(&abs)?;
    Ok(abs)
}

/// Delete the oldest backup files of the given `extension`, keeping only the
/// newest `keep_count`.  Only files matching `jiezi-meta-*.<extension>` are
/// touched, so unrelated files placed in the same directory are never deleted.
fn rotate_old_backups(dir: &Path, keep_count: usize, extension: &str) -> Result<(), BackupError> {
    let mut backups: Vec<PathBuf> = std::fs::read_dir(dir)?
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|p| {
            p.extension().is_some_and(|ext| ext == extension)
                && p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with("jiezi-meta-"))
        })
        .collect();

    // ISO-8601 filenames sort lexicographically == chronologically.
    backups.sort();

    if backups.len() > keep_count {
        let to_delete = backups.len() - keep_count;
        for old in backups.iter().take(to_delete) {
            match std::fs::remove_file(old) {
                Ok(()) => info!(path = %old.display(), "rotated old backup"),
                Err(e) => warn!(path = %old.display(), error = %e, "failed to delete old backup"),
            }
        }
    }
    Ok(())
}
