//! Integration tests for token session management.
//!
//! Covers:
//! - `POST /auth/refresh` — token rotation: new pair returned, old refresh invalidated
//! - `POST /auth/logout` — revokes refresh token; re-use must fail

mod common;

use actix_web::{http::StatusCode, test};
use serde_json::{json, Value};

// ─── Token rotation ───────────────────────────────────────────────────────────

/// `POST /auth/refresh` returns a new token pair.
/// The new `refresh_token` must be different from the consumed one (rotation).
#[actix_web::test]
async fn refresh_token_rotation() {
    let app    = common::make_app(true).await;
    common::register_user(&app, "frank", "frank@example.com", "frank-pass-1234").await;
    let tokens = common::do_login(&app, "frank", "frank-pass-1234").await;

    let original_refresh = tokens["refresh_token"].as_str().unwrap().to_string();

    let resp = common::test_post_json(
        &app,
        "/api/v1/auth/refresh",
        json!({ "refresh_token": original_refresh }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);

    let body: Value = test::read_body_json(resp).await;
    assert!(body["access_token"].is_string(),  "new access_token missing");
    assert!(body["refresh_token"].is_string(), "new refresh_token missing");
    assert_ne!(
        body["refresh_token"].as_str().unwrap(),
        original_refresh,
        "new refresh_token must differ from the consumed one"
    );
}

// ─── Logout ───────────────────────────────────────────────────────────────────

/// After logout the revoked refresh token must no longer be usable.
#[actix_web::test]
async fn logout_revokes_refresh_token() {
    let app    = common::make_app(true).await;
    common::register_user(&app, "grace", "grace@example.com", "grace-pass-1234").await;
    let tokens = common::do_login(&app, "grace", "grace-pass-1234").await;

    let refresh_token = tokens["refresh_token"].as_str().unwrap().to_string();

    // Logout — sends the refresh token so the server can revoke it.
    let logout_resp = common::test_post_json(
        &app,
        "/api/v1/auth/logout",
        json!({ "refresh_token": refresh_token }),
    )
    .await;
    assert!(
        logout_resp.status().is_success(),
        "logout should succeed, got {}",
        logout_resp.status()
    );

    // Reusing the revoked token must fail.
    let reuse_resp = common::test_post_json(
        &app,
        "/api/v1/auth/refresh",
        json!({ "refresh_token": refresh_token }),
    )
    .await;
    assert_eq!(
        reuse_resp.status(),
        StatusCode::UNAUTHORIZED,
        "reusing a revoked refresh token must return 401"
    );
}
