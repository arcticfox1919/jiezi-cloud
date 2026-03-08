//! HTTP error mapping for the `jiezi-cloud-server` crate.
//!
//! [`ApiError`] is a newtype wrapper around [`AppError`] that implements
//! [`actix_web::ResponseError`], mapping each variant to the appropriate HTTP
//! status code and a uniform JSON body:
//!
//! ```json
//! { "code": 404, "error": "NOT_FOUND", "message": "not found: file xyz" }
//! ```
//!
//! For structured error variants (`PayloadTooLarge`, `QuicRequired`) the body
//! carries additional machine-readable fields alongside the standard ones.
//!
//! The newtype is required because both `AppError` (from `jiezi-cloud-core`)
//! and `ResponseError` (from `actix-web`) are foreign to this crate —
//! the orphan rule prevents implementing a foreign trait for a foreign type.

use actix_web::http::StatusCode;
use actix_web::{HttpResponse, ResponseError};
use serde::Serialize;

use jiezi_cloud_core::error::AppError;

// ─── Response bodies ──────────────────────────────────────────────────────────

/// Standard error body used for most error variants.
#[derive(Serialize)]
struct ErrorBody {
    /// HTTP status code (mirrors the response status line).
    code: u16,
    /// Machine-readable error identifier (e.g. `"NOT_FOUND"`, `"CONFLICT"`).
    error: &'static str,
    /// Human-readable error description.
    message: String,
}

/// Error body returned when an upload exceeds the web-client size limit.
///
/// The client should surface `max_bytes` to the user and, if `quic_required`
/// is `true`, prompt them to install the native client.
#[derive(Serialize)]
struct PayloadTooLargeBody {
    code: u16,
    /// Machine-readable error identifier.
    error: &'static str,
    message: String,
    /// The applicable upload limit in bytes.
    max_bytes: u64,
    /// Whether the file *can* be transferred if the native client is used.
    quic_required: bool,
}

/// Error body returned to native clients that must switch to QUIC.
#[derive(Serialize)]
struct QuicRequiredBody {
    code: u16,
    /// Machine-readable error identifier.
    error: &'static str,
    message: String,
    /// Minimum file size that requires QUIC.
    threshold_bytes: u64,
    /// UDP port of the QUIC server the client should connect to.
    quic_port: u16,
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
            AppError::NotFound(_)       => StatusCode::NOT_FOUND,
            AppError::Unauthorized(_)   => StatusCode::UNAUTHORIZED,
            AppError::Forbidden(_)      => StatusCode::FORBIDDEN,
            AppError::Conflict(_)       => StatusCode::CONFLICT,
            AppError::Gone(_)           => StatusCode::GONE,
            AppError::Validation(_)     => StatusCode::UNPROCESSABLE_ENTITY,
            AppError::PayloadTooLarge { .. } => StatusCode::PAYLOAD_TOO_LARGE,
            AppError::QuicRequired { .. }    => StatusCode::UPGRADE_REQUIRED,
            AppError::Storage(_)
            | AppError::Database(_)
            | AppError::Serialization(_)
            | AppError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    fn error_response(&self) -> HttpResponse {
        let status = self.status_code();
        match &self.0 {
            AppError::PayloadTooLarge { max_bytes } => {
                HttpResponse::build(status).json(PayloadTooLargeBody {
                    code: status.as_u16(),
                    error: "FILE_TOO_LARGE_FOR_WEB",
                    message: format!(
                        "Files over {} MiB cannot be transferred via web browser. \
                         Please install the Jiezi Cloud native client.",
                        max_bytes / (1024 * 1024)
                    ),
                    max_bytes: *max_bytes,
                    quic_required: true,
                })
            }
            AppError::QuicRequired { threshold_bytes, quic_port } => {
                HttpResponse::build(status)
                    .insert_header(("Upgrade", "QUIC"))
                    .json(QuicRequiredBody {
                        code: status.as_u16(),
                        error: "QUIC_REQUIRED",
                        message: format!(
                            "Files >= {} MiB must be transferred via the QUIC transport. \
                             Connect to UDP port {}.",
                            threshold_bytes / (1024 * 1024),
                            quic_port,
                        ),
                        threshold_bytes: *threshold_bytes,
                        quic_port: *quic_port,
                    })
            }
            _ => HttpResponse::build(status).json(ErrorBody {
                code: status.as_u16(),
                error: self.0.error_code(),
                message: self.0.to_string(),
            }),
        }
    }
}
