//! End-to-end authentication lifecycle test.
//!
//! Exercises the complete user journey on a fresh server:
//! setup wizard → register → login → GET /me → refresh → logout

mod common;

use actix_web::{http::StatusCode, test, test::TestRequest};
use serde_json::{json, Value};

/// Full lifecycle: setup → register → login → GET /me → refresh → logout.
///
/// This mirrors the exact sequence a new user performs on first use of the
/// system.  It is deliberately kept as a single test to ensure the happy-path
/// works end-to-end with a fresh database.
#[actix_web::test]
async fn full_auth_lifecycle() {
    // ── A: fresh server ───────────────────────────────────────────────────────
    let app = common::make_app(false).await;

    // Health is reachable before anything is configured.
    let health = TestRequest::get().uri("/health").to_request();
    let h_resp = test::call_service(&app, health).await;
    assert_eq!(h_resp.status(), StatusCode::OK, "health before setup");

    // ── B: run the setup wizard ───────────────────────────────────────────────
    let setup_resp = common::post_setup_complete(&app, "superadmin").await;
    assert_eq!(setup_resp.status(), StatusCode::OK, "POST /setup/complete");

    // ── C: register a normal user ─────────────────────────────────────────────
    let reg_resp = common::test_post_json(
        &app,
        "/api/v1/auth/register",
        json!({
            "username": "judy",
            "email":    "judy@test.local",
            "password": "judy-secret-pass-1"
        }),
    )
    .await;
    assert_eq!(reg_resp.status(), StatusCode::CREATED, "POST /auth/register");
    let user: Value = test::read_body_json(reg_resp).await;
    assert_eq!(user["username"], "judy");

    // ── D: login ──────────────────────────────────────────────────────────────
    let tokens  = common::do_login(&app, "judy", "judy-secret-pass-1").await;
    let access  = tokens["access_token"].as_str().expect("no access_token");
    let refresh = tokens["refresh_token"].as_str().expect("no refresh_token");

    // ── E: GET /me ────────────────────────────────────────────────────────────
    let me = TestRequest::get()
        .uri("/api/v1/auth/me")
        .insert_header(("Authorization", format!("Bearer {access}")))
        .to_request();
    let me_resp = test::call_service(&app, me).await;
    assert_eq!(me_resp.status(), StatusCode::OK, "GET /auth/me");
    let me_body: Value = test::read_body_json(me_resp).await;
    assert_eq!(me_body["username"], "judy");

    // ── F: refresh ────────────────────────────────────────────────────────────
    let ref_resp = common::test_post_json(
        &app,
        "/api/v1/auth/refresh",
        json!({ "refresh_token": refresh }),
    )
    .await;
    assert_eq!(ref_resp.status(), StatusCode::OK, "POST /auth/refresh");
    let new_tokens: Value = test::read_body_json(ref_resp).await;
    let new_refresh = new_tokens["refresh_token"].as_str().expect("no new refresh_token");

    // ── G: logout ─────────────────────────────────────────────────────────────
    let logout_resp = common::test_post_json(
        &app,
        "/api/v1/auth/logout",
        json!({ "refresh_token": new_refresh }),
    )
    .await;
    assert!(logout_resp.status().is_success(), "POST /auth/logout");
}
