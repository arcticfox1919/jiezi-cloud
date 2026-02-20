//! WebDAV storage backend.
//!
//! Stores chunks on any WebDAV-compliant server such as Nextcloud, ownCloud,
//! nginx with `ngx_http_dav_module`, or Apache with `mod_dav`.
//!
//! # URL convention
//!
//! `WebDavConfig::url` must point to a WebDAV *collection* (directory) that
//! the server user has write permission on.  Chunk keys (`"ab/abcdef…"`) are
//! appended to the base URL by [`reqwest_dav`]'s `start_request()`:
//!
//! ```text
//! url  =  https://nextcloud.example.com/remote.php/dav/files/alice/jiezi/
//! key  =  ab/abcdef1234…
//! obj  →  https://nextcloud.example.com/remote.php/dav/files/alice/jiezi/ab/abcdef1234…
//! ```
//!
//! # Authentication
//!
//! | Config field                  | Protocol                          |
//! |-------------------------------|-----------------------------------|
//! | `username` + `password`       | HTTP Basic (handled by reqwest_dav) |
//! | `bearer_token`                | `Authorization: Bearer <token>` (injected manually; reqwest_dav has no Bearer variant) |
//! | neither                       | `Auth::Anonymous` (no auth header) |
//!
//! # Parent-directory creation
//!
//! The first PUT for any two-character prefix (`"ab/"`, `"cd/"` …) sends a
//! `MKCOL` request to create the parent collection.  Successfully created
//! prefixes are cached in memory to avoid redundant MKCOL round-trips.

use std::collections::HashSet;
use std::ops::Range;
use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use reqwest_dav::{Auth, ClientBuilder};
use reqwest_dav::re_exports::reqwest::{self as rq};
use tokio::sync::Mutex;
use tracing::{debug, instrument, warn};

use jiezi_cloud_core::{
    error::{AppError, AppResult},
    models::backend::WebDavConfig,
    types::{BackendId, HealthStatus},
};

use crate::hashing::validate_key;

// ─── WebDavBackend ────────────────────────────────────────────────────────────

/// Storage backend that stores chunks on a WebDAV server.
///
/// Constructed via [`WebDavBackend::new`] from a [`WebDavConfig`].
pub struct WebDavBackend {
    id: BackendId,
    /// The reqwest_dav client (handles URL building + Basic/Digest auth).
    client: reqwest_dav::Client,
    /// Base URL (also stored in `client.host`), kept for `Debug` output.
    base_url: String,
    /// Bearer token, if provided.  reqwest_dav has no Bearer auth variant so
    /// we inject the `Authorization` header manually via `start_request()`.
    bearer_token: Option<String>,
    /// Cache of shard-prefix directories that have already been MKCOL'd.
    created_prefixes: Arc<Mutex<HashSet<String>>>,
}

impl WebDavBackend {
    /// Create a new [`WebDavBackend`] from a [`WebDavConfig`].
    ///
    /// Returns an error if the underlying HTTP client cannot be constructed
    /// (rare — only fails on TLS or platform issues).
    pub fn new(id: BackendId, config: &WebDavConfig) -> AppResult<Self> {
        let mut url = config.url.clone();
        if !url.ends_with('/') {
            url.push('/');
        }

        let auth = match (&config.username, &config.password) {
            (Some(user), Some(pass)) => Auth::Basic(user.clone(), pass.clone()),
            _ => Auth::Anonymous,
        };

        let client = ClientBuilder::new()
            .set_host(url.clone())
            .set_auth(auth)
            .build()
            .map_err(|e| AppError::Storage(format!("WebDAV client init failed: {e}")))?;

        Ok(Self {
            id,
            client,
            base_url: url,
            bearer_token: config.bearer_token.clone(),
            created_prefixes: Arc::new(Mutex::new(HashSet::new())),
        })
    }

    // ── Internal helpers ──────────────────────────────────────────────────────

