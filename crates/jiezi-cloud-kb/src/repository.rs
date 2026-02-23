//! Database access layer for the knowledge-base tables.
//!
//! All queries are expressed through SeaORM — no raw SQL.

use chrono::Utc;
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, DatabaseConnection, EntityTrait,
    PaginatorTrait, QueryFilter, QueryOrder, QuerySelect,
};

use jiezi_cloud_core::{
    error::{AppError, AppResult},
    types::{FileId, KbNoteId, PageRequest, PageResponse, SpaceId},
};

use crate::entities::{
    kb_asset, kb_backlink, kb_note, kb_tag,
};

/// Thin database-access wrapper for KB tables.
#[derive(Clone)]
pub struct KbRepository {
    db: DatabaseConnection,
}

impl KbRepository {
    /// Create a new repository backed by `db`.
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }

    // ── kb_notes ──────────────────────────────────────────────────────────────

    /// Insert or replace a `kb_notes` row.
    ///
    /// `group_name` is the name of the VFS group directory that directly
    /// contains the note file (e.g. `"Daily Notes"`).  Pass `None` for notes
    /// outside the standard `.knowledge-base/{group}/` layout.
    pub async fn upsert_note(
        &self,
        id: &KbNoteId,
        file_node_id: &FileId,
        space_id: &SpaceId,
        group_name: Option<&str>,
    ) -> AppResult<()> {
        let model = kb_note::ActiveModel {
            id: Set(id.to_string()),
            file_node_id: Set(file_node_id.to_string()),
            space_id: Set(space_id.to_string()),
            indexed_at: Set(Utc::now()),
            slug: Set(None),
            published_at: Set(None),
            group_name: Set(group_name.map(str::to_owned)),
        };
        kb_note::Entity::insert(model)
            .on_conflict(
                sea_orm::sea_query::OnConflict::column(kb_note::Column::FileNodeId)
                    .update_columns([
                        kb_note::Column::IndexedAt,
                        kb_note::Column::GroupName,
                    ])
                    .to_owned(),
            )
            .exec(&self.db)
            .await
            .map_err(|e| AppError::Internal(e.to_string()))?;
        Ok(())
    }

    /// Find a KB note by its VFS file node ID.
    pub async fn find_by_file(
        &self,
        file_node_id: &FileId,
    ) -> AppResult<Option<kb_note::Model>> {
        kb_note::Entity::find()
            .filter(kb_note::Column::FileNodeId.eq(file_node_id.to_string()))
            .one(&self.db)
            .await
            .map_err(|e| AppError::Internal(e.to_string()))
    }

    /// Find a KB note by its KB-layer primary key.
    pub async fn find_by_id(&self, id: &KbNoteId) -> AppResult<Option<kb_note::Model>> {
        kb_note::Entity::find_by_id(id.to_string())
            .one(&self.db)
            .await
            .map_err(|e| AppError::Internal(e.to_string()))
    }

    /// List notes in a space, ordered by `indexed_at DESC`, paginated.
    ///
    /// When `group_name` is `Some`, only notes with a matching `group_name`
    /// column are returned.
    pub async fn list_in_space(
        &self,
        space_id: &SpaceId,
        group_name: Option<&str>,
        page: &PageRequest,
    ) -> AppResult<PageResponse<kb_note::Model>> {
        let per_page = page.per_page.max(1) as u64;
        let offset = ((page.page.max(1) - 1) as u64) * per_page;

        // Build two queries (count + fetch) with the same optional group filter.
        let mut count_q = kb_note::Entity::find()
            .filter(kb_note::Column::SpaceId.eq(space_id.to_string()));
        let mut items_q = kb_note::Entity::find()
            .filter(kb_note::Column::SpaceId.eq(space_id.to_string()));

        if let Some(g) = group_name {
            count_q = count_q.filter(kb_note::Column::GroupName.eq(g));
            items_q = items_q.filter(kb_note::Column::GroupName.eq(g));
        }

        let total = count_q
            .count(&self.db)
            .await
            .map_err(|e| AppError::Internal(e.to_string()))?;

        let items = items_q
            .order_by_desc(kb_note::Column::IndexedAt)
            .limit(per_page)
            .offset(offset)
            .all(&self.db)
            .await
            .map_err(|e| AppError::Internal(e.to_string()))?;

        Ok(PageResponse {
            items,
            total,
            page: page.page,
            per_page: page.per_page,
        })
    }

    /// Delete a KB note by its VFS file node ID.
    ///
    /// Cascades to `kb_backlinks` and `kb_tags` via FK `ON DELETE CASCADE`.
    pub async fn delete_by_file(&self, file_node_id: &FileId) -> AppResult<()> {
        kb_note::Entity::delete_many()
            .filter(kb_note::Column::FileNodeId.eq(file_node_id.to_string()))
            .exec(&self.db)
            .await
            .map_err(|e| AppError::Internal(e.to_string()))?;
        Ok(())
    }

    /// Update `slug` and `published_at` for a note.
    pub async fn set_published(
        &self,
        note_id: &KbNoteId,
        slug: &str,
    ) -> AppResult<kb_note::Model> {
        let model = self
            .find_by_id(note_id)
            .await?
            .ok_or_else(|| AppError::NotFound(format!("kb note {note_id}")))?;

        let mut active: kb_note::ActiveModel = model.into();
        active.slug = Set(Some(slug.to_owned()));
        active.published_at = Set(Some(Utc::now()));
        active
            .update(&self.db)
            .await
            .map_err(|e| AppError::Internal(e.to_string()))
    }

    /// Clear `slug` and `published_at`, reverting a note to private.
    pub async fn set_unpublished(&self, note_id: &KbNoteId) -> AppResult<kb_note::Model> {
        let model = self
            .find_by_id(note_id)
            .await?
            .ok_or_else(|| AppError::NotFound(format!("kb note {note_id}")))?;

        let mut active: kb_note::ActiveModel = model.into();
        active.slug = Set(None);
        active.published_at = Set(None);
        active
            .update(&self.db)
            .await
            .map_err(|e| AppError::Internal(e.to_string()))
    }

    // ── kb_backlinks ──────────────────────────────────────────────────────────

    /// Replace all backlinks originating **from** `from_note_id`.
    ///
    /// Deletes existing rows, then inserts the new set atomically.
    /// Each tuple is `(target_title, anchor_text)` — raw unresolved WikiLink data.
    pub async fn replace_backlinks(
        &self,
        from_note_id: &KbNoteId,
        links: Vec<(String, String)>, // (target_title, anchor_text)
    ) -> AppResult<()> {
        // Delete old outgoing links from this note.
        kb_backlink::Entity::delete_many()
            .filter(kb_backlink::Column::FromNoteId.eq(from_note_id.to_string()))
            .exec(&self.db)
            .await
            .map_err(|e| AppError::Internal(e.to_string()))?;

        if links.is_empty() {
            return Ok(());
        }

        let rows: Vec<kb_backlink::ActiveModel> = links
            .into_iter()
            .map(|(target, anchor)| kb_backlink::ActiveModel {
                from_note_id: Set(from_note_id.to_string()),
                target_title: Set(target),
                anchor_text: Set(anchor),
            })
            .collect();

        kb_backlink::Entity::insert_many(rows)
            .exec(&self.db)
            .await
            .map_err(|e| AppError::Internal(e.to_string()))?;
        Ok(())
    }

    /// Return all backlinks that point *to* the note whose title is `title`.
    ///
    /// This implements the lazy-resolution strategy: WikiLinks store the raw
    /// `[[Title]]` string and resolution happens here at query time.
    pub async fn backlinks_to_title(
        &self,
        title: &str,
    ) -> AppResult<Vec<kb_backlink::Model>> {
        kb_backlink::Entity::find()
            .filter(kb_backlink::Column::TargetTitle.eq(title))
            .all(&self.db)
            .await
            .map_err(|e| AppError::Internal(e.to_string()))
    }

    /// Return all backlinks that originate *from* `from_note_id` (outgoing links).
    pub async fn links_from(
        &self,
        from_note_id: &KbNoteId,
    ) -> AppResult<Vec<kb_backlink::Model>> {
        kb_backlink::Entity::find()
            .filter(kb_backlink::Column::FromNoteId.eq(from_note_id.to_string()))
            .all(&self.db)
            .await
            .map_err(|e| AppError::Internal(e.to_string()))
    }

    // ── kb_tags ───────────────────────────────────────────────────────────────

    /// Replace all tags for a note.
    pub async fn replace_tags(
        &self,
        note_id: &KbNoteId,
        tags: Vec<String>,
    ) -> AppResult<()> {
        kb_tag::Entity::delete_many()
            .filter(kb_tag::Column::NoteId.eq(note_id.to_string()))
            .exec(&self.db)
            .await
            .map_err(|e| AppError::Internal(e.to_string()))?;

        if tags.is_empty() {
            return Ok(());
        }

        let rows: Vec<kb_tag::ActiveModel> = tags
            .into_iter()
            .map(|t| kb_tag::ActiveModel {
                note_id: Set(note_id.to_string()),
                tag: Set(t),
            })
            .collect();

        kb_tag::Entity::insert_many(rows)
            .exec(&self.db)
            .await
            .map_err(|e| AppError::Internal(e.to_string()))?;
        Ok(())
    }

    /// Return all tags for a note.
    pub async fn tags_for(&self, note_id: &KbNoteId) -> AppResult<Vec<String>> {
        let rows = kb_tag::Entity::find()
            .filter(kb_tag::Column::NoteId.eq(note_id.to_string()))
            .all(&self.db)
            .await
            .map_err(|e| AppError::Internal(e.to_string()))?;
        Ok(rows.into_iter().map(|r| r.tag).collect())
    }

    // ── kb_assets ─────────────────────────────────────────────────────────────

    /// Replace the complete set of embedded asset references for a note.
    ///
    /// Deletes all existing `kb_assets` rows for `note_id`, then inserts the
    /// new set.  An empty `file_ids` list simply clears all references.
    pub async fn replace_assets(
        &self,
        note_id: &KbNoteId,
        file_ids: Vec<FileId>,
    ) -> AppResult<()> {
        kb_asset::Entity::delete_many()
            .filter(kb_asset::Column::NoteId.eq(note_id.to_string()))
            .exec(&self.db)
            .await
            .map_err(|e| AppError::Internal(e.to_string()))?;

        if file_ids.is_empty() {
            return Ok(());
        }

        let rows: Vec<kb_asset::ActiveModel> = file_ids
            .into_iter()
            .map(|fid| kb_asset::ActiveModel {
                note_id: Set(note_id.to_string()),
                file_id: Set(fid.to_string()),
            })
            .collect();

        kb_asset::Entity::insert_many(rows)
            .exec(&self.db)
            .await
            .map_err(|e| AppError::Internal(e.to_string()))?;
        Ok(())
    }

    /// Return the VFS file IDs of all assets embedded in `note_id`.
    pub async fn assets_for(&self, note_id: &KbNoteId) -> AppResult<Vec<String>> {
        let rows = kb_asset::Entity::find()
            .filter(kb_asset::Column::NoteId.eq(note_id.to_string()))
            .all(&self.db)
            .await
            .map_err(|e| AppError::Internal(e.to_string()))?;
        Ok(rows.into_iter().map(|r| r.file_id).collect())
    }
}
