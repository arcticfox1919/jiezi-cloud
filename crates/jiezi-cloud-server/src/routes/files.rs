//! File system API handlers.
//!
//! All VFS metadata routes are mounted under `/api/v1/files`.
//! Upload and download routes are mounted under `/api/v1/upload` and
//! `/api/v1/download` by [`super::configure`].
//! Auth is required for every endpoint (via the [`AuthUser`] extractor).
//!
//! ## VFS metadata routes (under `/api/v1/files`)
//!
//! | Method   | Path                      | Description                              |
//! |----------|---------------------------|------------------------------------------|
//! | GET      | `/{id}`                   | Get a single node (file or dir)          |
//! | GET      | `/{id}/children`          | List a directory's contents              |
//! | POST     | `/directory`              | Create a new directory                   |
//! | PUT      | `/{id}/name`              | Rename a node                            |
//! | POST     | `/{id}/move`              | Move a node to a new parent              |
//! | POST     | `/{id}/copy`              | Copy a node to a new parent              |
//! | DELETE   | `/{id}`                   | Soft-delete (move to trash)              |
//! | POST     | `/{id}/restore`           | Restore from trash                       |
//! | DELETE   | `/{id}/permanent`         | Permanently delete a node                |
//! | GET      | `/trash`                  | List the current user's trash            |
//!
//! ## Upload routes (under `/api/v1/upload`)
//!
//! | Method   | Path                      | Description                              |
//! |----------|---------------------------|------------------------------------------|
//! | POST     | `/`                       | Upload a file (raw body, max 10 GiB)     |
//!
//! ## Download routes (under `/api/v1/download`)
//!
//! | Method   | Path                      | Description                              |
//! |----------|---------------------------|------------------------------------------|
//! | GET      | `/{id}`                   | Download file content (supports Range)   |

use std::str::FromStr;

use actix_web::{web, HttpRequest, HttpResponse};
use bytes::Bytes;
use serde::Deserialize;

use jiezi_cloud_core::error::AppError;
use jiezi_cloud_core::models::backend::ReplicationPolicy;
use jiezi_cloud_core::types::{FileId, PageRequest, UserId};

use crate::error::ApiError;
use crate::middleware::auth::AuthUser;
use crate::state::AppState;

// ─── Route registration ───────────────────────────────────────────────────────

/// Register VFS metadata routes under `/api/v1/files`.
pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg
        // Static routes must come before parameterised routes.
        .route("/directory", web::post().to(create_directory))
        .route("/trash",     web::get().to(list_trash))
        .route("/{id}",                web::get().to(get_node))
        .route("/{id}/children",       web::get().to(list_children))
        .route("/{id}/name",           web::put().to(rename))
        .route("/{id}/move",           web::post().to(move_node))
        .route("/{id}/copy",           web::post().to(copy_node))
        .route("/{id}",                web::delete().to(soft_delete))
        .route("/{id}/restore",        web::post().to(restore))
        .route("/{id}/permanent",      web::delete().to(permanent_delete));
}

/// Register upload routes under `/api/v1/upload`.
pub fn configure_upload(cfg: &mut web::ServiceConfig) {
    cfg.route("/", web::post().to(upload_file));
}

/// Register download routes under `/api/v1/download`.
pub fn configure_download(cfg: &mut web::ServiceConfig) {
    cfg.route("/{id}", web::get().to(download_file));
}

// ─── Request bodies ───────────────────────────────────────────────────────────

/// Body for `POST /files/directory`.
#[derive(Deserialize)]
struct CreateDirectoryBody {
    /// ID of the parent directory.
    parent_id: String,
    /// Name of the new directory.
    name: String,
}

/// Body for `PUT /files/{id}/name`.
#[derive(Deserialize)]
struct RenameBody {
    new_name: String,
}

/// Body for `POST /files/{id}/move`.
#[derive(Deserialize)]
struct MoveBody {
    new_parent_id: String,
}

/// Body for `POST /files/{id}/copy`.
#[derive(Deserialize)]
struct CopyBody {
    /// ID of the destination parent directory.
    new_parent_id: String,
}

