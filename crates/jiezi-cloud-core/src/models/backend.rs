//! Storage-backend configuration models.
//!
//! # Design philosophy
//!
//! Instead of a closed enum that must be updated every time a new backend is
//! added, the driver is represented as an **open string identifier** with a
//! free-form JSON configuration blob.  Well-known drivers have typed config
//! structs and constants; a third-party driver requires no changes here.
//!
//! This mirrors how [AList](https://alist.nn.ci/) handles its 40+ drivers:
//! a driver kind string plus a per-driver settings object.
//!
//! ## Adding a new driver
//!
//! 1. Define a config struct (e.g. `AliyunPanConfig`).
//! 2. Add a `pub const DRIVER_*: &str` constant.
//! 3. Implement the `StorageBackend` trait in `jiezi-cloud-storage`.
//! 4. Register the driver in the backend factory.
//!
//! **No changes to this file.  No schema migrations.**
//!
//! ## Serialisation on disk / in DB
//!
//! Stored as a flat JSON object with a `"driver"` key plus driver-specific
//! fields merged at the top level (via `#[serde(flatten)]`):
//!
//! ```json
//! { "driver": "s3_compatible", "bucket": "my-bucket", "region": "us-east-1" }
//! { "driver": "webdav",        "url": "https://nextcloud.example.com/…"     }
//! { "driver": "aliyun_pan",    "token": "…", "root_path": "/备份"           }
//! ```

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::types::{BackendId, UserId};

// ─── Well-known driver-kind constants ─────────────────────────────────────────
//
// These are the drivers shipped with jiezi-cloud.  Third-party or
// user-contributed drivers may use any other string and need not be listed here.

/// Built-in: local filesystem directory on the server host.
pub const DRIVER_LOCAL_FS: &str = "local_fs";
/// Built-in: any S3-compatible object-storage service (AWS, MinIO, R2, B2,
/// Aliyun OSS, Tencent COS, Cloudflare R2 …).
pub const DRIVER_S3_COMPATIBLE: &str = "s3_compatible";
/// Built-in: WebDAV server (Nextcloud, ownCloud, nginx dav module …).
pub const DRIVER_WEBDAV: &str = "webdav";

// ─── Typed config structs for built-in drivers ────────────────────────────────

/// Configuration for the [`DRIVER_LOCAL_FS`] driver.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalFsConfig {
    /// Absolute path to the root directory that holds all chunk files.
    pub root_dir: String,
}

/// Configuration for the [`DRIVER_S3_COMPATIBLE`] driver.
///
/// Compatible with AWS S3, MinIO, Cloudflare R2, Backblaze B2, Wasabi,
/// Tencent COS (S3 mode), Aliyun OSS (S3 mode), and any S3-API service.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct S3Config {
    /// Custom endpoint URL for S3-compatible services.
    ///
    /// Omit for vanilla AWS S3.  Examples:
    /// - MinIO:          `"http://minio.internal:9000"`
    /// - Cloudflare R2:  `"https://<account>.r2.cloudflarestorage.com"`
    /// - Backblaze B2:   `"https://s3.us-west-004.backblazeb2.com"`
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,

    /// S3 bucket name.
    pub bucket: String,

    /// AWS region string, e.g. `"us-east-1"` or `"auto"` (R2).
    pub region: String,

    /// Optional path prefix inside the bucket, e.g. `"jiezi-cloud/"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prefix: Option<String>,

    /// Access-key ID.
    pub access_key_id: String,

    /// Secret access key.  **Store encrypted at rest in production.**
    pub secret_access_key: String,
}

/// Configuration for the [`DRIVER_WEBDAV`] driver.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebDavConfig {
    /// Base URL of the WebDAV collection.
    ///
    /// Example: `"https://nextcloud.example.com/remote.php/dav/files/alice/"`
    pub url: String,

    /// HTTP Basic auth username.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,

    /// HTTP Basic auth password.  **Store encrypted at rest in production.**
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,

    /// HTTP Bearer token (alternative to Basic auth).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bearer_token: Option<String>,
}

