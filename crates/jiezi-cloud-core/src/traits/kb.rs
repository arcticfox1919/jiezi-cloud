//! Knowledge-base service trait.
//!
//! The KB layer sits **above** the VFS and storage layers.  It indexes `.md`
//! files that already exist in the VFS and enriches them with application-layer
//! metadata: tags, backlinks and (later) vector embeddings.
//!
//! # Invariant
//!
//! A [`KbNote`] record is always a *projection* of an existing [`FileId`].
//! The Markdown bytes live in the VFS file; the KB only stores metadata *about*
//! that file.  Deleting the KB record never deletes the underlying file, and
//! vice-versa: deleting the file triggers cleanup of the KB record via a domain
//! event.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::AppResult;
use crate::types::{FileId, KbNoteId, PageRequest, PageResponse, SpaceId, UserId};

// ─── Domain types ─────────────────────────────────────────────────────────────

/// Application-layer metadata for a Markdown file that has been indexed into
/// the knowledge base.
///
/// The actual Markdown bytes live in the VFS file referenced by `file_node_id`.
/// This struct stores only the derived/annotated metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KbNote {
    /// Unique KB-layer identifier (UUIDv7).
    pub id: KbNoteId,

    /// The VFS file that backs this note.
    ///
    /// `UNIQUE` in the database — one file maps to at most one KB note.
    pub file_node_id: FileId,

    /// Which space this note belongs to (propagated from the [`FileNode`]).
    pub space_id: SpaceId,

    /// Application-level semantic tags (distinct from [`FileMetadata::tags`]).
    pub tags: Vec<String>,

    /// When this note was last indexed or re-indexed.
    pub indexed_at: DateTime<Utc>,

    /// Optional public slug used when the note is published as a blog post.
    ///
    /// `None` = private / unlisted.  Must be unique per space when set.
    pub slug: Option<String>,

    /// When `Some`, the note is publicly accessible at the slug URL.
    pub published_at: Option<DateTime<Utc>>,
}

/// A directed link from one KB note to another, parsed from `[[WikiLink]]`
/// syntax in the Markdown source.
///
/// The `target_title` is the **raw text** inside `[[…]]`.  Resolution to a
/// concrete [`KbNoteId`] is done lazily at query time by matching the title
/// against VFS file names (`target_title + ".md"`).  This avoids write-time
/// resolution failures when the target note hasn't been indexed yet.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KbBacklink {
    /// The note that contains the `[[WikiLink]]`.
    pub from_note_id: KbNoteId,

    /// Raw WikiLink target as written by the author, e.g. `"Rust ownership"`.
    pub target_title: String,

    /// Display alias from `[[Target|Alias]]`.  Equals `target_title` when no
    /// alias is present.
    pub anchor_text: String,
}

/// Query filter for listing or searching KB notes.
#[derive(Debug, Clone, Default)]
pub struct KbNoteFilter {
    /// Restrict to notes with this tag.
    pub tag: Option<String>,

    /// Free-text search delegated to the full-text engine.
    pub q: Option<String>,

    /// Only return published notes (non-`None` `published_at`).
    pub published_only: bool,

    /// Restrict to notes inside this group (VFS sub-directory name).
    ///
    /// Corresponds to the folder directly containing the `.md` file under
    /// `.knowledge-base/`, e.g. `"Daily Notes"`.
    pub group: Option<String>,
}

// ─── Service trait ────────────────────────────────────────────────────────────

/// Knowledge-base service contract.
///
/// All methods are **idempotent**: calling `register` twice on the same file
/// is equivalent to calling it once plus `reindex`.
#[async_trait]
pub trait KbService: Send + Sync {
    /// Register a VFS file as a KB note.
    ///
    /// Called automatically when a `.md` file is uploaded.  Reads the file
    /// content from storage, parses `[[WikiLink]]` backlinks, upserts the
    /// `kb_notes` record, and submits the content to the full-text index.
    ///
    /// # Errors
    ///
    /// - [`AppError::NotFound`] if `file_node_id` does not exist in the VFS.
    /// - [`AppError::Internal`] on storage or database failure.
    async fn register(&self, file_node_id: &FileId) -> AppResult<KbNote>;

