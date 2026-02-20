//! Layered configuration loading for Jiezi Cloud.
//!
//! # Loading order (highest priority wins)
//!
//! 1. **Environment variables** — prefixed `JIEZI__`, double underscore for
//!    nesting (e.g. `JIEZI__AUTH__JWT_SECRET=...`).
//! 2. **`config/local.toml`** — machine-local overrides, not committed to git.
//! 3. **`config/{environment}.toml`** — environment-specific overrides
//!    (`development`, `production`, `test`).  Selected by the
//!    `JIEZI__ENVIRONMENT` env var (default: `development`).
//! 4. **`config/default.toml`** — base defaults committed to the repository.
//!
//! # Dev workflow
//!
//! Copy `.env.example` to `.env` and edit as needed.  The loader calls
//! `dotenvy::dotenv()` before reading environment variables, so variables in
//! `.env` are treated as if they were exported in the shell.  `.env` is
//! git-ignored.
//!
//! # Production deployment
//!
//! Set `JIEZI__ENVIRONMENT=production` and supply secrets via environment
//! variables (e.g. `JIEZI__AUTH__JWT_SECRET`, `JIEZI__DATABASE__URL`).
//! The `config/production.toml` contains only non-secret defaults.
//!
//! # Example
//!
//! ```rust,no_run
//! let cfg = jiezi_cloud_config::load().expect("failed to load config");
//! println!("Listening on {}:{}", cfg.server.host, cfg.server.port);
//! ```

use std::path::PathBuf;

use serde::Deserialize;
use thiserror::Error;

// ---- Error ------------------------------------------------------------------

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("configuration error: {0}")]
    Load(#[from] config::ConfigError),
}

// ---- Top-level --------------------------------------------------------------

/// Complete application configuration.
///
/// All fields are deserialized from the layered config sources; see the module
/// documentation for the loading order and environment variable naming.
#[derive(Debug, Clone, Deserialize)]
pub struct AppConfig {
    /// Current runtime environment.  Affects which `config/{env}.toml` file
    /// is loaded and may influence log verbosity defaults.
    pub environment: Environment,

    pub server:   ServerConfig,
    pub database: DatabaseConfig,
    pub auth:     AuthConfig,
    pub storage:  StorageConfig,
    pub tracing:  TracingConfig,
}

// ---- Environment ------------------------------------------------------------

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Environment {
    Development,
    Production,
    Test,
}

impl Environment {
    pub fn is_production(&self) -> bool {
        *self == Self::Production
    }
    pub fn is_development(&self) -> bool {
        *self == Self::Development
    }
}

// ---- Server -----------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
pub struct ServerConfig {
    /// Bind address (e.g. `"127.0.0.1"` for dev, `"0.0.0.0"` for prod).
    pub host: String,

    /// TCP port the HTTP server listens on.
    pub port: u16,

    /// Number of Actix-web worker threads.  `0` means use [`std::thread::available_parallelism`].
    pub workers: usize,
}

impl ServerConfig {
    /// Returns the full `host:port` string, ready for `HttpServer::bind`.
    pub fn bind_address(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }
}

// ---- Database ---------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
pub struct DatabaseConfig {
    /// Full connection URL.
    ///
    /// Examples:
    /// - SQLite:     `sqlite:./data/jiezi-dev.db?mode=rwc`
    /// - PostgreSQL: `postgres://user:pass@localhost/jiezi`
    /// - MySQL:      `mysql://user:pass@localhost/jiezi`
    pub url: String,

    /// Maximum number of connections in the pool.
    pub max_connections: u32,

    /// Minimum (idle) connections kept alive.
    pub min_connections: u32,

    /// Seconds to wait before a connection attempt times out.
    pub connect_timeout_seconds: u64,

    /// Automatic hot-backup settings (primarily for SQLite; see [`DatabaseBackupConfig`]).
    pub backup: DatabaseBackupConfig,
}

