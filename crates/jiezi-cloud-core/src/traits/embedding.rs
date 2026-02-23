//! Vector embedding provider trait.
//!
//! Decouples the KB and AI layers from any concrete LLM provider (OpenAI,
//! Ollama, local model, …).  The server can swap providers by changing the
//! concrete type injected at startup without touching KB code.

use async_trait::async_trait;

use crate::error::AppResult;

// ─── Embedding types ──────────────────────────────────────────────────────────

/// A dense floating-point vector produced by an embedding model.
///
/// The length (dimensionality) depends on the model:
/// - `text-embedding-3-small` → 1536 dimensions
/// - `nomic-embed-text`       → 768 dimensions
pub type EmbeddingVector = Vec<f32>;

// ─── Provider trait ───────────────────────────────────────────────────────────

/// Contract for a text-embedding backend.
///
/// Implementations live in `jiezi-cloud-ai`.  This trait is defined here so
/// that `jiezi-cloud-kb` can depend on it without depending on any concrete AI
/// crate.
#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    /// Human-readable identifier for this provider/model combination.
    ///
    /// Stored alongside each embedding vector so that vectors from different
    /// models can be kept and queried separately.
    ///
    /// Examples: `"openai/text-embedding-3-small"`, `"ollama/nomic-embed-text"`.
    fn model_id(&self) -> &str;

    /// Dimensionality of the vectors this model produces.
    fn dimensions(&self) -> usize;

    /// Embed a single text string into a dense vector.
    ///
    /// The input is typically a Markdown document or a paragraph-level chunk.
    /// Long documents should be split by the caller before embedding.
    ///
    /// # Errors
    ///
    /// - [`AppError::Internal`] on network failure, rate-limit, or model error.
    async fn embed(&self, text: &str) -> AppResult<EmbeddingVector>;

    /// Embed multiple texts in one batch (default: sequential fallback).
    ///
    /// Providers that support native batching should override this for
    /// efficiency.
    async fn embed_batch(&self, texts: &[&str]) -> AppResult<Vec<EmbeddingVector>> {
        let mut results = Vec::with_capacity(texts.len());
        for text in texts {
            results.push(self.embed(text).await?);
        }
        Ok(results)
    }
}
