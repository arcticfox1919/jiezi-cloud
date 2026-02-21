//! Download token API handlers.
//!
//! Implements Stage 8: time-limited, optionally single-use download tokens
//! that allow unauthenticated access to a file.
//!
//! ## Endpoints
//!
//! | Method | Path                              | Auth | Description                              |
//! |--------|-----------------------------------|------|------------------------------------------|
//! | POST   | `/download/{id}/token`            | JWT  | Issue a download token for a file        |
//! | GET    | `/download/t/{token}`             | none | Download file via token (no JWT needed)  |
//!
//! ## Token flow
//!
//! 1. Authenticated user calls `POST /download/{id}/token` to obtain a short-lived token.
//! 2. The token URL (`GET /download/t/{token}`) can be embedded in `<a href>` or shared.
//! 3. The token is validated on each request; one-time tokens are invalidated immediately.

use std::str::FromStr;

use actix_web::{web, HttpRequest, HttpResponse};
use serde::{Deserialize, Serialize};

use jiezi_cloud_core::{
    error::AppError,
    types::{FileId, UserId},
};

use crate::{
    error::ApiError,
    middleware::auth::AuthUser,
    repository::download_token::DEFAULT_TOKEN_TTL_SECS,
    state::AppState,
};

// ─── Route registration ───────────────────────────────────────────────────────

/// Register download-token routes under `/api/v1/download`.
pub fn configure_token(cfg: &mut web::ServiceConfig) {
    cfg
        // Static prefix `t/` before `{id}` so they don't conflict.
        .route("/t/{token}",   web::get().to(download_by_token))
        .route("/{id}/token",  web::post().to(issue_token));
}

// ─── Request / response types ─────────────────────────────────────────────────

/// Request body for `POST /download/{id}/token`.
#[derive(Deserialize, utoipa::ToSchema)]
pub struct IssueTokenRequest {
    /// Token lifetime in seconds.  Capped at 7 days server-side.
    /// Defaults to 3 600 s (1 hour) when absent.
    pub ttl_secs: Option<u64>,
    /// When `true` the token is invalidated after the first download.
    #[serde(default)]
    pub one_time: bool,
}

/// Response body for `POST /download/{id}/token`.
#[derive(Serialize, utoipa::ToSchema)]
pub struct IssueTokenResponse {
    /// The opaque download token string (32 hex chars).
    pub token: String,
    /// Full URL path the recipient can use to download the file.
    pub url: String,
    /// Unix timestamp (seconds) at which the token expires.
    pub expires_at: i64,
    pub one_time: bool,
}

// ─── Handlers ─────────────────────────────────────────────────────────────────

/// `POST /download/{id}/token` — issue a time-limited download token.
///
/// The caller must be authenticated.  The token can be passed to anyone
/// (or embedded in a `<a href>`) and does not require a JWT to consume.
#[utoipa::path(
    post,
    path = "/api/v1/download/{id}/token",
    params(("id" = String, Path, description = "File node ID")),
    request_body = IssueTokenRequest,
    responses(
        (status = 200, description = "Token issued", body = IssueTokenResponse),
        (status = 404, description = "File not found"),
    ),
    security(("bearer_auth" = [])),
    tag = "download"
)]
pub async fn issue_token(
    state: web::Data<AppState>,
    auth: AuthUser,
    path: web::Path<String>,
    body: web::Json<IssueTokenRequest>,
    req: HttpRequest,
) -> Result<HttpResponse, ApiError> {
    let file_id = parse_file_id(&path.into_inner())?;
    let user_id = parse_user_id(&auth.0.sub)?;

    // Verify the file exists before issuing a token.
    state.vfs.get_node(&file_id).await?;

    let ttl = body.ttl_secs.unwrap_or(DEFAULT_TOKEN_TTL_SECS);
    let token_record = state
        .download_tokens
        .create(&file_id, &user_id, ttl, body.one_time)
        .await?;

    // Build the download URL from the current request's connection info.
    let conn = req.connection_info();
    let base = format!("{}://{}", conn.scheme(), conn.host());
    let url = format!("{base}/api/v1/download/t/{}", token_record.token);

    Ok(HttpResponse::Ok().json(IssueTokenResponse {
        token: token_record.token,
        url,
        expires_at: token_record.expires_at.timestamp(),
        one_time: token_record.one_time,
    }))
}

/// `GET /download/t/{token}` — download a file using a pre-issued token.
///
/// No JWT required.  The token is looked up, validated, and (for one-time
/// tokens) consumed atomically before the download begins.
#[utoipa::path(
    get,
    path = "/api/v1/download/t/{token}",
    params(("token" = String, Path, description = "Download token")),
    responses(
        (status = 200, description = "File bytes"),
        (status = 206, description = "Partial content (Range request)"),
        (status = 410, description = "Token expired or already used"),
        (status = 404, description = "Token or file not found"),
    ),
    tag = "download"
)]
pub async fn download_by_token(
    state: web::Data<AppState>,
    path: web::Path<String>,
    req: HttpRequest,
) -> Result<HttpResponse, ApiError> {
    let token = path.into_inner();

    // Validate and optionally consume the token.
    let token_record = state.download_tokens.validate_and_consume(&token).await?;

    let file_id: FileId = token_record.file_node_id.parse().map_err(|_| {
        ApiError(AppError::Internal("invalid file_node_id in token".into()))
    })?;

    // Honour Range header.
    if let Some(range_header) = req.headers().get("Range") {
        if let Ok(range_str) = range_header.to_str() {
            if let Some(bytes_range) = range_str.strip_prefix("bytes=") {
                let parts: Vec<&str> = bytes_range.splitn(2, '-').collect();
                if parts.len() == 2 {
                    let start: Option<u64> = parts[0].parse().ok();
                    let end: Option<u64> = parts[1].parse().ok();
                    if let (Some(start), Some(end)) = (start, end) {
                        let data = state.download.read_range(&file_id, start, end + 1).await?;
                        return Ok(HttpResponse::PartialContent()
                            .insert_header(("Content-Range", format!("bytes {start}-{end}/*")))
                            .content_type("application/octet-stream")
                            .body(data));
                    }
                }
            }
        }
    }

    // Full streaming download via the bounded mpsc channel.
    let stream = state.download.stream_file(&file_id).await?;
    let body = collect_stream(stream).await?;
    Ok(HttpResponse::Ok()
        .content_type("application/octet-stream")
        .body(body))
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

fn parse_file_id(s: &str) -> Result<FileId, ApiError> {
    FileId::from_str(s).map_err(|_| {
        ApiError(AppError::Validation(format!("invalid file ID: {s}")))
    })
}

fn parse_user_id(s: &str) -> Result<UserId, ApiError> {
    UserId::from_str(s).map_err(|_| {
        ApiError(AppError::Unauthorized(
            "invalid user ID in token subject".into(),
        ))
    })
}

/// Drain the [`FileDownloadStream`] receiver into a contiguous [`bytes::Bytes`].
async fn collect_stream(
    mut stream: jiezi_cloud_storage::FileDownloadStream,
) -> Result<bytes::Bytes, ApiError> {
    let mut buf = bytes::BytesMut::with_capacity(stream.meta.total_size as usize);
    while let Some(chunk) = stream.rx.recv().await {
        let data = chunk?;
        buf.extend_from_slice(&data);
    }
    Ok(buf.freeze())
}