/// Automatic database backup configuration.
///
/// For **SQLite** the backup is performed via `VACUUM INTO` (SQLite 3.27+), which
/// creates a consistent, compacted snapshot of the live database without blocking
/// writers.  The backup file is a standard SQLite file that can be opened
/// directly with any SQLite tool.
///
/// For **PostgreSQL / MySQL** this setting is ignored — use `pg_dump` / `mysqldump`
/// managed by your database server or the OS scheduler instead.
///
/// # Durability note
///
/// Backups protect against **disk hardware failure** and **software corruption
/// that is detected too late**.  Against **power-cut-induced mid-write corruption**
/// the first line of defence is SQLite WAL mode (`PRAGMA journal_mode=WAL` +
/// `PRAGMA synchronous=FULL`), which must be set when opening the database
/// connection — see `jiezi-cloud-integrity::db_pragmas`.
#[derive(Debug, Clone, Deserialize)]
pub struct DatabaseBackupConfig {
    /// Enable or disable automatic periodic backups.
    pub enabled: bool,

    /// How often to run a backup, in minutes.
    pub interval_minutes: u64,

    /// Number of backup files to retain before rotating (oldest first).
    ///
    /// Default is 48 (= 12 hours at 15-minute intervals).
    pub keep_count: usize,

    /// Directory where backup `.db` files are stored.
    ///
    /// Relative paths are resolved against the process working directory.
    /// Ideally point this at a **different physical device** (e.g. a USB drive
    /// or network share) to protect against the primary disk failing.
    pub dir: PathBuf,
}

// ---- Auth -------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
pub struct AuthConfig {
    /// HMAC-SHA256 signing secret for JWTs.
    ///
    /// **Must be overridden via `JIEZI__AUTH__JWT_SECRET` in production.**
    /// The default value in `config/default.toml` is intentionally a weak
    /// placeholder — the server will refuse to start in production mode
    /// unless this is changed.
    pub jwt_secret: String,

    /// Access token lifetime in seconds (default: 900 = 15 minutes).
    pub access_token_ttl_seconds: u64,

    /// Refresh token lifetime in seconds (default: 2592000 = 30 days).
    pub refresh_token_ttl_seconds: u64,
}

// ---- Storage ----------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
pub struct StorageConfig {
    /// Root directory for local file storage.
    pub local_root: PathBuf,

    /// Maximum allowed upload size in bytes (default: 10 GiB).
    pub max_upload_bytes: u64,
}

// ---- Tracing ----------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
pub struct TracingConfig {
    /// `tracing-subscriber` filter string (e.g. `"debug"`, `"info,sqlx=warn"`).
    pub level: String,

