//! Full-text search engine trait and supporting types.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::AppResult;
use crate::types::{FileId, PageResponse};

// ─── Search types ─────────────────────────────────────────────────────────────

/// A document submitted to the search index.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchDocument {
    /// The file node this document represents.
    pub file_id: FileId,
    /// File or directory name (always indexed).
    pub name: String,
    /// Extracted text content (e.g. from PDF or Office documents).
    pub content: Option<String>,
    /// User-defined tags attached to the file.
    #[serde(default)]
    pub tags: Vec<String>,
    /// MIME type for filtering (e.g. `"application/pdf"`).
    pub mime_type: Option<String>,
}

/// A structured search query issued by a client.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchQuery {
    /// Query string (may include field-specific operators like `name:report`).
    pub q: String,
    /// 1-based page number.
    #[serde(default = "default_page")]
    pub page: u32,
    /// Items per page.
    #[serde(default = "default_per_page")]
    pub per_page: u32,
    /// Restrict results to a specific MIME type prefix (e.g. `"image/"`).
    pub mime_filter: Option<String>,
}

fn default_page() -> u32 {
    1
}
fn default_per_page() -> u32 {
    20
}

/// A single result item returned from a search.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchHit {
    /// The matching file node.
    pub file_id: FileId,
    /// File name.
    pub name: String,
    /// HTML-highlighted snippet of the matching content region.
    pub snippet: Option<String>,
    /// Relevance score assigned by the search engine (higher = more relevant).
    pub score: f32,
}

/// Paginated search result.
pub type SearchResult = PageResponse<SearchHit>;

// ─── SearchEngine trait ───────────────────────────────────────────────────────

/// Full-text search engine contract.
///
/// The concrete implementation in `jiezi-cloud-search` uses the Tantivy
/// library.  A null implementation (no-op) can be used in tests that are
/// not testing search functionality.
#[async_trait]
pub trait SearchEngine: Send + Sync {
    /// Index or re-index a document.
    ///
    /// If a document with the same `file_id` already exists it is overwritten.
    ///
    /// # Errors
    ///
    /// - [`AppError::Internal`] if the index is corrupted or unavailable.
    async fn index_document(&self, doc: SearchDocument) -> AppResult<()>;

    /// Execute a search query and return a paginated result set.
    ///
    /// # Errors
    ///
    /// - [`AppError::Validation`] if the query syntax is invalid.
    /// - [`AppError::Internal`] on index errors.
    async fn search(&self, query: &SearchQuery) -> AppResult<SearchResult>;

    /// Remove a document from the index by its file ID.
    ///
    /// This is a no-op if the document does not exist.
    ///
    /// # Errors
    ///
    /// - [`AppError::Internal`] on index errors.
    async fn delete_document(&self, id: &FileId) -> AppResult<()>;

    /// Delete all documents and rebuild the index from scratch.
    ///
    /// This is a long-running, resource-intensive operation.  Prefer running
    /// it as a background task.
    ///
    /// # Errors
    ///
    /// - [`AppError::Internal`] on index errors.
    async fn rebuild_index(&self) -> AppResult<()>;
}

// ─── NoopSearchEngine ─────────────────────────────────────────────────────────

/// A no-op [`SearchEngine`] that silently discards all operations.
///
/// Useful for running the server before a concrete search backend is
/// configured, and in integration tests that are not exercising search
/// functionality.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoopSearchEngine;

#[async_trait]
impl SearchEngine for NoopSearchEngine {
    async fn index_document(&self, _doc: SearchDocument) -> AppResult<()> {
        Ok(())
    }

    async fn search(&self, query: &SearchQuery) -> AppResult<SearchResult> {
        Ok(SearchResult::new(vec![], 0, query.page, query.per_page))
    }

    async fn delete_document(&self, _id: &FileId) -> AppResult<()> {
        Ok(())
    }

    async fn rebuild_index(&self) -> AppResult<()> {
        Ok(())
    }
}