    /// Re-parse and re-index a note after its Markdown content has changed.
    ///
    /// Called automatically on [`DomainEvent::FileUpdated`].
    ///
    /// # Errors
    ///
    /// - [`AppError::NotFound`] if the KB note for `file_node_id` does not
    ///   exist (the caller should use `register` in that case).
    async fn reindex(&self, file_node_id: &FileId) -> AppResult<()>;

    /// Remove KB metadata for a file that has been permanently deleted.
    ///
    /// Safe to call even if no KB note exists for the given file (no-op).
    ///
    /// # Errors
    ///
    /// - [`AppError::Internal`] on database failure.
    async fn unregister(&self, file_node_id: &FileId) -> AppResult<()>;

    /// Retrieve a KB note by its file node ID.
    ///
    /// # Errors
    ///
    /// - [`AppError::NotFound`] if no KB note is registered for this file.
    async fn get_by_file(&self, file_node_id: &FileId) -> AppResult<KbNote>;

    /// List and optionally filter KB notes within a space.
    async fn list(
        &self,
        space_id: &SpaceId,
        filter: &KbNoteFilter,
        page: &PageRequest,
    ) -> AppResult<PageResponse<KbNote>>;

    /// Return all notes that contain a `[[WikiLink]]` pointing at `target`.
    ///
    /// This is the "backlinks to me" query shown in knowledge-graph applications.
    async fn backlinks_to(&self, target: &KbNoteId) -> AppResult<Vec<KbBacklink>>;

    /// Return all outgoing WikiLinks from `source`.
    async fn links_from(&self, source: &KbNoteId) -> AppResult<Vec<KbBacklink>>;

    /// Publish a note as a blog post by setting its slug and `published_at`.
    ///
    /// # Errors
    ///
    /// - [`AppError::NotFound`] if the note does not exist.
    /// - [`AppError::Conflict`] if `slug` is already taken in this space.
    async fn publish(&self, note_id: &KbNoteId, slug: &str) -> AppResult<KbNote>;

    /// Retract a published note back to private (clears `slug` + `published_at`).
    ///
    /// # Errors
    ///
    /// - [`AppError::NotFound`] if the note does not exist.
    async fn unpublish(&self, note_id: &KbNoteId) -> AppResult<KbNote>;

    /// Bootstrap the `.knowledge-base/` folder hierarchy inside a space.
    ///
    /// Creates the following VFS directories under `root_id` when they do not
    /// already exist:
    /// - `.knowledge-base/`   — the KB root directory
    /// - `.knowledge-base/.assets/`  — space-level shared asset folder
    ///
    /// Safe to call more than once (idempotent — already-existing directories
    /// are silently skipped).
    ///
    /// # Parameters
    ///
    /// - `space_id`  — ID of the space being initialised (stored for context).
    /// - `root_id`   — VFS node ID of the space's root directory.
    /// - `owner_id`  — user who owns the space (used as VFS directory owner).
    ///
    /// # Errors
    ///
    /// - [`AppError::Internal`] on unexpected VFS or database failure.
    async fn init_space(
        &self,
        space_id: &SpaceId,
        root_id: &FileId,
        owner_id: &UserId,
    ) -> AppResult<()>;

    /// Render a note's Markdown to HTML.
    ///
    /// Rewrites every `jiezi://asset/<file_id>` URI to an HTTP download URL
    /// before parsing, so that the resulting HTML contains ordinary `<img>`
    /// tags that browsers can fetch directly.
    ///
    /// The rewritten URL pattern is:
    /// ```text
    /// {base_url}/api/v1/files/{file_id}
    /// ```
    ///
    /// # Errors
    ///
    /// - [`AppError::NotFound`] if `note_id` is not registered.
    /// - [`AppError::Internal`] on storage read failure.
    async fn render_note(&self, note_id: &KbNoteId, base_url: &str) -> AppResult<String>;
}