    /// Log output format.
    pub format: TracingFormat,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum TracingFormat {
    /// Human-readable, coloured output.  Suitable for local development.
    Pretty,
    /// Structured JSON.  Suitable for log aggregators (Loki, CloudWatch, etc.).
    Json,
    /// Compact single-line text.
    Compact,
}

// ---- Loader -----------------------------------------------------------------

/// Load the application configuration using the layered strategy described in
/// the module documentation.
///
/// Pass `config_dir` to override the directory where `default.toml` and
/// environment-specific files are looked up.  Pass `None` to use
/// `$CWD/config` (the default for the server binary).
pub fn load_from(config_dir: PathBuf) -> Result<AppConfig, ConfigError> {
    // Load .env file into the process environment (no-op if file is absent).
    // Errors are intentionally ignored — a missing .env is fine in production.
    let _ = dotenvy::dotenv();

    // Determine the environment tier.
    let env = std::env::var("JIEZI__ENVIRONMENT")
        .unwrap_or_else(|_| "development".to_string());

    let cfg = config::Config::builder()
        // Layer 1 (lowest): committed default values.
        .add_source(
            config::File::from(config_dir.join("default"))
                .required(true),
        )
        // Layer 2: environment-specific overrides (optional file).
        .add_source(
            config::File::from(config_dir.join(&env))
                .required(false),
        )
        // Layer 3: local machine overrides — never committed (optional file).
        .add_source(
            config::File::from(config_dir.join("local"))
                .required(false),
        )
        // Layer 4 (highest): environment variables.
        // `JIEZI__SERVER__PORT=9090`  →  config key `server.port`
        .add_source(
            config::Environment::with_prefix("JIEZI")
                .separator("__"),
        )
        .build()?;

    Ok(cfg.try_deserialize()?)
}

/// Convenience wrapper that resolves `config_dir` to `$CWD/config`.
///
/// This is what the server binary should call.  In tests, prefer [`load_from`]
/// with an explicit path.
pub fn load() -> Result<AppConfig, ConfigError> {
    let config_dir = std::env::current_dir()
        .expect("cannot determine current working directory")
        .join("config");

    load_from(config_dir)
}

// ---- Validation -------------------------------------------------------------

impl AppConfig {
    /// Validate invariants that cannot be expressed in the type system.
    ///
    /// Returns an error string describing the first problem found, or `Ok(())`
    /// if the configuration is safe to use.
    pub fn validate(&self) -> Result<(), String> {
        const WEAK_SECRET: &str = "CHANGE_ME_IN_PRODUCTION_USE_A_LONG_RANDOM_STRING";

        if self.environment.is_production() && self.auth.jwt_secret == WEAK_SECRET {
            return Err(
                "JIEZI__AUTH__JWT_SECRET must be set to a secret value in production; \
                 the default placeholder is not safe"
                    .to_owned(),
            );
        }

        if self.auth.jwt_secret.len() < 32 {
            return Err(format!(
                "auth.jwt_secret must be at least 32 characters (got {})",
                self.auth.jwt_secret.len()
            ));
        }

        if self.server.port == 0 {
            return Err("server.port must be a non-zero value".to_owned());
        }

        Ok(())
    }
}

// ---- Tests ------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn test_config_dir() -> PathBuf {
        // Workspace root is three levels up from this source file.
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("config")
    }

    // Test 1: default config loads without error
    #[test]
    fn test_load_default_config() {
        let cfg = load_from(test_config_dir()).expect("default config should load");
        assert_eq!(cfg.environment, Environment::Development);
        assert_eq!(cfg.server.port, 8080);
    }

    // Test 2: server bind_address formats correctly
    #[test]
    fn test_bind_address() {
        let cfg = load_from(test_config_dir()).unwrap();
        let addr = cfg.server.bind_address();
        assert!(addr.contains(':'), "bind address should contain a colon");
    }

    // Test 3: auth TTLs are positive
    #[test]
    fn test_auth_ttls_are_positive() {
        let cfg = load_from(test_config_dir()).unwrap();
        assert!(cfg.auth.access_token_ttl_seconds > 0);
        assert!(cfg.auth.refresh_token_ttl_seconds > 0);
        // Refresh token should outlive access token
        assert!(cfg.auth.refresh_token_ttl_seconds > cfg.auth.access_token_ttl_seconds);
    }

    // Test 4: validate() passes for the development defaults
    #[test]
    fn test_validate_development_config() {
        let cfg = load_from(test_config_dir()).unwrap();
        // Development config uses the weak placeholder secret — that is OK.
        assert!(cfg.validate().is_ok());
    }

    // Test 5: validate() rejects weak secret in production mode
    #[test]
    fn test_validate_rejects_weak_secret_in_production() {
        let mut cfg = load_from(test_config_dir()).unwrap();
        cfg.environment = Environment::Production;
        // The default placeholder secret should be rejected.
        assert!(cfg.validate().is_err());
    }

    // Test 6: validate() rejects secrets shorter than 32 characters
    #[test]
    fn test_validate_rejects_short_secret() {
        let mut cfg = load_from(test_config_dir()).unwrap();
        cfg.auth.jwt_secret = "tooshort".to_owned();
        assert!(cfg.validate().is_err());
    }

    // Test 7: backup config defaults are sane
    #[test]
    fn test_backup_config_defaults() {
        let cfg = load_from(test_config_dir()).unwrap();
        assert!(cfg.database.backup.enabled);
        assert!(cfg.database.backup.interval_minutes > 0);
        assert!(cfg.database.backup.keep_count >= 4,
            "keep at least 4 backups (1 hour at 15-min intervals)");
    }

    // Test 8: Environment::is_production / is_development helpers
    #[test]
    fn test_environment_helpers() {
        assert!(Environment::Production.is_production());
        assert!(!Environment::Production.is_development());
        assert!(Environment::Development.is_development());
        assert!(!Environment::Development.is_production());
    }
}
