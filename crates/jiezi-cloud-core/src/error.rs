//! Unified error type and result alias for the entire Jiezi Cloud platform.
//!
//! All service boundaries should map their internal errors to [`AppError`]
//! before returning them to callers.  This keeps the API surface clean and
//! makes error handling predictable throughout the codebase.

use thiserror::Error;

/// The canonical application error.
///
/// Each variant corresponds to a distinct failure category with its own
/// HTTP status code semantic so that the API layer can translate errors
/// to responses without additional branching.
#[derive(Debug, Error)]
pub enum AppError {
    /// A requested resource could not be found (HTTP 404).
    #[error("not found: {0}")]
    NotFound(String),

    /// The caller has not authenticated (HTTP 401).
    #[error("unauthorized: {0}")]
    Unauthorized(String),

    /// The caller is authenticated but lacks permission (HTTP 403).
    #[error("forbidden: {0}")]
    Forbidden(String),

    /// A uniqueness or state conflict occurred (HTTP 409).
    #[error("conflict: {0}")]
    Conflict(String),

    /// The resource existed but is no longer available (HTTP 410).
    ///
    /// Used for one-time tokens that have already been used or have expired.
    #[error("gone: {0}")]
    Gone(String),

    /// Input failed validation (HTTP 422).
    #[error("validation error: {0}")]
    Validation(String),

    /// A storage I/O operation failed (HTTP 500 or 503).
    #[error("storage error: {0}")]
    Storage(String),

    /// A database operation failed (HTTP 500).
    #[error("database error: {0}")]
    Database(String),

    /// JSON or binary serialization/deserialization failed (HTTP 500).
    #[error("serialization error: {0}")]
    Serialization(String),

    /// An unexpected internal error (HTTP 500).
    #[error("internal error: {0}")]
    Internal(String),
}

/// Convenience type alias used throughout the codebase.
pub type AppResult<T> = Result<T, AppError>;

// ─── Standard library conversions ────────────────────────────────────────────

impl From<std::io::Error> for AppError {
    fn from(e: std::io::Error) -> Self {
        AppError::Storage(e.to_string())
    }
}

impl From<serde_json::Error> for AppError {
    fn from(e: serde_json::Error) -> Self {
        AppError::Serialization(e.to_string())
    }
}

