//! HTTP error mapping for the `jiezi-cloud-server` crate.
//!
//! [`ApiError`] is a newtype wrapper around [`AppError`] that implements
//! [`actix_web::ResponseError`], mapping each variant to the appropriate HTTP
//! status code and a uniform JSON body:
//!
//! ```json
//! { "code": 404, "message": "not found: file xyz" }
//! ```
//!
//! The newtype is required because both `AppError` (from `jiezi-cloud-core`)
//! and `ResponseError` (from `actix-web`) are foreign to this crate —
//! the orphan rule prevents implementing a foreign trait for a foreign type.

use actix_web::http::StatusCode;
use actix_web::{HttpResponse, ResponseError};
use serde::Serialize;

use jiezi_cloud_core::error::AppError;

// ─── Response body ────────────────────────────────────────────────────────────

#[derive(Serialize)]
struct ErrorBody {
    /// HTTP status code (mirrors the response status line).
    code: u16,
    /// Human-readable error description.
    message: String,
}

// ─── ApiError ─────────────────────────────────────────────────────────────────

/// Newtype that bridges [`AppError`] to [`actix_web::ResponseError`].
///
/// Convert with `ApiError::from(app_err)` or the `?` operator in handlers
/// that return `Result<HttpResponse, ApiError>`.
#[derive(Debug)]
pub struct ApiError(pub AppError);

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl From<AppError> for ApiError {
    fn from(e: AppError) -> Self {
        Self(e)
    }
}

impl ResponseError for ApiError {
    fn status_code(&self) -> StatusCode {
        match &self.0 {
            AppError::NotFound(_) => StatusCode::NOT_FOUND,
            AppError::Unauthorized(_) => StatusCode::UNAUTHORIZED,
            AppError::Forbidden(_) => StatusCode::FORBIDDEN,
            AppError::Conflict(_) => StatusCode::CONFLICT,
            AppError::Validation(_) => StatusCode::UNPROCESSABLE_ENTITY,
            AppError::Storage(_)
            | AppError::Database(_)
            | AppError::Serialization(_)
            | AppError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    fn error_response(&self) -> HttpResponse {
        let status = self.status_code();
        HttpResponse::build(status).json(ErrorBody {
            code: status.as_u16(),
            message: self.0.to_string(),
        })
    }
}
