//! Resumable upload API handlers.
//!
//! Implements Stage 7.5: chunked HTTP upload with session management so that
//! large files can be uploaded incrementally and resumed after network
//! interruptions.
//!
//! ## Endpoints
//!
//! | Method | Path                                      | Description                        |
//! |--------|-------------------------------------------|------------------------------------|
//! | POST   | `/upload/prepare`                         | Create a session; fast-path dedup  |
//! | GET    | `/upload/{session_id}/status`             | List missing chunk indices         |
//! | POST   | `/upload/{session_id}/chunk/{index}`      | Upload one chunk                   |
//! | POST   | `/upload/{session_id}/complete`           | Assemble and create VFS record     |
//! | DELETE | `/upload/{session_id}`                    | Cancel and clean up                |
//!
//! ## Resumability protocol
//!
//! 1. **PREPARE** — client sends file metadata; server returns `session_id`.
//!    If the file hash already exists in storage the server returns
//!    `instant_complete: true` with the existing [`FileNode`] (zero-byte upload).
//! 2. **STATUS** — client queries which chunk indices are still missing.
//! 3. **CHUNK** — client uploads each missing chunk (idempotent: re-upload is safe).
//! 4. **COMPLETE** — all chunks present; server assembles, hashes, stores, and
//!    creates the VFS record.  Returns the new [`FileNode`].
//! 5. **CANCEL** — optional early termination; server deletes temp files.

use std::str::FromStr;

use actix_web::{web, HttpResponse};
use bytes::Bytes;
use serde::{Deserialize, Serialize};

use jiezi_cloud_core::{
    error::AppError,
    models::{backend::ReplicationPolicy, file::FileNode},
    types::{FileId, UserId},
};

use crate::{
    error::ApiError,
    middleware::auth::AuthUser,
    repository::upload_session::{assemble_chunks, DEFAULT_CHUNK_SIZE},
    state::AppState,
};

// ─── Route registration ───────────────────────────────────────────────────────

/// Register resumable-upload routes under `/api/v1/upload`.
///
/// Must be called **after** `configure_simple_upload` so that the more
/// specific `/upload/prepare` static path comes before the `/{session_id}` catch-all.
pub fn configure_resumable(cfg: &mut web::ServiceConfig) {
    cfg
        .route("/prepare",                          web::post().to(prepare))
        .route("/{session_id}/status",             web::get().to(status))
        .route("/{session_id}/chunk/{index}",      web::post().to(upload_chunk))
        .route("/{session_id}/complete",           web::post().to(complete))
        .route("/{session_id}",                    web::delete().to(cancel));
}

// ─── Request / response types ─────────────────────────────────────────────────

/// Request body for `POST /upload/prepare`.
#[derive(Deserialize, utoipa::ToSchema)]
pub struct PrepareRequest {
    /// ID of the VFS parent directory that will hold the file.
    pub parent_id: String,
    /// Filename.
    pub file_name: String,
    /// Total file size in bytes.
    pub total_size: u64,
    /// SHA-256 hex digest of the complete file (used for dedup and integrity).
    pub content_hash: String,
    /// Optional MIME type hint.
    pub mime_type: Option<String>,
    /// Preferred chunk size in bytes.  Server may adjust.
    /// Defaults to [`DEFAULT_CHUNK_SIZE`] (8 MiB) when not supplied.
    pub chunk_size: Option<u64>,
    /// When `true` this is a one-time upload — do not accept the instant-complete
    /// shortcut even if the hash is known.
    #[serde(default)]
    pub skip_dedup: bool,
}

/// Response body for `POST /upload/prepare`.
#[derive(Serialize, utoipa::ToSchema)]
pub struct PrepareResponse {
    /// Session identifier to use in subsequent requests.
    pub session_id: String,
    /// Negotiated chunk size in bytes.
    pub chunk_size: u64,
    /// Total number of chunks expected.
    pub chunk_count: u32,
    /// When `true` the file was already present; `file_node` is populated and
    /// no chunks need to be uploaded.
    pub instant_complete: bool,
    /// Set when `instant_complete = true`.
    pub file_node: Option<FileNode>,
}

/// Response body for `GET /upload/{session_id}/status`.
#[derive(Serialize, utoipa::ToSchema)]
pub struct SessionStatusResponse {
    pub session_id: String,
    pub status: String,
    pub chunk_count: u32,
    pub received_chunks: Vec<u32>,
    pub missing_chunks: Vec<u32>,
}

// ─── Handlers ─────────────────────────────────────────────────────────────────

