//! File system API handlers.
//!
//! All routes are mounted under `/api/v1/files` by [`super::configure`].
//! Auth is required for every endpoint (via the [`AuthUser`] extractor).
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
