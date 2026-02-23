//! Concrete implementation of [`KbService`].
//!
//! # Wiring (in `main.rs`)
//!
//! ```rust,no_run
//! # use std::sync::Arc;
//! # use sea_orm::DatabaseConnection;
//! let kb: Arc<dyn KbService> = Arc::new(KbServiceImpl::new(
//!     db,
//!     vfs_service,
//!     file_reader,
//!     search_engine,
//! ));
//! ```

use std::sync::Arc;

use async_trait::async_trait;
use tracing::{debug, info, warn};

use jiezi_cloud_core::{
    error::{AppError, AppResult},
    models::file::FileNode,
    traits::{
        file_content::FileContentReader,
        kb::{KbBacklink, KbNote, KbNoteFilter, KbService},
        search::{SearchDocument, SearchEngine},
        vfs::VfsService,
    },
    types::{FileId, KbNoteId, PageRequest, PageResponse, SpaceId, UserId},
};

use crate::{
    backlink,
    repository::KbRepository,
};

/// Production implementation of [`KbService`].
pub struct KbServiceImpl {
    repo: KbRepository,
    vfs: Arc<dyn VfsService>,
    reader: Arc<dyn FileContentReader>,
    search: Arc<dyn SearchEngine>,
}

impl KbServiceImpl {
    /// Create a new service instance.
    ///
    /// - `db`      — SeaORM connection pool.
    /// - `vfs`     — VFS service for reading file metadata.
    /// - `reader`  — File content reader (provided by the storage layer).
    /// - `search`  — Full-text search engine for indexing note content.
    pub fn new(
        repo: KbRepository,
        vfs: Arc<dyn VfsService>,
        reader: Arc<dyn FileContentReader>,
        search: Arc<dyn SearchEngine>,
    ) -> Self {
        Self { repo, vfs, reader, search }
    }

