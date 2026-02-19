//! AI assistant module for Jiezi Cloud.
//!
//! Provides RAG-based retrieval, document embedding, auto-summarization, and
//! intelligent file classification via pluggable LLM providers.

pub mod embedding;
pub mod provider;
pub mod rag;
pub mod summary;
