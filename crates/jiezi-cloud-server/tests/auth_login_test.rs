//! Integration tests for user login (`POST /api/v1/auth/login`).
//!
//! Covers:
//! - Happy path: valid credentials → 200 with access + refresh token
//! - Wrong password → 401
//! - Non-existent username → 401

mod common;

use actix_web::{http::StatusCode, test};
use serde_json::Value;

// ─── Happy path ───────────────────────────────────────────────────────────────

/// Valid credentials → 200 OK with a `TokenPair` body.
#[actix_web::test]
async fn login_with_valid_credentials() {
    let app = common::make_app(true).await;
    common::register_user(&app, "carol", "carol@example.com", "S3cretP@ss").await;

    let resp = common::login_user(&app, "carol", "S3cretP@ss").await;
    assert_eq!(resp.status(), StatusCode::OK);

    let body: Value = test::read_body_json(resp).await;
    assert!(body["access_token"].is_string(),  "access_token missing");
    assert!(body["refresh_token"].is_string(), "refresh_token missing");
    assert!(body["expires_in"].is_number(),    "expires_in missing");
}

// ─── Wrong password ───────────────────────────────────────────────────────────

/// Incorrect password for an existing user → 401 Unauthorized.
#[actix_web::test]
async fn login_with_wrong_password() {
    let app = common::make_app(true).await;
    common::register_user(&app, "dave", "dave@example.com", "correct-pass").await;

    let resp = common::login_user(&app, "dave", "wrong-pass").await;
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

// ─── Unknown user ─────────────────────────────────────────────────────────────

/// Login attempt for a username that does not exist → 401.
/// Must not reveal whether the account exists (same status as wrong password).
#[actix_web::test]
async fn login_with_unknown_user() {
    let app  = common::make_app(true).await;
    let resp = common::login_user(&app, "nobody", "doesnt-matter").await;
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}