    /// Build a [`rq::RequestBuilder`] for `method` + `path`, applying the
    /// bearer token header when configured.
    ///
    /// `path` is relative to the base URL (e.g. `"ab/abcdef…"` or `"ab/"`).
    /// Use an empty string `""` to target the base collection itself.
    async fn request(
        &self,
        method: rq::Method,
        path: &str,
    ) -> AppResult<rq::RequestBuilder> {
        let mut rb = self
            .client
            .start_request(method, path)
            .await
            .map_err(|e| AppError::Storage(format!("WebDAV request build error: {e}")))?;

        if let Some(token) = &self.bearer_token {
            rb = rb.header(rq::header::AUTHORIZATION, format!("Bearer {token}"));
        }
        Ok(rb)
    }

    /// Issue a `MKCOL` for the shard-prefix directory of `key` if we haven't
    /// done so already.  WebDAV PUT returns `409 Conflict` when the parent
    /// collection does not exist; pre-creating the prefix avoids this.
    async fn ensure_prefix(&self, key: &str) -> AppResult<()> {
        let prefix = match key.split_once('/') {
            Some((p, _)) => p.to_owned(),
            None => return Ok(()), // flat key — no parent directory needed
        };
        let prefix_path = format!("{prefix}/");

        {
            let cache = self.created_prefixes.lock().await;
            if cache.contains(&prefix_path) {
                return Ok(());
            }
        }

        debug!(prefix = %prefix_path, "MKCOL shard prefix");
        let mkcol = rq::Method::from_bytes(b"MKCOL").expect("MKCOL is a valid HTTP method");
        let resp = self
            .request(mkcol, &prefix_path)
            .await?
            .send()
            .await
            .map_err(|e| AppError::Storage(format!("WebDAV MKCOL failed: {e}")))?;

        match resp.status() {
            // 201 Created — new directory.
            // 405 Method Not Allowed — already exists (most servers).
            // 301/302 — redirect; treat as existing.
            rq::StatusCode::CREATED
            | rq::StatusCode::METHOD_NOT_ALLOWED
            | rq::StatusCode::MOVED_PERMANENTLY
            | rq::StatusCode::FOUND => {}
            other => {
                warn!(
                    status = %other,
                    prefix = %prefix_path,
                    "MKCOL returned unexpected status"
                );
            }
        }

        // Cache regardless — even a failed MKCOL should not be retried on
        // every PUT; the PUT itself will expose the real problem.
        self.created_prefixes.lock().await.insert(prefix_path);
        Ok(())
    }
}

impl std::fmt::Debug for WebDavBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebDavBackend")
            .field("id", &self.id)
            .field("base_url", &self.base_url)
            .finish_non_exhaustive()
    }
}

// ─── StorageBackend impl ──────────────────────────────────────────────────────

#[async_trait]
impl jiezi_cloud_core::traits::storage::StorageBackend for WebDavBackend {
    fn backend_id(&self) -> &BackendId {
        &self.id
    }

    #[instrument(skip(self, data), fields(backend = %self.id, key, bytes = data.len()))]
    async fn put_chunk(&self, key: &str, data: Bytes) -> AppResult<()> {
        validate_key(key)?;
        self.ensure_prefix(key).await?;

        debug!(key, bytes = data.len(), "WebDAV PUT");

        let resp = self
            .request(rq::Method::PUT, key)
            .await?
            .body(data)
            .send()
            .await
            .map_err(|e| AppError::Storage(format!("WebDAV PUT failed for key={key}: {e}")))?;

        match resp.status() {
            s if s.is_success() => Ok(()),
            rq::StatusCode::CONFLICT => Err(AppError::Storage(format!(
                "WebDAV PUT got 409 Conflict for key={key}; shard prefix may be missing"
            ))),
            other => Err(AppError::Storage(format!(
                "WebDAV PUT failed for key={key}: HTTP {other}"
            ))),
        }
    }

    #[instrument(skip(self), fields(backend = %self.id, key))]
    async fn get_chunk(&self, key: &str) -> AppResult<Bytes> {
        validate_key(key)?;
        debug!(key, "WebDAV GET");

        let resp = self
            .request(rq::Method::GET, key)
            .await?
            .send()
            .await
            .map_err(|e| AppError::Storage(format!("WebDAV GET failed for key={key}: {e}")))?;

        match resp.status() {
            rq::StatusCode::NOT_FOUND => {
                Err(AppError::NotFound(format!("chunk not found: {key}")))
            }
            s if !s.is_success() => Err(AppError::Storage(format!(
                "WebDAV GET failed for key={key}: HTTP {s}"
            ))),
            _ => resp.bytes().await.map_err(|e| {
                AppError::Storage(format!("WebDAV body read failed for key={key}: {e}"))
            }),
        }
    }

