//! File system API handlers.
//!
//! All routes are mounted under `/api/v1/files` by [`super::configure`].
//! Auth is required for every endpoint (via the [`AuthUser`] extractor).
//!
//! | Method   | Path                      | Description                        |
//! |----------|---------------------------|------------------------------------|
//! | GET      | `/{id}`                   | Get a single node (file or dir)    |
//! | GET      | `/{id}/children`          | List a directory's contents        |
//! | POST     | `/directory`              | Create a new directory             |
//! | PUT      | `/{id}/name`              | Rename a node                      |
//! | POST     | `/{id}/move`              | Move a node to a new parent        |
//! | DELETE   | `/{id}`                   | Soft-delete (move to trash)        |
//! | POST     | `/{id}/restore`           | Restore from trash                 |
//!
//! # TODO
//!
//! - `TODO(Phase 5 — tests)`: add handler tests using `actix_web::test`.
//! - `TODO(upload)`: `POST /files` to create a file node after chunks are uploaded.
//! - `TODO(Phase 7)`: upload session preparation endpoint.

use std::str::FromStr;

use actix_web::{web, HttpResponse};
use serde::Deserialize;

use jiezi_cloud_core::error::AppError;
use jiezi_cloud_core::types::{FileId, PageRequest, UserId};

use crate::error::ApiError;
use crate::middleware::auth::AuthUser;
use crate::state::AppState;

// ─── Route registration ───────────────────────────────────────────────────────

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg
        // Static route must come before parameterised routes to avoid conflicts.
        .route("/directory", web::post().to(create_directory))
        .route("/{id}", web::get().to(get_node))
        .route("/{id}/children", web::get().to(list_children))
        .route("/{id}/name", web::put().to(rename))
        .route("/{id}/move", web::post().to(move_node))
        .route("/{id}", web::delete().to(soft_delete))
        .route("/{id}/restore", web::post().to(restore));
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

// ─── Handlers ─────────────────────────────────────────────────────────────────

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
