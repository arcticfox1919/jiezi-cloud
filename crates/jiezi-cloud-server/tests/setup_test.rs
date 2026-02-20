//! Integration tests for the first-run setup wizard (`/api/v1/setup/**`).
//!
//! Covers:
//! - `GET /setup/status` before and after the wizard
//! - `POST /setup/complete` happy path and duplicate rejection

mod common;

use actix_web::{http::StatusCode, test};
use serde_json::Value;

// ─── Setup status (before wizard) ────────────────────────────────────────────

/// Fresh database → `setup_required: true`, version string present.
#[actix_web::test]
async fn setup_status_before_wizard() {
    let app = common::make_app(false).await;

    let req  = actix_web::test::TestRequest::get()
        .uri("/api/v1/setup/status")
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), StatusCode::OK);

    let body: Value = test::read_body_json(resp).await;
    assert_eq!(body["setup_required"], true);
    assert!(body["version"].is_string());
}

// ─── Run the setup wizard ────────────────────────────────────────────────────

/// `POST /setup/complete` with valid owner credentials → 200 success.
#[actix_web::test]
async fn run_setup_wizard() {
    let app  = common::make_app(false).await;
    let resp = common::post_setup_complete(&app, "admin").await;
    assert_eq!(resp.status(), StatusCode::OK);

    let body: Value = test::read_body_json(resp).await;
    assert_eq!(body["success"], true);
}

// ─── Setup status (after wizard) ─────────────────────────────────────────────

/// After completing setup, `setup_required` flips to `false`.
#[actix_web::test]
async fn setup_status_after_wizard() {
    let app = common::make_app(false).await;
    let _   = common::post_setup_complete(&app, "admin").await;

    let req  = actix_web::test::TestRequest::get()
        .uri("/api/v1/setup/status")
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), StatusCode::OK);

    let body: Value = test::read_body_json(resp).await;
    assert_eq!(body["setup_required"], false,
        "setup_required should be false after completing the wizard");
}

// ─── Duplicate setup rejection ──────────────────────────────────────────────

/// Running the wizard a second time on an already-configured server → 409.
#[actix_web::test]
async fn duplicate_setup_is_conflict() {
    let app = common::make_app(false).await;

    let first = common::post_setup_complete(&app, "admin").await;
    assert_eq!(first.status(), StatusCode::OK);

    let second = common::post_setup_complete(&app, "admin2").await;
    assert_eq!(second.status(), StatusCode::CONFLICT,
        "second setup attempt must return 409 Conflict");
}