    #[instrument(skip(self), fields(backend = %self.id, key, ?range))]
    async fn get_chunk_range(&self, key: &str, range: Range<u64>) -> AppResult<Bytes> {
        validate_key(key)?;
        // HTTP Range header: "bytes=start-end_inclusive"
        let range_header = format!("bytes={}-{}", range.start, range.end.saturating_sub(1));
        debug!(key, %range_header, "WebDAV GET range");

        let resp = self
            .request(rq::Method::GET, key)
            .await?
            .header(rq::header::RANGE, &range_header)
            .send()
            .await
            .map_err(|e| {
                AppError::Storage(format!("WebDAV GET range failed for key={key}: {e}"))
            })?;

        match resp.status() {
            rq::StatusCode::NOT_FOUND => {
                Err(AppError::NotFound(format!("chunk not found: {key}")))
            }
            // 206 Partial Content (success) or 200 OK (server ignored Range header)
            rq::StatusCode::PARTIAL_CONTENT | rq::StatusCode::OK => {
                resp.bytes().await.map_err(|e| {
                    AppError::Storage(format!("WebDAV body read failed for key={key}: {e}"))
                })
            }
            other => Err(AppError::Storage(format!(
                "WebDAV GET range failed for key={key}: HTTP {other}"
            ))),
        }
    }

    #[instrument(skip(self), fields(backend = %self.id, key))]
    async fn delete_chunk(&self, key: &str) -> AppResult<()> {
        validate_key(key)?;
        debug!(key, "WebDAV DELETE");

        let resp = self
            .request(rq::Method::DELETE, key)
            .await?
            .send()
            .await
            .map_err(|e| {
                AppError::Storage(format!("WebDAV DELETE failed for key={key}: {e}"))
            })?;

        match resp.status() {
            // 204 No Content (success) or 404 Not Found (already gone — idempotent)
            rq::StatusCode::NO_CONTENT | rq::StatusCode::NOT_FOUND | rq::StatusCode::OK => Ok(()),
            other => Err(AppError::Storage(format!(
                "WebDAV DELETE failed for key={key}: HTTP {other}"
            ))),
        }
    }

    #[instrument(skip(self), fields(backend = %self.id, key))]
    async fn exists(&self, key: &str) -> AppResult<bool> {
        validate_key(key)?;

        let resp = self
            .request(rq::Method::HEAD, key)
            .await?
            .send()
            .await
            .map_err(|e| {
                AppError::Storage(format!("WebDAV HEAD failed for key={key}: {e}"))
            })?;

        Ok(resp.status().is_success())
    }

    async fn health_check(&self) -> AppResult<HealthStatus> {
        // PROPFIND with Depth: 0 on the base collection verifies connectivity
        // and authentication without listing any children.
        let propfind =
            rq::Method::from_bytes(b"PROPFIND").expect("PROPFIND is a valid HTTP method");

        let send_result = self
            .request(propfind, "")
            .await
            .map(|rb| rb.header("Depth", "0"));

        match send_result {
            Err(e) => Ok(HealthStatus::Unhealthy {
                reason: format!("WebDAV health check setup failed: {e}"),
            }),
            Ok(rb) => match rb.send().await {
                Ok(r)
                    if r.status().is_success()
                        || r.status() == rq::StatusCode::MULTI_STATUS =>
                {
                    Ok(HealthStatus::Healthy)
                }
                Ok(r) => Ok(HealthStatus::Degraded {
                    reason: format!(
                        "WebDAV PROPFIND on base collection returned HTTP {}",
                        r.status()
                    ),
                }),
                Err(e) => Ok(HealthStatus::Unhealthy {
                    reason: format!("WebDAV connectivity check failed: {e}"),
                }),
            },        }
    }
}