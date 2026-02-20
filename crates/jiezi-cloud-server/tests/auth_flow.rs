//! Integration tests for the authentication API (`/api/v1/auth/**`).
//!
//! Every test spins up a real Actix-web application backed by an in-memory
//! SQLite database.  All HTTP requests go through the full middleware stack
//! (setup guard, tracing logger, JSON error handler) and real service objects,
//! so these tests exercise the complete request-response path while remaining
//! fast (no disk I/O, no port binding).
//!
//! # Test coverage
//!
//! | # | Test name                          | What it checks                             |
//! |---|------------------------------------|--------------------------------------------|
//! | 1 | `health_returns_ok`                | GET /health → 200 before and after setup   |
//! | 2 | `setup_guard_blocks_api_routes`    | 503 on auth routes until wizard finishes   |
//! | 3 | `setup_status_before_wizard`       | setup_required = true on a fresh DB        |
//! | 4 | `run_setup_wizard`                 | POST /setup/complete → 200                 |
//! | 5 | `setup_status_after_wizard`        | setup_required = false after completion    |
//! | 6 | `duplicate_setup_is_conflict`      | second POST /setup/complete → 409          |
//! | 7 | `register_new_user`                | POST /auth/register → 201 after setup      |
//! | 8 | `login_with_valid_credentials`     | POST /auth/login → 200 with token pair     |
//! | 9 | `login_with_wrong_password`        | POST /auth/login → 401                     |
//! |10 | `get_own_profile`                  | GET /auth/me with Bearer → 200             |
//! |11 | `refresh_token_rotation`           | POST /auth/refresh → 200, new pair         |
//! |12 | `logout_revokes_refresh_token`     | POST /auth/logout + re-use → 401          |
//! |13 | `change_own_password`              | POST /auth/me/password → 200               |
//! |14 | `update_own_profile`               | PATCH /auth/me → 200                       |
//! |15 | `full_auth_lifecycle`              | setup → register → login → me → refresh → logout |

mod common;

use actix_web::{
    http::StatusCode,
    test::{self, TestRequest},
};
use serde_json::{json, Value};

// ─── 1. Health probe ──────────────────────────────────────────────────────────

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

// ─── 2. Setup guard ───────────────────────────────────────────────────────────

/// Auth routes must be blocked with 503 until the wizard has been completed.
#[actix_web::test]
async fn setup_guard_blocks_api_routes() {
    let app = common::make_app(false).await; // setup NOT done

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

/// The setup and health endpoints must pass through the guard.
#[actix_web::test]
async fn setup_guard_allows_setup_and_health() {
    let app = common::make_app(false).await;

    let setup_status = TestRequest::get()
        .uri("/api/v1/setup/status")
        .to_request();
    let resp = test::call_service(&app, setup_status).await;
    assert_eq!(resp.status(), StatusCode::OK, "setup/status should bypass guard");

    let health = TestRequest::get().uri("/health").to_request();
    let resp   = test::call_service(&app, health).await;
    assert_eq!(resp.status(), StatusCode::OK, "/health should bypass guard");
}

// ─── 3. Setup status (before) ─────────────────────────────────────────────────

/// Fresh database → `setup_required: true`.
#[actix_web::test]
async fn setup_status_before_wizard() {
    let app = common::make_app(false).await;

    let req  = TestRequest::get().uri("/api/v1/setup/status").to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), StatusCode::OK);

    let body: Value = test::read_body_json(resp).await;
    assert_eq!(body["setup_required"], true);
    assert!(body["version"].is_string());
}

// ─── 4. Run the setup wizard ──────────────────────────────────────────────────

/// POST /setup/complete with valid owner details → 200 success.
#[actix_web::test]
async fn run_setup_wizard() {
    let app = common::make_app(false).await;

    let resp = post_setup_complete(&app, "admin").await;
    assert_eq!(resp.status(), StatusCode::OK);

    let body: Value = test::read_body_json(resp).await;
    assert_eq!(body["success"], true);
}

// ─── 5. Setup status (after) ──────────────────────────────────────────────────

/// After completing setup, `setup_required` flips to `false`.
#[actix_web::test]
async fn setup_status_after_wizard() {
    let app = common::make_app(false).await;

    // Complete setup first.
    let _ = post_setup_complete(&app, "admin").await;

    let req  = TestRequest::get().uri("/api/v1/setup/status").to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), StatusCode::OK);

    let body: Value = test::read_body_json(resp).await;
    assert_eq!(body["setup_required"], false,
        "setup_required should be false after completing the wizard");
}

// ─── 6. Duplicate setup ───────────────────────────────────────────────────────

