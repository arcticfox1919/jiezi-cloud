//! Integration tests for the health probe and setup guard middleware.
//!
//! Covers:
//! - `GET /health` — liveness probe before and after setup
//! - `setup_guard` — 503 routing before wizard, pass-through for setup/health

mod common;

use actix_web::{
    http::StatusCode,
    test::{self, TestRequest},
};
use serde_json::Value;

// ─── Health probe ─────────────────────────────────────────────────────────────

/// `GET /health` must return 200 even before the setup wizard is run.
/// Container orchestrators rely on this as a liveness probe.
#[actix_web::test]
async fn health_returns_ok() {
    let app = common::make_app(false).await; // setup NOT done

    let req  = TestRequest::get().uri("/health").to_request();
    let resp = test::call_service(&app, req).await;

    assert_eq!(resp.status(), StatusCode::OK);

    let body: Value = test::read_body_json(resp).await;
    assert_eq!(body["status"], "ok");
}

// ─── Setup guard ─────────────────────────────────────────────────────────────

/// Auth and file routes must return 503 until the setup wizard is completed.
#[actix_web::test]
async fn setup_guard_blocks_api_routes() {
    let app = common::make_app(false).await;

    for uri in &[
        "/api/v1/auth/login",
        "/api/v1/auth/me",
        "/api/v1/files/trash",
    ] {
        let req  = TestRequest::get().uri(uri).to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(
            resp.status(),
            StatusCode::SERVICE_UNAVAILABLE,
            "expected 503 for {uri} before setup"
        );
    }
}

/// The `/setup/**` and `/health` endpoints must bypass the guard entirely.
#[actix_web::test]
async fn setup_guard_allows_setup_and_health() {
    let app = common::make_app(false).await;

    let req  = TestRequest::get().uri("/api/v1/setup/status").to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), StatusCode::OK, "setup/status should bypass guard");

    let req  = TestRequest::get().uri("/health").to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), StatusCode::OK, "/health should bypass guard");
}