    /// Internal helper used by both `register` and `reindex`.
    ///
    /// Fetches file metadata + content, parses WikiLinks, writes KB rows, and
    /// submits the document to the full-text index.
    async fn index_file(&self, file_node_id: &FileId) -> AppResult<KbNote> {
        // 1. Fetch VFS metadata to get space_id, file name, and parent info.
        let node = self.vfs.get_node(file_node_id).await?;

        // 1a. Resolve the group name from the parent directory hierarchy.
        //     Expected layout: .knowledge-base/{group}/note.md
        //     → parent = group dir, grandparent = .knowledge-base/
        let group_name: Option<String> = 'group: {
            let Some(parent_id) = node.parent_id else { break 'group None };
            let Ok(parent) = self.vfs.get_node(&parent_id).await else { break 'group None };
            let Some(gp_id) = parent.parent_id else { break 'group None };
            let Ok(gp) = self.vfs.get_node(&gp_id).await else { break 'group None };
            if gp.name == crate::consts::KB_ROOT_DIR {
                Some(parent.name)
            } else {
                None
            }
        };

        // 2. Read raw Markdown bytes from storage.
        let bytes = self.reader.read_file(file_node_id).await?;
        let markdown = std::str::from_utf8(&bytes).map_err(|e| {
            AppError::Validation(format!("note {file_node_id} is not valid UTF-8: {e}"))
        })?;

        // 3. Parse [[WikiLink]] backlinks.
        let wikilinks = backlink::extract(markdown);
        debug!(
            file = %file_node_id,
            links = wikilinks.len(),
            "parsed WikiLinks"
        );

        // 4. Upsert kb_notes row (includes group_name).
        let note_id = KbNoteId::new();
        self.repo
            .upsert_note(&note_id, file_node_id, &node.space_id, group_name.as_deref())
            .await?;

        // Re-fetch to get the real id (upsert may have found an existing row).
        let db_note = self
            .repo
            .find_by_file(file_node_id)
            .await?
            .ok_or_else(|| AppError::Internal("upsert did not create note row".into()))?;

        let actual_id: KbNoteId = db_note.id.parse::<uuid::Uuid>()
            .map(KbNoteId::from)
            .map_err(|e| AppError::Internal(e.to_string()))?;

        // 5. Replace backlinks (unresolved titles stored verbatim).
        let link_pairs: Vec<(String, String)> = wikilinks
            .into_iter()
            .map(|l| (l.target, l.display))
            .collect();
        self.repo.replace_backlinks(&actual_id, link_pairs).await?;

        // 6. Extract and record embedded asset references for GC tracking.
        let asset_ids = extract_asset_ids(markdown);
        debug!(
            file = %file_node_id,
            assets = asset_ids.len(),
            "extracted jiezi://asset/ references"
        );
        if let Err(e) = self.repo.replace_assets(&actual_id, asset_ids).await {
            warn!(error = %e, file = %file_node_id, "asset tracking update failed — continuing");
        }

        // 7. Submit to full-text search index.
        let doc = SearchDocument {
            file_id: *file_node_id,
            name: node.name.clone(),
            content: Some(markdown.to_owned()),
            tags: node.metadata.tags.clone(),
            mime_type: node.mime_type.clone(),
        };
        if let Err(e) = self.search.index_document(doc).await {
            warn!(error = %e, file = %file_node_id, "FTS indexing failed — continuing");
        }

        Ok(model_to_domain(db_note, actual_id))
    }

    /// Create a VFS directory under `parent_id` with `name`, returning the node.
    ///
    /// If creation fails with [`AppError::Conflict`] (the directory already
    /// exists), this method lists the parent's children and returns the
    /// matching node instead.  This makes the operation idempotent.
    async fn find_or_create_dir(
        &self,
        parent_id: &FileId,
        name: &str,
        owner: &UserId,
    ) -> AppResult<FileNode> {
        match self.vfs.create_directory(parent_id, name, owner).await {
            Ok(node) => Ok(node),
            Err(AppError::Conflict(_)) => {
                // Directory already exists — find and return it.
                let page = PageRequest { page: 1, per_page: 500 };
                let children = self.vfs.list_children(parent_id, &page).await?;
                children
                    .items
                    .into_iter()
                    .find(|n| n.name == name)
                    .ok_or_else(|| {
                        AppError::Internal(format!(
                            "expected directory '{name}' not found after Conflict"
                        ))
                    })
            }
            Err(e) => Err(e),
        }
    }
}

// ─── Trait implementation ─────────────────────────────────────────────────────

#[async_trait]
impl KbService for KbServiceImpl {
    async fn register(&self, file_node_id: &FileId) -> AppResult<KbNote> {
        info!(file = %file_node_id, "registering note into KB");
        let note = self.index_file(file_node_id).await?;
        info!(note = %note.id, "KB note registered");
        Ok(note)
    }

    async fn reindex(&self, file_node_id: &FileId) -> AppResult<()> {
        info!(file = %file_node_id, "re-indexing KB note");
        self.index_file(file_node_id).await?;
        Ok(())
    }

    async fn unregister(&self, file_node_id: &FileId) -> AppResult<()> {
        self.repo.delete_by_file(file_node_id).await?;
        Ok(())
    }

    async fn get_by_file(&self, file_node_id: &FileId) -> AppResult<KbNote> {
        let db = self
            .repo
            .find_by_file(file_node_id)
            .await?
            .ok_or_else(|| AppError::NotFound(format!("no KB note for file {file_node_id}")))?;
        let id: KbNoteId = db.id.parse::<uuid::Uuid>()
            .map(KbNoteId::from)
            .map_err(|e| AppError::Internal(e.to_string()))?;
        Ok(model_to_domain(db, id))
    }

    async fn list(
        &self,
        space_id: &SpaceId,
        filter: &KbNoteFilter,
        page: &PageRequest,
    ) -> AppResult<PageResponse<KbNote>> {
        // TODO(Phase 15): apply tag and FTS filters.
        let raw = self
            .repo
            .list_in_space(space_id, filter.group.as_deref(), page)
            .await?;
        let items = raw
            .items
            .into_iter()
            .map(|m| {
                let id: KbNoteId = m.id.parse::<uuid::Uuid>()
                    .map(KbNoteId::from)
                    .unwrap_or_else(|_| KbNoteId::new());
                model_to_domain(m, id)
            })
            .collect();
        Ok(PageResponse {
            items,
            total: raw.total,
            page: raw.page,
            per_page: raw.per_page,
        })
    }

    async fn backlinks_to(&self, target: &KbNoteId) -> AppResult<Vec<KbBacklink>> {
        // Resolve target note to its file name (without .md extension).
        let db_note = self
            .repo
            .find_by_id(target)
            .await?
            .ok_or_else(|| AppError::NotFound(format!("kb note {target}")))?;

        let node = self.vfs.get_node(&db_note.file_node_id.parse::<uuid::Uuid>()
            .map(FileId::from)
            .map_err(|e| AppError::Internal(e.to_string()))?)
            .await?;

        // Strip ".md" extension to get the title used in WikiLinks.
        let title = node
            .name
            .strip_suffix(".md")
            .unwrap_or(&node.name)
            .to_owned();

        let rows = self.repo.backlinks_to_title(&title).await?;
        rows.into_iter()
            .map(|r| {
                let from: KbNoteId = r.from_note_id.parse::<uuid::Uuid>()
                    .map(KbNoteId::from)
                    .map_err(|e| AppError::Internal(e.to_string()))?;
                Ok(KbBacklink {
                    from_note_id: from,
                    target_title: r.target_title,
                    anchor_text: r.anchor_text,
                })
            })
            .collect()
    }

    async fn links_from(&self, source: &KbNoteId) -> AppResult<Vec<KbBacklink>> {
        let rows = self.repo.links_from(source).await?;
        rows.into_iter()
            .map(|r| {
                let from: KbNoteId = r.from_note_id.parse::<uuid::Uuid>()
                    .map(KbNoteId::from)
                    .map_err(|e| AppError::Internal(e.to_string()))?;
                Ok(KbBacklink {
                    from_note_id: from,
                    target_title: r.target_title,
                    anchor_text: r.anchor_text,
                })
            })
            .collect()
    }

    async fn publish(&self, note_id: &KbNoteId, slug: &str) -> AppResult<KbNote> {
        let db = self.repo.set_published(note_id, slug).await?;
        let id = *note_id;
        Ok(model_to_domain(db, id))
    }

    async fn unpublish(&self, note_id: &KbNoteId) -> AppResult<KbNote> {
        let db = self.repo.set_unpublished(note_id).await?;
        let id = *note_id;
        Ok(model_to_domain(db, id))
    }

    async fn init_space(
        &self,
        _space_id: &SpaceId,
        root_id: &FileId,
        owner_id: &UserId,
    ) -> AppResult<()> {
        // Create .knowledge-base/ at the space root.
        let kb_dir = self
            .find_or_create_dir(root_id, crate::consts::KB_ROOT_DIR, owner_id)
            .await?;

        // Create .knowledge-base/.assets/ for space-level shared assets.
        self.find_or_create_dir(
            &kb_dir.id,
            crate::consts::KB_SPACE_ASSETS_DIR,
            owner_id,
        )
        .await?;

        info!(root = ?root_id, "KB directory structure initialised");
        Ok(())
    }

    async fn render_note(&self, note_id: &KbNoteId, base_url: &str) -> AppResult<String> {
        // 1. Resolve note → file_node_id.
        let db_note = self
            .repo
            .find_by_id(note_id)
            .await?
            .ok_or_else(|| AppError::NotFound(format!("kb note {note_id}")))?;

        let file_id: FileId = db_note
            .file_node_id
            .parse::<uuid::Uuid>()
            .map(FileId::from)
            .map_err(|e| AppError::Internal(e.to_string()))?;

        // 2. Read raw Markdown bytes.
        let bytes = self.reader.read_file(&file_id).await?;
        let markdown = std::str::from_utf8(&bytes).map_err(|e| {
            AppError::Validation(format!("note {note_id} is not valid UTF-8: {e}"))
        })?;

        // 3. Rewrite jiezi://asset/{file_id} → {base_url}/api/v1/files/{file_id}.
        let download_base = format!("{}/api/v1/files/", base_url.trim_end_matches('/'));
        let rewritten = markdown.replace(crate::consts::JIEZI_ASSET_SCHEME, &download_base);

        // 4. Parse Markdown and render to HTML.
        let parser =
            pulldown_cmark::Parser::new_ext(&rewritten, pulldown_cmark::Options::all());
        let mut html = String::with_capacity(rewritten.len() * 2);
        pulldown_cmark::html::push_html(&mut html, parser);
        Ok(html)
    }
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

fn model_to_domain(m: crate::entities::kb_note::Model, id: KbNoteId) -> KbNote {
    KbNote {
        id,
        file_node_id: m
            .file_node_id
            .parse::<uuid::Uuid>()
            .map(FileId::from)
            .unwrap_or_else(|_| FileId::new()),
        space_id: m
            .space_id
            .parse::<uuid::Uuid>()
            .map(SpaceId::from)
            .unwrap_or_else(|_| SpaceId::new()),
        tags: Vec::new(), // lazily loaded from kb_tags when needed
        indexed_at: m.indexed_at,
        slug: m.slug,
        published_at: m.published_at,
    }
}

/// Scan `markdown` for `jiezi://asset/<uuid>` URIs and return the file IDs.
///
/// The UUID is expected in the canonical 36-character `xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx`
/// hyphenated form immediately after the scheme prefix.  Invalid UUIDs or
/// malformed occurrences are silently skipped.
fn extract_asset_ids(markdown: &str) -> Vec<FileId> {
    let scheme = crate::consts::JIEZI_ASSET_SCHEME;
    let mut ids = Vec::new();
    let mut remaining = markdown;

    while let Some(pos) = remaining.find(scheme) {
        let after = &remaining[pos + scheme.len()..];
        // A hyphenated UUID is exactly 36 characters.
        if after.len() >= 36 {
            if let Ok(uuid) = after[..36].parse::<uuid::Uuid>() {
                ids.push(FileId::from(uuid));
            }
        }
        // Advance past the current scheme occurrence.
        remaining = &remaining[pos + scheme.len()..];
    }

    ids
}
