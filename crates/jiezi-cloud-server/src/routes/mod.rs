//! HTTP route configuration for the `/api/v1` namespace.

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
            // GET /api/v1/events  — SSE real-time push stream
            .route("/events", web::get().to(sse::events)),
    );
}