// ─── BackendDriver ────────────────────────────────────────────────────────────

/// Open backend driver descriptor: a `kind` string + free-form JSON config.
///
/// The `kind` field selects which `StorageBackend` implementation to
/// instantiate.  For well-known drivers the config can be deserialised into a
/// typed struct; for unknown/custom drivers the raw JSON is preserved.
///
/// # Serialisation
///
/// Stored as a flat JSON object — the `"driver"` key holds the kind, and the
/// remaining keys are the driver-specific fields flattened in:
///
/// ```json
/// { "driver": "s3_compatible", "bucket": "my-bucket", "region": "us-east-1" }
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BackendDriver {
    /// Driver kind string.  Use a `DRIVER_*` constant for built-in drivers.
    #[serde(rename = "driver")]
    pub kind: String,

    /// Driver-specific configuration, flattened into the JSON object.
    #[serde(flatten)]
    pub config: serde_json::Map<String, serde_json::Value>,
}

impl BackendDriver {
    // ── Constructors ──────────────────────────────────────────────────────────

    /// Build a driver descriptor from a typed config struct.
    ///
    /// # Panics
    ///
    /// Panics if `config` cannot be serialised to a JSON object — this is a
    /// programming error; all provided config types serialise correctly.
    pub fn from_config<C: Serialize>(kind: impl Into<String>, config: &C) -> Self {
        let json = serde_json::to_value(config).expect("BackendDriver config serialisation");
        let map = match json {
            serde_json::Value::Object(m) => m,
            other => panic!("BackendDriver config must be a JSON object, got: {other:?}"),
        };
        Self { kind: kind.into(), config: map }
    }

    /// Create a local-filesystem driver.
    pub fn local_fs(config: LocalFsConfig) -> Self {
        Self::from_config(DRIVER_LOCAL_FS, &config)
    }

    /// Create an S3-compatible driver.
    pub fn s3_compatible(config: S3Config) -> Self {
        Self::from_config(DRIVER_S3_COMPATIBLE, &config)
    }

    /// Create a WebDAV driver.
    pub fn webdav(config: WebDavConfig) -> Self {
        Self::from_config(DRIVER_WEBDAV, &config)
    }

    /// Create a custom / third-party driver with a raw JSON config map.
    pub fn custom(
        kind: impl Into<String>,
        config: serde_json::Map<String, serde_json::Value>,
    ) -> Self {
        Self { kind: kind.into(), config }
    }

    // ── Typed accessors ───────────────────────────────────────────────────────

    /// Try to deserialise the config as [`LocalFsConfig`].
    /// Returns `None` if the kind does not match or deserialisation fails.
    pub fn as_local_fs(&self) -> Option<LocalFsConfig> {
        if self.kind != DRIVER_LOCAL_FS { return None; }
        serde_json::from_value(serde_json::Value::Object(self.config.clone())).ok()
    }

    /// Try to deserialise the config as [`S3Config`].
    pub fn as_s3_compatible(&self) -> Option<S3Config> {
        if self.kind != DRIVER_S3_COMPATIBLE { return None; }
        serde_json::from_value(serde_json::Value::Object(self.config.clone())).ok()
    }

    /// Try to deserialise the config as [`WebDavConfig`].
    pub fn as_webdav(&self) -> Option<WebDavConfig> {
        if self.kind != DRIVER_WEBDAV { return None; }
        serde_json::from_value(serde_json::Value::Object(self.config.clone())).ok()
    }

    /// Returns `true` if the kind matches a built-in `DRIVER_*` constant.
    pub fn is_builtin(&self) -> bool {
        matches!(
            self.kind.as_str(),
            DRIVER_LOCAL_FS | DRIVER_S3_COMPATIBLE | DRIVER_WEBDAV
        )
    }

    /// Human-readable display name derived from the driver kind.
    pub fn display_name(&self) -> String {
        match self.kind.as_str() {
            DRIVER_LOCAL_FS => "Local Filesystem".into(),
            DRIVER_S3_COMPATIBLE => "S3-Compatible".into(),
            DRIVER_WEBDAV => "WebDAV".into(),
            other => other.to_owned(),
        }
    }
}

