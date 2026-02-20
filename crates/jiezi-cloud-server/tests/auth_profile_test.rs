//! Integration tests for user profile endpoints.
//!
//! Covers:
//! - `GET /auth/me` — authenticated and unauthenticated
//! - `PATCH /auth/me` — update display_name
//! - `POST /auth/me/password` — change password

mod common;

use actix_web::{http::StatusCode, test, test::TestRequest};
use serde_json::{json, Value};

// ─── GET /auth/me ─────────────────────────────────────────────────────────────

/// Authenticated `GET /auth/me` → 200 with the user's own profile.
/// Sensitive fields (password hash) must not appear.
#[actix_web::test]
async fn get_own_profile() {
    let app    = common::make_app(true).await;
    common::register_user(&app, "eve", "eve@example.com", "eve-password-1234").await;
    let tokens = common::do_login(&app, "eve", "eve-password-1234").await;

    let req = TestRequest::get()
        .uri("/api/v1/auth/me")
        .insert_header((
            "Authorization",
            format!("Bearer {}", tokens["access_token"].as_str().unwrap()),
        ))
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), StatusCode::OK);

    let body: Value = test::read_body_json(resp).await;
    assert_eq!(body["username"], "eve");
    assert_eq!(body["email"],    "eve@example.com");
}

/// `GET /auth/me` without a Bearer token → 401 Unauthorized.
#[actix_web::test]
async fn get_own_profile_without_token() {
    let app  = common::make_app(true).await;
    let req  = TestRequest::get().uri("/api/v1/auth/me").to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

// ─── PATCH /auth/me ──────────────────────────────────────────────────────────

/// `PATCH /auth/me` updates `display_name` and returns the updated user.
#[actix_web::test]
async fn update_own_profile() {
    let app    = common::make_app(true).await;
    common::register_user(&app, "ivan", "ivan@example.com", "ivan-pass-1234").await;
    let tokens = common::do_login(&app, "ivan", "ivan-pass-1234").await;
    let access = tokens["access_token"].as_str().unwrap();

    // `display_name` as a plain string = Some(Some("...")) — set the field.
    // `null` = Some(None) — clear the field.
    // absent = None — leave unchanged.
    let req = TestRequest::patch()
        .uri("/api/v1/auth/me")
        .insert_header(("Authorization", format!("Bearer {access}")))
        .set_json(json!({ "display_name": "Ivan the Tester" }))
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), StatusCode::OK);

    let body: Value = test::read_body_json(resp).await;
    assert_eq!(body["display_name"], "Ivan the Tester");
}

// ─── POST /auth/me/password ──────────────────────────────────────────────────

/// Changing password invalidates the old one and accepts the new one.
#[actix_web::test]
async fn change_own_password() {
    let app    = common::make_app(true).await;
    common::register_user(&app, "heidi", "heidi@example.com", "old-password-123").await;
    let tokens = common::do_login(&app, "heidi", "old-password-123").await;
    let access = tokens["access_token"].as_str().unwrap();

    // Change the password.
    let req = TestRequest::post()
        .uri("/api/v1/auth/me/password")
        .insert_header(("Authorization", format!("Bearer {access}")))
        .set_json(json!({
            "old_password": "old-password-123",
            "new_password": "new-better-password-456"
        }))
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert!(resp.status().is_success(), "password change failed: {}", resp.status());

    // Old password must be rejected.
    let old_login = common::login_user(&app, "heidi", "old-password-123").await;
    assert_eq!(old_login.status(), StatusCode::UNAUTHORIZED,
        "old password should no longer be accepted");

    // New password must work.
    let new_login = common::login_user(&app, "heidi", "new-better-password-456").await;
    assert_eq!(new_login.status(), StatusCode::OK,
        "new password should be accepted");
}