/// Running the wizard a second time on an already-set-up server → 409.
#[actix_web::test]
async fn duplicate_setup_is_conflict() {
    let app = common::make_app(false).await;

    // First call succeeds.
    let first = post_setup_complete(&app, "admin").await;
    assert_eq!(first.status(), StatusCode::OK);

    // Second call with a different username still fails — setup is done.
    let second = post_setup_complete(&app, "admin2").await;
    assert_eq!(second.status(), StatusCode::CONFLICT,
        "second setup attempt must return 409 Conflict");
}

// ─── 7. Register ─────────────────────────────────────────────────────────────

/// POST /auth/register → 201 Created with the new user object.
/// Requires `registration_enabled: true` in the wizard (default here: `true`).
#[actix_web::test]
async fn register_new_user() {
    // setup_completed = true: skip wizard, go straight to auth routes.
    let app = common::make_app(true).await;

    let req = TestRequest::post()
        .uri("/api/v1/auth/register")
        .set_json(json!({
            "username":     "alice",
            "email":        "alice@example.com",
            "password":     "correct-horse-battery",
            "display_name": "Alice"
        }))
        .to_request();

    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), StatusCode::CREATED);

    let body: Value = test::read_body_json(resp).await;
    assert_eq!(body["username"], "alice");
    assert_eq!(body["email"],    "alice@example.com");
    // Password hash must never appear in the API response.
    assert!(body.get("password_hash").is_none(), "password hash must not be returned");
}

/// Registering the same username twice → 409.
#[actix_web::test]
async fn duplicate_username_is_conflict() {
    let app = common::make_app(true).await;

    let req_body = json!({
        "username": "bob",
        "email":    "bob@example.com",
        "password": "passw0rd-long-enough"
    });

    let first  = test_post_json(&app, "/api/v1/auth/register", req_body.clone()).await;
    assert_eq!(first.status(), StatusCode::CREATED);

    let second = test_post_json(&app, "/api/v1/auth/register", req_body).await;
    assert_eq!(second.status(), StatusCode::CONFLICT);
}

// ─── 8. Login (happy path) ────────────────────────────────────────────────────

/// POST /auth/login with valid credentials returns a `TokenPair`.
#[actix_web::test]
async fn login_with_valid_credentials() {
    let app = common::make_app(true).await;
    register_user(&app, "carol", "carol@example.com", "S3cretP@ss").await;

    let resp = login_user(&app, "carol", "S3cretP@ss").await;
    assert_eq!(resp.status(), StatusCode::OK);

    let body: Value = test::read_body_json(resp).await;
    assert!(body["access_token"].is_string(),  "access_token missing");
    assert!(body["refresh_token"].is_string(), "refresh_token missing");
    assert!(body["expires_in"].is_number(),    "expires_in missing");
}

// ─── 9. Login (wrong password) ────────────────────────────────────────────────

/// Wrong password → 401 Unauthorized.
#[actix_web::test]
async fn login_with_wrong_password() {
    let app = common::make_app(true).await;
    register_user(&app, "dave", "dave@example.com", "correct-pass").await;

    let resp = login_user(&app, "dave", "wrong-pass").await;
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

/// Login with a username that does not exist → 401.
#[actix_web::test]
async fn login_with_unknown_user() {
    let app  = common::make_app(true).await;
    let resp = login_user(&app, "nobody", "doesnt-matter").await;
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

// ─── 10. GET /auth/me ────────────────────────────────────────────────────────

/// Authenticated users can retrieve their own profile.
#[actix_web::test]
async fn get_own_profile() {
    let app    = common::make_app(true).await;
    register_user(&app, "eve", "eve@example.com", "eve-password-1234").await;
    let tokens = do_login(&app, "eve", "eve-password-1234").await;

    let req = TestRequest::get()
        .uri("/api/v1/auth/me")
        .insert_header(("Authorization", format!("Bearer {}", tokens["access_token"].as_str().unwrap())))
        .to_request();

    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), StatusCode::OK);

    let body: Value = test::read_body_json(resp).await;
    assert_eq!(body["username"], "eve");
    assert_eq!(body["email"],    "eve@example.com");
}