/// Query parameters for `POST /api/v1/upload/`.
#[derive(Deserialize)]
struct UploadQuery {
    /// ID of the parent directory in the VFS.
    parent_id: String,
    /// Filename to store.
    name: String,
    /// Optional MIME type hint from the client.
    mime_type: Option<String>,
}

// ─── VFS metadata handlers ────────────────────────────────────────────────────

/// `GET /files/{id}` — fetch a single node.
async fn get_node(
    state: web::Data<AppState>,
    _auth: AuthUser,
    path: web::Path<String>,
) -> Result<HttpResponse, ApiError> {
    let id = parse_file_id(&path.into_inner())?;
    let node = state.vfs.get_node(&id).await?;
    Ok(HttpResponse::Ok().json(node))
}

/// `GET /files/{id}/children?page=1&per_page=20` — list a directory's contents.
async fn list_children(
    state: web::Data<AppState>,
    _auth: AuthUser,
    path: web::Path<String>,
    query: web::Query<PageRequest>,
) -> Result<HttpResponse, ApiError> {
    let id = parse_file_id(&path.into_inner())?;
    let page = query.into_inner();
    let result = state.vfs.list_children(&id, &page).await?;
    Ok(HttpResponse::Ok().json(result))
}

/// `POST /files/directory` — create a new subdirectory.
async fn create_directory(
    state: web::Data<AppState>,
    auth: AuthUser,
    body: web::Json<CreateDirectoryBody>,
) -> Result<HttpResponse, ApiError> {
    let parent_id = parse_file_id(&body.parent_id)?;
    let owner_id = parse_user_id(&auth.0.sub)?;
    let node = state
        .vfs
        .create_directory(&parent_id, &body.name, &owner_id)
        .await?;
    Ok(HttpResponse::Created().json(node))
}

/// `PUT /files/{id}/name` — rename a node.
async fn rename(
    state: web::Data<AppState>,
    _auth: AuthUser,
    path: web::Path<String>,
    body: web::Json<RenameBody>,
) -> Result<HttpResponse, ApiError> {
    let id = parse_file_id(&path.into_inner())?;
    let node = state.vfs.rename(&id, &body.new_name).await?;
    Ok(HttpResponse::Ok().json(node))
}

/// `POST /files/{id}/move` — move a node to a different parent.
async fn move_node(
    state: web::Data<AppState>,
    _auth: AuthUser,
    path: web::Path<String>,
    body: web::Json<MoveBody>,
) -> Result<HttpResponse, ApiError> {
    let id = parse_file_id(&path.into_inner())?;
    let new_parent = parse_file_id(&body.new_parent_id)?;
    let node = state.vfs.move_node(&id, &new_parent).await?;
    Ok(HttpResponse::Ok().json(node))
}

/// `DELETE /files/{id}` — soft-delete (move to trash).
async fn soft_delete(
    state: web::Data<AppState>,
    _auth: AuthUser,
    path: web::Path<String>,
) -> Result<HttpResponse, ApiError> {
    let id = parse_file_id(&path.into_inner())?;
    state.vfs.soft_delete(&id).await?;
    Ok(HttpResponse::NoContent().finish())
}

/// `POST /files/{id}/restore` — restore from trash.
async fn restore(
    state: web::Data<AppState>,
    _auth: AuthUser,
    path: web::Path<String>,
) -> Result<HttpResponse, ApiError> {
    let id = parse_file_id(&path.into_inner())?;
    let node = state.vfs.restore(&id).await?;
    Ok(HttpResponse::Ok().json(node))
}

/// `DELETE /files/{id}/permanent` — permanently destroy a node.
///
/// Irreversible.  The node and all its descendants are removed from the
/// database.  Content deduplication means storage chunks are only freed
/// by a separate garbage-collection pass.
async fn permanent_delete(
    state: web::Data<AppState>,
    _auth: AuthUser,
    path: web::Path<String>,
) -> Result<HttpResponse, ApiError> {
    let id = parse_file_id(&path.into_inner())?;
    state.vfs.permanent_delete(&id).await?;
    Ok(HttpResponse::NoContent().finish())
}

