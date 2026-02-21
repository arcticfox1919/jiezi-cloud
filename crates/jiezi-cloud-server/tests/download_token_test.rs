//! Integration tests for Stage 8 — Download Tokens & Streaming.
//!
//! # Coverage
//!
//! | # | Test                                 | What it checks                               |
//! |---|--------------------------------------|----------------------------------------------|
//! | 1 | `issue_token_returns_url`            | POST /download/{id}/token → 200 with token   |
//! | 2 | `download_by_token_returns_file`     | GET /download/t/{token} → 200 with bytes     |
//! | 3 | `one_time_token_invalidated`         | Second use of one-time token → 410           |
//! | 4 | `invalid_token_returns_not_found`    | Unknown token → 404                          |
//! | 5 | `issue_token_for_nonexistent_file`   | Token issue for bad file ID → 404            |

mod common;

use actix_web::{
    http::StatusCode,
    test::{self, TestRequest},
};
use bytes::Bytes;
use sha2::{Digest, Sha256};

// ─── Setup helpers ─────────────────────────────────────────────────────────

/// Register, log in, create root, upload a tiny file.
/// Returns `(bearer, file_node_id)`.
async fn upload_test_file<S, B>(
    app: &S,
    username: &str,
    content: &'static [u8],
) -> (String, String)
where
    S: actix_web::dev::Service<
        actix_http::Request,
        Response = actix_web::dev::ServiceResponse<B>,
        Error = actix_web::Error,
    >,
    B: actix_web::body::MessageBody + Unpin,
{
    common::register_user(app, username, &format!("{username}@test.local"), "Test-Pass-1234!").await;
    let tokens = common::do_login(app, username, "Test-Pass-1234!").await;
    let bearer = tokens["access_token"].as_str().unwrap().to_owned();

    // Create root.
    let req = TestRequest::post()
        .uri("/api/v1/files/root")
        .insert_header(("Authorization", format!("Bearer {bearer}")))
        .to_request();
    let resp = test::call_service(app, req).await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    let root: serde_json::Value = test::read_body_json(resp).await;
    let root_id = root["id"].as_str().unwrap().to_owned();

    // PREPARE → CHUNK → COMPLETE.
    let hash = sha256_hex(content);
    let total_size = content.len() as u64;

    let prepared: serde_json::Value = {
        let req = TestRequest::post()
            .uri("/api/v1/upload/prepare")
            .insert_header(("Authorization", format!("Bearer {bearer}")))
            .set_json(serde_json::json!({
                "parent_id":    root_id,
                "file_name":    "test.bin",
                "total_size":   total_size,
                "content_hash": hash,
            }))
            .to_request();
        let resp = test::call_service(app, req).await;
        assert_eq!(resp.status(), StatusCode::OK);
        test::read_body_json(resp).await
    };
    let session_id = prepared["session_id"].as_str().unwrap();

    // Chunk 0.
    let req = TestRequest::post()
        .uri(&format!("/api/v1/upload/{session_id}/chunk/0"))
        .insert_header(("Authorization", format!("Bearer {bearer}")))
        .insert_header(("Content-Type", "application/octet-stream"))
        .set_payload(Bytes::copy_from_slice(content))
        .to_request();
    let resp = test::call_service(app, req).await;
    assert_eq!(resp.status(), StatusCode::OK);

    // Complete.
    let req = TestRequest::post()
        .uri(&format!("/api/v1/upload/{session_id}/complete"))
        .insert_header(("Authorization", format!("Bearer {bearer}")))
        .to_request();
    let resp = test::call_service(app, req).await;
    assert_eq!(resp.status(), StatusCode::CREATED, "upload complete failed");
    let node: serde_json::Value = test::read_body_json(resp).await;
    let file_id = node["id"].as_str().unwrap().to_owned();

    (bearer, file_id)
}

fn sha256_hex(data: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(data);
    hex::encode(h.finalize())
}

// ─── Test 1: Issue token returns token + URL ───────────────────────────────

