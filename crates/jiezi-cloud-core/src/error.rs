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

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_not_found_display() {
        let err = AppError::NotFound("users/42".to_owned());
        assert_eq!(err.to_string(), "not found: users/42");
    }

    #[test]
    fn test_unauthorized_display() {
        let err = AppError::Unauthorized("token expired".to_owned());
        assert!(err.to_string().contains("unauthorized"));
    }

    #[test]
    fn test_validation_display() {
        let err = AppError::Validation("username too short".to_owned());
        assert!(err.to_string().contains("validation error"));
    }

    #[test]
    fn test_from_io_error_maps_to_storage_variant() {
        let io_err = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "access denied");
        let app_err: AppError = io_err.into();
        assert!(matches!(app_err, AppError::Storage(_)));
    }

    #[test]
    fn test_from_json_error_maps_to_serialization_variant() {
        let json_err = serde_json::from_str::<String>("not valid json!!!").unwrap_err();
        let app_err: AppError = json_err.into();
        assert!(matches!(app_err, AppError::Serialization(_)));
    }

    #[test]
    fn test_app_result_ok_carries_value() {
        let result: AppResult<u32> = Ok(99);
        assert!(matches!(result, Ok(99)));
    }

    #[test]
    fn test_app_result_err_is_accessible() {
        let result: AppResult<u32> = Err(AppError::Forbidden("read-only space".to_owned()));
        assert!(result.is_err());
        let Err(err) = result else {
            panic!("expected Err variant");
        };
        assert!(err.to_string().contains("forbidden"));
    }
}
