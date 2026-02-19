//! Full-text search module for Jiezi Cloud.
//!
//! Implements [`jiezi_cloud_core::traits::search::SearchEngine`] using the
//! Tantivy search library.  Chinese tokenization is provided via `jieba-rs`.

pub mod indexer;
pub mod query;
pub mod service;
