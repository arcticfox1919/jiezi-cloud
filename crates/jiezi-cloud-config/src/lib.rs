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
    pub security: SecurityConfig,
    pub email:    EmailConfig,
    #[serde(default)]
    pub quic:     QuicConfig,
    #[serde(default)]
    pub tunnel:   TunnelConfig,
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
    /// ES256 (ECDSA P-256) private key in PKCS#8 PEM format.
    ///
    /// Set to the special sentinel `"GENERATE"` to have the server
    /// auto-generate a fresh ephemeral keypair at startup.  This is
    /// convenient for development but tokens are **invalidated on every
    /// restart**.  In production, set this via the
    /// `JIEZI__AUTH__JWT_PRIVATE_KEY_PEM` environment variable.
    pub jwt_private_key_pem: String,

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

    /// Files strictly below this size (bytes) are sent via HTTP.
    /// Native clients MUST use QUIC for files at or above this threshold.
    /// Default: 20 MiB (20 * 1024 * 1024 = 20_971_520).
    #[serde(default = "default_large_file_threshold")]
    pub large_file_threshold_bytes: u64,

    /// Maximum HTTP upload bytes accepted from **web** clients when the
    /// tunnel relay is **disabled** (pure LAN access).  Browser uploads
    /// above this limit receive `413` with error code `FILE_TOO_LARGE_FOR_WEB`.
    /// Default: 500 MiB.
    #[serde(default = "default_web_upload_no_tunnel")]
    pub web_upload_max_bytes_no_tunnel: u64,

    /// Maximum HTTP upload bytes accepted from **web** clients when the
    /// tunnel relay is **enabled**.  Relay traffic consumes upstream bandwidth,
    /// so a more conservative limit is used.  Default: 100 MiB.
    #[serde(default = "default_web_upload_with_tunnel")]
    pub web_upload_max_bytes_with_tunnel: u64,
}

fn default_large_file_threshold() -> u64 {
    20 * 1024 * 1024 // 20 MiB
}

fn default_web_upload_no_tunnel() -> u64 {
    500 * 1024 * 1024 // 500 MiB
}

fn default_web_upload_with_tunnel() -> u64 {
    100 * 1024 * 1024 // 100 MiB
}

// ---- QUIC transfer server --------------------------------------------------

/// Configuration for the JTP/1 QUIC file-transfer server.
///
/// This server handles large-file uploads and downloads over QUIC (UDP),
/// typically for files >= `storage.large_file_threshold_bytes`.
#[derive(Debug, Clone, Deserialize)]
pub struct QuicConfig {
    /// Whether the QUIC transfer server is enabled.
    #[serde(default)]
    pub enabled: bool,

    /// UDP bind address (e.g. `"0.0.0.0"`).
    #[serde(default = "default_quic_listen_addr")]
    pub listen_addr: String,

    /// UDP port to listen on (default: 4433).
    #[serde(default = "default_quic_port")]
    pub port: u16,

    /// Path to a PEM TLS certificate file.
    /// Set to `"GENERATE"` to auto-generate an ephemeral self-signed cert at
    /// startup (development only — clients must use fingerprint pinning).
    #[serde(default = "default_generate")]
    pub tls_cert_pem: String,

    /// Path to a PEM TLS private key file.
    /// Set to `"GENERATE"` to match `tls_cert_pem = "GENERATE"`.
    #[serde(default = "default_generate")]
    pub tls_key_pem: String,

    /// Maximum number of concurrent bidirectional streams per connection.
    #[serde(default = "default_quic_max_streams")]
    pub max_concurrent_bidi_streams: u32,

    /// Idle timeout in seconds before a connection is closed.
    #[serde(default = "default_quic_idle_timeout")]
    pub idle_timeout_secs: u64,

    /// Maximum JTP/1 chunk data payload size in bytes (default: 4 MiB).
    /// Must be <= the FastCDC max chunk size used during upload.
    #[serde(default = "default_quic_max_chunk_bytes")]
    pub max_chunk_bytes: u32,
}

impl Default for QuicConfig {
    fn default() -> Self {
        Self {
            enabled:                    false,
            listen_addr:               default_quic_listen_addr(),
            port:                      default_quic_port(),
            tls_cert_pem:              default_generate(),
            tls_key_pem:               default_generate(),
            max_concurrent_bidi_streams: default_quic_max_streams(),
            idle_timeout_secs:         default_quic_idle_timeout(),
            max_chunk_bytes:           default_quic_max_chunk_bytes(),
        }
    }
}

fn default_quic_listen_addr()  -> String { "0.0.0.0".to_owned() }
fn default_quic_port()         -> u16    { 4433 }
fn default_generate()          -> String { "GENERATE".to_owned() }
fn default_quic_max_streams()  -> u32    { 128 }
fn default_quic_idle_timeout() -> u64    { 30 }
fn default_quic_max_chunk_bytes() -> u32 { 4 * 1024 * 1024 } // 4 MiB