/// `POST /files/{id}/copy` — copy a node (and subtree) under a new parent.
///
/// Returns the root of the copied subtree with a new ID.
async fn copy_node(
    state: web::Data<AppState>,
    auth:  AuthUser,
    path:  web::Path<String>,
    body:  web::Json<CopyBody>,
) -> Result<HttpResponse, ApiError> {
    let id         = parse_file_id(&path.into_inner())?;
    let new_parent = parse_file_id(&body.new_parent_id)?;
    let owner_id   = parse_user_id(&auth.0.sub)?;
    let node = state.vfs.copy_node(&id, &new_parent, &owner_id).await?;
    Ok(HttpResponse::Created().json(node))
}

/// `GET /files/trash` — list all soft-deleted nodes owned by the caller.
async fn list_trash(
    state: web::Data<AppState>,
    auth:  AuthUser,
) -> Result<HttpResponse, ApiError> {
    let owner_id = parse_user_id(&auth.0.sub)?;
    let nodes = state.vfs.list_trash(&owner_id).await?;
    Ok(HttpResponse::Ok().json(nodes))
}

// ─── Upload / download handlers ───────────────────────────────────────────────

/// `POST /upload/?parent_id=…&name=…` — upload a file.
///
/// Accepts the raw file bytes in the request body.  The pipeline:
/// 1. Receives raw bytes (Actix request body limit applies).
/// 2. Runs FastCDC chunking.
/// 3. Writes chunks to all configured storage backends (dedup-aware).
/// 4. Creates a `FileNode` record in the VFS under `parent_id`.
///
/// Returns the new [`FileNode`] as JSON with `201 Created`.
async fn upload_file(
    state: web::Data<AppState>,
    auth:  AuthUser,
    query: web::Query<UploadQuery>,
    body:  Bytes,
) -> Result<HttpResponse, ApiError> {
    let parent_id = parse_file_id(&query.parent_id)?;
    let owner_id  = parse_user_id(&auth.0.sub)?;

    // Assign a new VFS file ID upfront so the upload service can record it.
    let file_id = jiezi_cloud_core::types::FileId::new();

    // Store bytes → backends, persist chunk records.
    let stored = state
        .upload
        .store_file(&file_id, body, &ReplicationPolicy::default())
        .await?;

    // Create the VFS metadata node.
    let node = state
        .vfs
        .create_file_record(
            &parent_id,
            &query.name,
            stored.total_size,
            Some(stored.content_hash),
            query.mime_type.clone(),
            &owner_id,
        )
        .await
        .map_err(|e| {
            // If the VFS record fails, the orphaned chunk records will be
            // cleaned up by the future garbage-collecto run.
            e
        })?;

    Ok(HttpResponse::Created().json(node))
}

/// `GET /download/{id}` — download file content.
///
/// Supports the `Range` header for partial content delivery (HTTP 206).
/// Full files are returned with `200 OK`.
async fn download_file(
    state:   web::Data<AppState>,
    _auth:   AuthUser,
    path:    web::Path<String>,
    req:     HttpRequest,
) -> Result<HttpResponse, ApiError> {
    let file_id = parse_file_id(&path.into_inner())?;

    // Parse optional Range header: "bytes=start-end"
    if let Some(range_header) = req.headers().get("Range") {
        if let Ok(range_str) = range_header.to_str() {
            if let Some(bytes_range) = range_str.strip_prefix("bytes=") {
                let parts: Vec<&str> = bytes_range.splitn(2, '-').collect();
                if parts.len() == 2 {
                    let start: Option<u64> = parts[0].parse().ok();
                    let end:   Option<u64> = parts[1].parse().ok();

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

    // Full file download.
    let data = state.download.read_file(&file_id).await?;
    Ok(HttpResponse::Ok()
        .content_type("application/octet-stream")
        .body(data))
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