/// Calling GET /auth/me without a token → 401.
#[actix_web::test]
async fn get_own_profile_without_token() {
    let app  = common::make_app(true).await;
    let req  = TestRequest::get().uri("/api/v1/auth/me").to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

// ─── 11. Refresh → new token pair ────────────────────────────────────────────

/// POST /auth/refresh returns a new token pair (rotation).
/// The new access_token must be different from the original.
#[actix_web::test]
async fn refresh_token_rotation() {
    let app    = common::make_app(true).await;
    register_user(&app, "frank", "frank@example.com", "frank-pass-1234").await;
    let tokens = do_login(&app, "frank", "frank-pass-1234").await;

    let original_refresh = tokens["refresh_token"].as_str().unwrap().to_string();

    let resp = test_post_json(
        &app,
        "/api/v1/auth/refresh",
        json!({ "refresh_token": original_refresh }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);

    let body: Value = test::read_body_json(resp).await;
    assert!(body["access_token"].is_string(),  "new access_token missing");
    assert!(body["refresh_token"].is_string(), "new refresh_token missing");
    // Each refresh token carries a unique jti, so the new one must differ.
    assert_ne!(
        body["refresh_token"].as_str().unwrap(),
        original_refresh,
        "new refresh_token must differ from the consumed one"
    );
}

// ─── 12. Logout ───────────────────────────────────────────────────────────────

/// After logout, attempting to refresh with the revoked token → 401.
#[actix_web::test]
async fn logout_revokes_refresh_token() {
    let app    = common::make_app(true).await;
    register_user(&app, "grace", "grace@example.com", "grace-pass-1234").await;
    let tokens = do_login(&app, "grace", "grace-pass-1234").await;

    let refresh_token = tokens["refresh_token"].as_str().unwrap().to_string();

    // Logout (sends the refresh token so the server can revoke it).
    let logout_resp = test_post_json(
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

    // Trying to use the revoked refresh token must fail.
    let reuse_resp = test_post_json(
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

// ─── 13. Change own password ─────────────────────────────────────────────────

/// POST /auth/me/password changes the password; logging in with the old one fails.
#[actix_web::test]
async fn change_own_password() {
    let app    = common::make_app(true).await;
    register_user(&app, "heidi", "heidi@example.com", "old-password-123").await;
    let tokens = do_login(&app, "heidi", "old-password-123").await;
    let access = tokens["access_token"].as_str().unwrap();

    // Change password.
    let change_resp = TestRequest::post()
        .uri("/api/v1/auth/me/password")
        .insert_header(("Authorization", format!("Bearer {access}")))
        .set_json(json!({
            "old_password": "old-password-123",
            "new_password": "new-better-password-456"
        }))
        .to_request();
    let resp = test::call_service(&app, change_resp).await;
    assert!(resp.status().is_success(), "password change failed: {}", resp.status());

    // Old password no longer works.
    let old_login = login_user(&app, "heidi", "old-password-123").await;
    assert_eq!(old_login.status(), StatusCode::UNAUTHORIZED,
        "old password should no longer be accepted");

    // New password works.
    let new_login = login_user(&app, "heidi", "new-better-password-456").await;
    assert_eq!(new_login.status(), StatusCode::OK,
        "new password should be accepted");
}

// ─── 14. Update own profile ───────────────────────────────────────────────────

/// PATCH /auth/me updates display_name, responds with the updated user.
#[actix_web::test]
async fn update_own_profile() {
    let app    = common::make_app(true).await;
    register_user(&app, "ivan", "ivan@example.com", "ivan-pass-1234").await;
    let tokens = do_login(&app, "ivan", "ivan-pass-1234").await;
    let access = tokens["access_token"].as_str().unwrap();

    // UpdateProfileRequest.display_name is Option<Option<String>>.
    // In JSON: a plain string value means Some(Some("...")) — set the field.
    //           null means Some(None) — clear the field.
    //           absent means None — leave unchanged.
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

// ─── 15. Full auth lifecycle ─────────────────────────────────────────────────

/// End-to-end: setup wizard → register → login → GET me → refresh → logout.
///
/// This test mirrors the exact sequence a new user performs on first use.
#[actix_web::test]
async fn full_auth_lifecycle() {
    // ── A: fresh server ───────────────────────────────────────────────────────
    let app = common::make_app(false).await;

    // Health is reachable before anything.
    let health  = TestRequest::get().uri("/health").to_request();
    let h_resp  = test::call_service(&app, health).await;
    assert_eq!(h_resp.status(), StatusCode::OK, "health before setup");

    // ── B: setup wizard ───────────────────────────────────────────────────────
    let setup_resp = post_setup_complete(&app, "superadmin").await;
    assert_eq!(setup_resp.status(), StatusCode::OK, "setup/complete");

    // ── C: register a normal user ─────────────────────────────────────────────
    let reg = TestRequest::post()
        .uri("/api/v1/auth/register")
        .set_json(json!({
            "username": "judy",
            "email":    "judy@test.local",
            "password": "judy-secret-pass-1"
        }))
        .to_request();
    let reg_resp = test::call_service(&app, reg).await;
    assert_eq!(reg_resp.status(), StatusCode::CREATED, "register");
    let user: Value = test::read_body_json(reg_resp).await;
    assert_eq!(user["username"], "judy");

    // ── D: login ──────────────────────────────────────────────────────────────
    let tokens = do_login(&app, "judy", "judy-secret-pass-1").await;
    let access  = tokens["access_token"].as_str().expect("no access_token");
    let refresh = tokens["refresh_token"].as_str().expect("no refresh_token");

    // ── E: GET /me ────────────────────────────────────────────────────────────
    let me = TestRequest::get()
        .uri("/api/v1/auth/me")
        .insert_header(("Authorization", format!("Bearer {access}")))
        .to_request();
    let me_resp = test::call_service(&app, me).await;
    assert_eq!(me_resp.status(), StatusCode::OK, "GET /me");
    let me_body: Value = test::read_body_json(me_resp).await;
    assert_eq!(me_body["username"], "judy");

    // ── F: refresh ────────────────────────────────────────────────────────────
    let ref_resp = test_post_json(
        &app,
        "/api/v1/auth/refresh",
        json!({ "refresh_token": refresh }),
    )
    .await;
    assert_eq!(ref_resp.status(), StatusCode::OK, "refresh");
    let new_tokens: Value = test::read_body_json(ref_resp).await;
    let new_refresh = new_tokens["refresh_token"].as_str().expect("no new refresh_token");

    // ── G: logout ─────────────────────────────────────────────────────────────
    let logout_resp = test_post_json(
        &app,
        "/api/v1/auth/logout",
        json!({ "refresh_token": new_refresh }),
    )
    .await;
    assert!(logout_resp.status().is_success(), "logout");
}

// ─── Private helpers ──────────────────────────────────────────────────────────

/// POST JSON to `uri`, return the raw `ServiceResponse`.
async fn test_post_json<S, B>(app: &S, uri: &str, body: Value) -> actix_web::dev::ServiceResponse<B>
where
    S: actix_web::dev::Service<
        actix_http::Request,
        Response = actix_web::dev::ServiceResponse<B>,
        Error = actix_web::Error,
    >,
{
    let req = TestRequest::post()
        .uri(uri)
        .set_json(body)
        .to_request();
    test::call_service(app, req).await
}

/// POST /api/v1/setup/complete for `username`, returns the `ServiceResponse`.
async fn post_setup_complete<S, B>(app: &S, username: &str) -> actix_web::dev::ServiceResponse<B>
where
    S: actix_web::dev::Service<
        actix_http::Request,
        Response = actix_web::dev::ServiceResponse<B>,
        Error = actix_web::Error,
    >,
{
    test_post_json(
        app,
        "/api/v1/setup/complete",
        json!({
            "admin_username":    username,
            "admin_email":       format!("{username}@example.com"),
            "admin_password":    "Admin-Pass-1234!",
            "site_name":         "Test Jiezi Cloud",
            "registration_enabled": true
        }),
    )
    .await
}

/// POST /api/v1/auth/register and assert 201.
async fn register_user<S, B>(app: &S, username: &str, email: &str, password: &str)
where
    S: actix_web::dev::Service<
        actix_http::Request,
        Response = actix_web::dev::ServiceResponse<B>,
        Error = actix_web::Error,
    >,
{
    let resp = test_post_json(
        app,
        "/api/v1/auth/register",
        json!({ "username": username, "email": email, "password": password }),
    )
    .await;
    assert_eq!(
        resp.status(),
        StatusCode::CREATED,
        "register_user({username}) failed with {}",
        resp.status()
    );
}

/// POST /api/v1/auth/login without asserting; returns the `ServiceResponse`.
async fn login_user<S, B>(app: &S, username: &str, password: &str) -> actix_web::dev::ServiceResponse<B>
where
    S: actix_web::dev::Service<
        actix_http::Request,
        Response = actix_web::dev::ServiceResponse<B>,
        Error = actix_web::Error,
    >,
{
    test_post_json(
        app,
        "/api/v1/auth/login",
        json!({ "credential": username, "password": password }),
    )
    .await
}

/// POST /api/v1/auth/login, assert 200, and return the parsed token pair JSON.
async fn do_login<S, B>(app: &S, username: &str, password: &str) -> Value
where
    S: actix_web::dev::Service<
        actix_http::Request,
        Response = actix_web::dev::ServiceResponse<B>,
        Error = actix_web::Error,
    >,
    B: actix_web::body::MessageBody + Unpin,
{
    let resp = login_user(app, username, password).await;
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "do_login({username}) failed with {}",
        resp.status()
    );
    test::read_body_json(resp).await
}