// ─── BackendConfig ────────────────────────────────────────────────────────────

/// Persisted configuration record for a single storage backend.
///
/// Stored in the `storage_backend_configs` table.  The driver kind and its
/// parameters are stored as a flat JSON object in `backend_type_json`, so new
/// driver types require no schema migration.
///
/// # Design note
///
/// There is no concept of a "required" or "default" backend.  A user may
/// configure any combination of S3, WebDAV, or other third-party backends
/// (including zero local-filesystem backends).  The system is designed to
/// operate entirely on third-party cloud storage.
///
/// # TODO
///
/// - `TODO(backend-repository)`: implement a SeaORM `BackendConfigEntity` and
///   a `BackendConfigRepository` with at minimum:
///   - `list_by_owner(user_id) -> Vec<BackendConfig>`
///   - `get(id) -> Option<BackendConfig>`
///   - `create(owner_id, display_name, driver) -> BackendConfig`
///   - `update(id, patch) -> BackendConfig`
///   - `delete(id)`
/// - `TODO(backend-api)`: REST/gRPC handlers so clients can list and manage
///   their configured backup backends.
///
/// Note: `PartialEq` but not `Eq` because [`BackendDriver`] contains a
/// `serde_json::Map` which does not implement `Eq`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BackendConfig {
    /// Unique identifier for this backend configuration.
    pub id: BackendId,

    /// The user who owns (configured) this backend.
    ///
    /// `None` for system-wide / admin-configured backends.
    pub owner_id: Option<UserId>,

    /// Free-form display name chosen by the user, e.g. `"Home NAS"`.
    pub display_name: String,

    /// Driver kind and connection parameters.
    pub driver: BackendDriver,

    /// Whether this backend is active and should receive writes.
    ///
    /// Disabled backends are skipped during upload but remain readable.
    pub is_enabled: bool,

    /// Lower numbers have higher priority in read-path fallback ordering.
    pub priority: i32,

    /// When this configuration record was created.
    pub created_at: DateTime<Utc>,

    /// When this configuration record was last updated.
    pub updated_at: DateTime<Utc>,
}

// ─── ReplicationPolicy ────────────────────────────────────────────────────────

/// Describes how many / which backends a file upload should be written to.
///
/// Evaluated by `StorageManager::put_with_policy` at upload time:
///
/// ```text
/// ReplicationPolicy::All          → write to every enabled backend
/// ReplicationPolicy::MinN(2)      → write to at least 2 enabled backends
///                                   (ordered by priority, first N selected)
/// ReplicationPolicy::Specific(…)  → write only to the named backend IDs
/// ```
///
/// Because local storage is optional, `MinN{n: 1}` is the minimum sensible
/// policy for a cloud-only deployment — it writes to whichever single
/// highest-priority backend is configured.
///
/// # TODO
///
/// - `TODO(put-with-policy)`: `StorageManager::put_with_policy` in
///   `jiezi-cloud-storage` is not yet implemented; see that crate's TODO list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "policy")]
pub enum ReplicationPolicy {
    /// Replicate to **all** enabled backends.
    All,

    /// Replicate to **at least N** enabled backends (highest-priority first).
    ///
    /// If fewer than N backends are available the write proceeds but a warning
    /// is emitted.
    #[serde(rename = "min_n")]
    MinN {
        /// Minimum number of backends to write to.
        n: usize,
    },

    /// Replicate only to the explicitly listed backend IDs.
    ///
    /// Backends not in this list are skipped entirely (even if enabled).
    Specific {
        /// The backend IDs that should receive this write.
        backend_ids: Vec<BackendId>,
    },
}

impl Default for ReplicationPolicy {
    fn default() -> Self {
        Self::All
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────
//
// Tests live in a separate `backend_tests.rs` file so this source file stays
// readable without test scaffolding mixed in.
//
