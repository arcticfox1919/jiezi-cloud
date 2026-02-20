//! Integration tests for user registration (`POST /api/v1/auth/register`).
//!
//! Covers:
//! - Successful registration → 201 with user object (no password hash)
//! - Duplicate username → 409 Conflict

mod common;

use actix_web::{http::StatusCode, test};
use serde_json::{json, Value};

// ─── Successful registration ──────────────────────────────────────────────────

/// `POST /auth/register` with a fresh username → 201 Created.
/// The response must contain the user object but must NOT expose the password hash.
#[actix_web::test]
async fn register_new_user() {
    let app  = common::make_app(true).await;
    let resp = common::test_post_json(
        &app,
        "/api/v1/auth/register",
        json!({
            "username":     "alice",
            "email":        "alice@example.com",
            "password":     "correct-horse-battery",
            "display_name": "Alice"
        }),
    )
    .await;

    assert_eq!(resp.status(), StatusCode::CREATED);

    let body: Value = test::read_body_json(resp).await;
    assert_eq!(body["username"], "alice");
    assert_eq!(body["email"],    "alice@example.com");
    assert!(
        body.get("password_hash").is_none(),
        "password hash must not appear in the API response"
    );
}

// ─── Duplicate username ───────────────────────────────────────────────────────

/// Registering the same username twice → 409 Conflict.
#[actix_web::test]
async fn duplicate_username_is_conflict() {
    let app      = common::make_app(true).await;
    let req_body = json!({
        "username": "bob",
        "email":    "bob@example.com",
        "password": "passw0rd-long-enough"
    });

    let first = common::test_post_json(&app, "/api/v1/auth/register", req_body.clone()).await;
    assert_eq!(first.status(), StatusCode::CREATED);

    let second = common::test_post_json(&app, "/api/v1/auth/register", req_body).await;
    assert_eq!(second.status(), StatusCode::CONFLICT);
}