// ---- Tunnel ----------------------------------------------------------------

/// Configuration for the optional jiezi-cloud-tunnel relay service.
///
/// When the tunnel is enabled, web-client uploads are subject to a tighter
/// byte limit (`storage.web_upload_max_bytes_with_tunnel`) to avoid saturating
/// relay bandwidth.  Native clients are unaffected and always use QUIC for
/// large files.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct TunnelConfig {
    /// Whether the tunnel relay is enabled.  Affects web upload/download limits.
    #[serde(default)]
    pub enabled: bool,
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

// ──── Security ─────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
pub struct SecurityConfig {
    // ── Login brute-force lockout ─────────────────────────────────────────────────────

    /// Number of consecutive failed logins before the account is locked.
    ///
    /// Set to `0` to disable lockout entirely (not recommended).
    pub max_login_attempts: u32,

    /// How long (in seconds) to lock an account after exceeding
    /// `max_login_attempts`.
    pub lockout_duration_secs: u64,

    // ── Rate limiting ────────────────────────────────────────────────────────────

    /// Maximum number of auth-endpoint requests (login, register, refresh)
    /// from a single IP address per minute.  Excess requests receive 429.
    pub auth_rate_limit_per_minute: u32,

    // ── CORS ───────────────────────────────────────────────────────────────────

    /// Explicit list of origins allowed by the CORS policy
    /// (e.g. `["http://localhost:5173", "https://cloud.myname.com"]`).
    ///
    /// If the list is **empty** the server allows any origin (`*`) — acceptable
    /// for same-machine development but **must** be set in production.
    pub cors_allowed_origins: Vec<String>,

    // ── Argon2 DoS guard ──────────────────────────────────────────────────────────

    /// Maximum allowed plaintext password length in bytes.
    ///
    /// Argon2 derives a 32-byte key from the password, but must hash all of
    /// `n` bytes first.  An attacker submitting a multi-megabyte password can
    /// cause a CPU spike.  Requests with passwords longer than this limit are
    /// rejected before any crypto work is done.
    ///
    /// 128 bytes comfortably covers any human-chosen passphrase while blocking
    /// trivial DoS payloads.
    pub max_password_bytes: usize,
}

// ──── Email ────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
pub struct EmailConfig {
    /// Whether SMTP sending is active.  When `false` (dev default), verification
    /// tokens are logged at `INFO` level instead of emailed.
    pub enabled: bool,

    /// SMTP server hostname.
    pub smtp_host: String,
    /// SMTP server port (587 for STARTTLS, 465 for implicit TLS).
    pub smtp_port: u16,
    pub smtp_username: String,
    pub smtp_password: String,

    /// RFC 5321 "From" address (e.g. `"no-reply@cloud.example.com"`).
    pub from_address: String,
    /// Human-readable sender name shown in email clients.
    pub from_name: String,

    /// If `true`, a user must verify their email OTP before the account is created.
    pub verification_required: bool,

    /// How long (seconds) an OTP remains valid before expiring.
    /// Default: 600 (10 minutes).
    pub otp_ttl_secs: u64,
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
        const GENERATE_SENTINEL: &str = "GENERATE";

        // Block the ephemeral-keypair sentinel in production — it would
        // invalidate all sessions on every restart.
        if self.environment.is_production()
            && self.auth.jwt_private_key_pem == GENERATE_SENTINEL
        {
            return Err(
                "JIEZI__AUTH__JWT_PRIVATE_KEY_PEM must be set to a real PKCS#8 PEM in \
                 production; the \"GENERATE\" placeholder is not safe"
                    .to_owned(),
            );
        }

        // In non-production environments, "GENERATE" is allowed (auto-keygen).
        // Any other value must look like a PEM file.
        if self.auth.jwt_private_key_pem != GENERATE_SENTINEL
            && !self.auth.jwt_private_key_pem.contains("-----BEGIN")
        {
            return Err(
                "auth.jwt_private_key_pem must be a PKCS#8 PEM string or \"GENERATE\""
                    .to_owned(),
            );
        }

        if self.server.port == 0 {
            return Err("server.port must be a non-zero value".to_owned());
        }

        // Email verification config consistency checks.
        // `verification_required = true` with `enabled = false` means users
        // would register but never be able to log in — catch this at startup.
        if self.email.verification_required && !self.email.enabled {
            return Err(
                "email.verification_required = true requires email.enabled = true; \
                 otherwise registered users can never verify and will be locked out"
                    .to_owned(),
            );
        }
        // If SMTP sending is enabled, the essential fields must be non-empty.
        if self.email.enabled {
            if self.email.smtp_host.is_empty() {
                return Err("email.smtp_host must not be empty when email.enabled = true".to_owned());
            }
            if self.email.from_address.is_empty() {
                return Err("email.from_address must not be empty when email.enabled = true".to_owned());
            }
        }

        Ok(())
    }
}