/// `POST /upload/prepare` — create a resumable upload session.
///
/// Checks for a duplicate file via `content_hash` deduplication before
/// creating the session.  If the file is already present and `skip_dedup` is
/// `false` the response includes the existing [`FileNode`] and skips upload.
#[utoipa::path(
    post,
    path = "/api/v1/upload/prepare",
    request_body = PrepareRequest,
    responses(
        (status = 200, description = "Session created or instant-complete", body = PrepareResponse),
        (status = 400, description = "Invalid request"),
    ),
    security(("bearer_auth" = [])),
    tag = "upload"
)]
pub async fn prepare(
    state: web::Data<AppState>,
    auth: AuthUser,
    body: web::Json<PrepareRequest>,
) -> Result<HttpResponse, ApiError> {
    let user_id = parse_user_id(&auth.0.sub)?;
    let chunk_size = body.chunk_size.unwrap_or(DEFAULT_CHUNK_SIZE).clamp(1, 64 * 1024 * 1024);

    // Fast-path: if the complete file hash is already in storage we can skip
    // the entire upload and just create the VFS record.
    if !body.skip_dedup {
        let existing_node = state
            .vfs
            .find_by_content_hash(&body.content_hash)
            .await
            .unwrap_or(None);

        if let Some(existing) = existing_node {
            // Instant-complete: the same content already exists in storage.
            // Return the existing FileNode directly — it already has the
            // correct file_chunks association so downloads work via its ID.
            // Creating a brand-new VFS node would require duplicating the
            // file_chunks rows while offering no real benefit for dedup.
            let session = state
                .upload_sessions
                .create(
                    &user_id,
                    &body.parent_id,
                    &body.file_name,
                    body.total_size,
                    &body.content_hash,
                    body.mime_type.clone(),
                    chunk_size,
                )
                .await?;
            state.upload_sessions.mark_complete(&session.id).await?;

            return Ok(HttpResponse::Ok().json(PrepareResponse {
                session_id: session.id,
                chunk_size: session.chunk_size,
                chunk_count: session.chunk_count,
                instant_complete: true,
                file_node: Some(existing),
            }));
        }
    }

    let session = state
        .upload_sessions
        .create(
            &user_id,
            &body.parent_id,
            &body.file_name,
            body.total_size,
            &body.content_hash,
            body.mime_type.clone(),
            chunk_size,
        )
        .await?;

    Ok(HttpResponse::Ok().json(PrepareResponse {
        chunk_size: session.chunk_size,
        chunk_count: session.chunk_count,
        session_id: session.id,
        instant_complete: false,
        file_node: None,
    }))
}

/// `GET /upload/{session_id}/status` — return missing chunk indices.
#[utoipa::path(
    get,
    path = "/api/v1/upload/{session_id}/status",
    params(("session_id" = String, Path, description = "Upload session ID")),
    responses(
        (status = 200, description = "Session status", body = SessionStatusResponse),
        (status = 404, description = "Session not found"),
    ),
    security(("bearer_auth" = [])),
    tag = "upload"
)]
pub async fn status(
    state: web::Data<AppState>,
    auth: AuthUser,
    path: web::Path<String>,
) -> Result<HttpResponse, ApiError> {
    let session_id = path.into_inner();
    let user_id = parse_user_id(&auth.0.sub)?;
    let session = state
        .upload_sessions
        .find_for_user(&session_id, &user_id)
        .await?;

    Ok(HttpResponse::Ok().json(SessionStatusResponse {
        session_id: session.id.clone(),
        status: session.status.clone(),
        chunk_count: session.chunk_count,
        missing_chunks: session.missing_chunks(),
        received_chunks: session.received_chunks,
    }))
}

/// `POST /upload/{session_id}/chunk/{index}` — upload a single chunk.
///
/// Idempotent: re-uploading an already-received chunk replaces it.
#[utoipa::path(
    post,
    path = "/api/v1/upload/{session_id}/chunk/{index}",
    params(
        ("session_id" = String, Path, description = "Upload session ID"),
        ("index" = u32, Path, description = "Zero-based chunk index"),
    ),
    request_body(content = Vec<u8>, content_type = "application/octet-stream"),
    responses(
        (status = 200, description = "Chunk accepted", body = SessionStatusResponse),
        (status = 400, description = "Chunk index out of range or size mismatch"),
        (status = 404, description = "Session not found"),
    ),
    security(("bearer_auth" = [])),
    tag = "upload"
)]
pub async fn upload_chunk(
    state: web::Data<AppState>,
    auth: AuthUser,
    path: web::Path<(String, u32)>,
    body: Bytes,
) -> Result<HttpResponse, ApiError> {
    let (session_id, index) = path.into_inner();
    let user_id = parse_user_id(&auth.0.sub)?;
    let session = state
        .upload_sessions
        .find_for_user(&session_id, &user_id)
        .await?;

    if index >= session.chunk_count {
        return Err(AppError::Validation(format!(
            "chunk index {index} out of range (chunk_count = {})",
            session.chunk_count
        ))
        .into());
    }

    // Validate chunk size: every chunk except the last must equal chunk_size.
    let is_last = index == session.chunk_count - 1;
    let expected_size = if is_last {
        // Last chunk may be smaller.
        let remainder = session.total_size % session.chunk_size;
        if remainder == 0 { session.chunk_size } else { remainder }
    } else {
        session.chunk_size
    };

    if body.len() as u64 != expected_size {
        return Err(AppError::Validation(format!(
            "chunk {index} size {} does not match expected {expected_size}",
            body.len()
        ))
        .into());
    }

    let updated = state
        .upload_sessions
        .receive_chunk(&session, index, &body)
        .await?;

    Ok(HttpResponse::Ok().json(SessionStatusResponse {
        session_id: updated.id.clone(),
        status: updated.status.clone(),
        chunk_count: updated.chunk_count,
        missing_chunks: updated.missing_chunks(),
        received_chunks: updated.received_chunks,
    }))
}