#[actix_web::test]
async fn issue_token_returns_url() {
    let app = common::make_app(true).await;
    common::post_setup_complete(&app, "admin_dt1").await;

    let (bearer, file_id) = upload_test_file(&app, "dt_user1", b"token test file").await;

    let req = TestRequest::post()
        .uri(&format!("/api/v1/download/{file_id}/token"))
        .insert_header(("Authorization", format!("Bearer {bearer}")))
        .set_json(serde_json::json!({ "ttl_secs": 3600, "one_time": false }))
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), StatusCode::OK);

    let body: serde_json::Value = test::read_body_json(resp).await;
    assert!(body["token"].is_string(), "token field missing");
    assert!(body["url"].is_string(), "url field missing");
    assert!(body["expires_at"].is_number(), "expires_at field missing");

    let token = body["token"].as_str().unwrap();
    assert_eq!(token.len(), 32, "token should be 32 hex chars, got {}", token.len());
}

// ─── Test 2: Download by token returns file bytes ─────────────────────────

#[actix_web::test]
async fn download_by_token_returns_file() {
    let app = common::make_app(true).await;
    common::post_setup_complete(&app, "admin_dt2").await;

    const CONTENT: &[u8] = b"file content for token download";
    let (bearer, file_id) = upload_test_file(&app, "dt_user2", CONTENT).await;

    // Issue a regular (multi-use) token.
    let issued: serde_json::Value = {
        let req = TestRequest::post()
            .uri(&format!("/api/v1/download/{file_id}/token"))
            .insert_header(("Authorization", format!("Bearer {bearer}")))
            .set_json(serde_json::json!({ "ttl_secs": 3600, "one_time": false }))
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::OK);
        test::read_body_json(resp).await
    };
    let token = issued["token"].as_str().unwrap();

    // Download without authentication.
    let req = TestRequest::get()
        .uri(&format!("/api/v1/download/t/{token}"))
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), StatusCode::OK, "token download failed: {}", resp.status());

    let body = test::read_body(resp).await;
    assert_eq!(body.as_ref(), CONTENT, "downloaded content mismatch");
}

// ─── Test 3: One-time token is invalidated after first use ────────────────

#[actix_web::test]
async fn one_time_token_invalidated() {
    let app = common::make_app(true).await;
    common::post_setup_complete(&app, "admin_dt3").await;

    let (bearer, file_id) = upload_test_file(&app, "dt_user3", b"one time file").await;

    // Issue a one-time token.
    let issued: serde_json::Value = {
        let req = TestRequest::post()
            .uri(&format!("/api/v1/download/{file_id}/token"))
            .insert_header(("Authorization", format!("Bearer {bearer}")))
            .set_json(serde_json::json!({ "ttl_secs": 3600, "one_time": true }))
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::OK);
        test::read_body_json(resp).await
    };
    let token = issued["token"].as_str().unwrap();

    // First use: should succeed.
    {
        let req = TestRequest::get()
            .uri(&format!("/api/v1/download/t/{token}"))
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::OK, "first use of one-time token failed");
    }

    // Second use: token already consumed → 410 Gone.
    {
        let req = TestRequest::get()
            .uri(&format!("/api/v1/download/t/{token}"))
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(
            resp.status(),
            StatusCode::GONE,
            "expected 410 on second one-time token use, got {}",
            resp.status()
        );
    }
}

// ─── Test 4: Unknown token → 404 ─────────────────────────────────────────

#[actix_web::test]
async fn invalid_token_returns_not_found() {
    let app = common::make_app(true).await;
    common::post_setup_complete(&app, "admin_dt4").await;

    let fake_token = "00000000000000000000000000000000"; // 32 hex zeros

    let req = TestRequest::get()
        .uri(&format!("/api/v1/download/t/{fake_token}"))
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

// ─── Test 5: Token for non-existent file → 404 ────────────────────────────

#[actix_web::test]
async fn issue_token_for_nonexistent_file() {
    let app = common::make_app(true).await;
    common::post_setup_complete(&app, "admin_dt5").await;

    common::register_user(&app, "dt_user5", "dt5@test.local", "Test-Pass-1234!").await;
    let tokens = common::do_login(&app, "dt_user5", "Test-Pass-1234!").await;
    let bearer = tokens["access_token"].as_str().unwrap();

    // Use a valid-looking but non-existent UUID.
    let fake_id = "01960000-0000-7000-8000-000000000000";

    let req = TestRequest::post()
        .uri(&format!("/api/v1/download/{fake_id}/token"))
        .insert_header(("Authorization", format!("Bearer {bearer}")))
        .set_json(serde_json::json!({ "ttl_secs": 60, "one_time": false }))
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}
