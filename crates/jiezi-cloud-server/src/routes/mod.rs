//! HTTP route configuration for the `/api/v1` namespace.

pub mod admin;
pub mod auth;
pub mod files;
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
            // Upload pipeline: POST /api/v1/upload/?parent_id=…&name=…
            .service(web::scope("/upload").configure(files::configure_upload))
            // Download pipeline: GET /api/v1/download/{id}
            .service(web::scope("/download").configure(files::configure_download))
            // Admin operations (Owner / Admin role required per-handler).
            .service(web::scope("/admin").configure(admin::configure))
            // GET /api/v1/events  — SSE real-time push stream
            .route("/events", web::get().to(sse::events))
            // WebSocket JTP/1 fallback transport (for environments where QUIC/UDP is blocked).
            // GET /api/v1/transfer/ws  — HTTP Upgrade → WebSocket
            .route("/transfer/ws", web::get().to(crate::ws::ws_transfer)),
    );
}