/// `POST /upload/{session_id}/complete` — assemble chunks and create VFS record.
///
/// Verifies the SHA-256 hash of the assembled file against the one provided at
/// PREPARE time, then stores chunks via [`UploadService`] and creates a
/// [`FileNode`].
#[utoipa::path(
    post,
    path = "/api/v1/upload/{session_id}/complete",
    params(("session_id" = String, Path, description = "Upload session ID")),
    responses(
        (status = 201, description = "File stored; VFS record created", body = FileNode),
        (status = 400, description = "Chunks missing or hash mismatch"),
        (status = 404, description = "Session not found"),
    ),
    security(("bearer_auth" = [])),
    tag = "upload"
)]
pub async fn complete(
    state: web::Data<AppState>,
    auth: AuthUser,
    path: web::Path<String>,
) -> Result<HttpResponse, ApiError> {
    let session_id = path.into_inner();
    let user_id = parse_user_id(&auth.0.sub)?;
    let session = state
        .upload_sessions
        .find_for_user(&session_id, &user_id)
        .await?;

    if !session.missing_chunks().is_empty() {
        return Err(AppError::Validation(format!(
            "upload incomplete: missing chunks {:?}",
            session.missing_chunks()
        ))
        .into());
    }

    // Assemble all chunk files into a contiguous buffer.
    let tmp_dir = state.upload_sessions.session_tmp_dir(&session.id);
    let assembled = assemble_chunks(&tmp_dir, session.chunk_count).await?;

    // SHA-256 integrity check.
    let actual_hash = jiezi_cloud_storage::hashing::sha256_hex(&assembled);
    if actual_hash != session.content_hash {
        return Err(AppError::Validation(format!(
            "content hash mismatch: expected {}, got {actual_hash}",
            session.content_hash
        ))
        .into());
    }

    // Store via the standard upload pipeline (CDC → backends → DB chunks).
    let file_id = FileId::new();
    let stored = state
        .upload
        .store_file(&file_id, assembled, &ReplicationPolicy::default())
        .await?;

    let parent_id = parse_file_id(&session.parent_id)?;
    let node = state
        .vfs
        .create_file_record(
            &parent_id,
            &session.file_name,
            stored.total_size,
            Some(stored.content_hash),
            session.mime_type.clone(),
            &user_id,
            // Pass the storage file_id so the VFS node ID matches the
            // file_chunks key — enabling download by VFS node ID.
            Some(file_id),
        )
        .await?;

    // Mark session complete and clean up temp files.
    state.upload_sessions.mark_complete(&session_id).await?;

    Ok(HttpResponse::Created().json(node))
}

/// `DELETE /upload/{session_id}` — cancel and remove temporary chunk files.
#[utoipa::path(
    delete,
    path = "/api/v1/upload/{session_id}",
    params(("session_id" = String, Path, description = "Upload session ID")),
    responses(
        (status = 204, description = "Session cancelled"),
        (status = 404, description = "Session not found"),
    ),
    security(("bearer_auth" = [])),
    tag = "upload"
)]
pub async fn cancel(
    state: web::Data<AppState>,
    auth: AuthUser,
    path: web::Path<String>,
) -> Result<HttpResponse, ApiError> {
    let session_id = path.into_inner();
    let user_id = parse_user_id(&auth.0.sub)?;
    // Verify ownership before cancelling.
    state
        .upload_sessions
        .find_for_user(&session_id, &user_id)
        .await?;
    state.upload_sessions.cancel(&session_id).await?;
    Ok(HttpResponse::NoContent().finish())
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
