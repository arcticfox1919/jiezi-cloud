//! HTTP route configuration for the `/api/v1` namespace.

pub mod admin;
pub mod auth;
pub mod download_token;
pub mod files;
pub mod resumable_upload;
pub mod setup;
pub mod sse;

use actix_web::web;

/// Register all `/api/v1` routes onto `cfg`.
///
/// Called from `main.rs` when constructing the [`actix_web::App`].
pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.service(
        web::scope("/api/v1")
            // First-run setup wizard (always reachable; bypasses setup guard).
            .service(web::scope("/setup").configure(setup::configure))
            .service(web::scope("/auth").configure(auth::configure))
            .service(web::scope("/files").configure(files::configure))
            // Upload pipeline:
            //   POST /api/v1/upload/          — simple single-shot upload
            //   POST /api/v1/upload/prepare   — resumable session prepare
            //   …etc.
            .service(
                web::scope("/upload")
                    .configure(files::configure_upload)
                    .configure(resumable_upload::configure_resumable),
            )
            // Download pipeline:
            //   GET  /api/v1/download/{id}           — authenticated full/range download
            //   GET  /api/v1/download/t/{token}       — token-authenticated download
            //   POST /api/v1/download/{id}/token      — issue a download token
            .service(
                web::scope("/download")
                    .configure(download_token::configure_token)
                    .configure(files::configure_download),
            )
            // Admin operations (Owner / Admin role required per-handler).
            .service(web::scope("/admin").configure(admin::configure))
            // GET /api/v1/events  — SSE real-time push stream
            .route("/events", web::get().to(sse::events))
            // WebSocket JTP/1 fallback transport (for environments where QUIC/UDP is blocked).
            // GET /api/v1/transfer/ws  — HTTP Upgrade → WebSocket
            .route("/transfer/ws", web::get().to(crate::ws::ws_transfer)),
    );
}
