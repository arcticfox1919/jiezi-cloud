//! Integration tests for Stage 7.5 — Resumable Upload Sessions.
//!
//! # Coverage
//!
//! | # | Test                              | What it checks                                   |
//! |---|-----------------------------------|--------------------------------------------------|
//! | 1 | `prepare_returns_session_id`      | PREPARE → 200 with session_id                    |
//! | 2 | `upload_single_chunk_complete`    | Single-chunk file: PREPARE → CHUNK → COMPLETE    |
//! | 3 | `status_shows_missing_chunks`     | STATUS returns correct missing-chunk list        |
//! | 4 | `upload_resume_after_partial`     | Partial upload → query STATUS → upload rest      |
//! | 5 | `cancel_removes_session`          | CANCEL → DELETE session; subsequent GET → 404    |
//! | 6 | `chunk_size_clamp`                | Requested chunk_size > 64 MiB is clamped         |
//! | 7 | `dedup_instant_complete`          | Uploading same content twice → instant_complete  |

mod common;

use actix_web::{
    http::StatusCode,
    test::{self, TestRequest},
};
use bytes::Bytes;
use sha2::{Digest, Sha256};

// ─── Setup helpers ─────────────────────────────────────────────────────────

/// Register a user, log in, create a VFS root, and return `(bearer, root_id)`.
async fn setup_user_with_root<S, B>(
    app: &S,
    username: &str,
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

    // Create a personal root directory.
    let req = TestRequest::post()
        .uri("/api/v1/files/root")
        .insert_header(("Authorization", format!("Bearer {bearer}")))
        .to_request();
    let resp = test::call_service(app, req).await;
    assert_eq!(resp.status(), StatusCode::CREATED, "create_root failed: {}", resp.status());
    let body: serde_json::Value = test::read_body_json(resp).await;
    let root_id = body["id"].as_str().unwrap().to_owned();

    (bearer, root_id)
}

/// Content-hash a byte slice using SHA-256, returning lowercase hex.
fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}

// ─── Test 1: PREPARE returns a session ────────────────────────────────────

#[actix_web::test]
async fn prepare_returns_session_id() {
    let app = common::make_app(true).await;
    common::post_setup_complete(&app, "admin1").await;
    let (bearer, root_id) = setup_user_with_root(&app, "user1").await;

    let data = b"hello world";
    let hash = sha256_hex(data);

    let body = serde_json::json!({
        "parent_id":   root_id,
        "file_name":   "hello.txt",
        "total_size":  data.len(),
        "content_hash": hash,
    });

    let req = TestRequest::post()
        .uri("/api/v1/upload/prepare")
        .insert_header(("Authorization", format!("Bearer {bearer}")))
        .set_json(body)
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), StatusCode::OK);

    let prepared: serde_json::Value = test::read_body_json(resp).await;
    assert!(prepared["session_id"].is_string(), "session_id missing");
    assert!(prepared["chunk_count"].as_u64().unwrap() >= 1);
    assert_eq!(prepared["instant_complete"], false);
}

// ─── Test 2: Single-chunk upload → COMPLETE ────────────────────────────────

#[actix_web::test]
async fn upload_single_chunk_complete() {
    let app = common::make_app(true).await;
    common::post_setup_complete(&app, "admin2").await;
    let (bearer, root_id) = setup_user_with_root(&app, "user2").await;

    let data = b"single chunk content";
    let hash = sha256_hex(data);
    let total_size = data.len() as u64;

    // PREPARE
    let prepared: serde_json::Value = {
        let req = TestRequest::post()
            .uri("/api/v1/upload/prepare")
            .insert_header(("Authorization", format!("Bearer {bearer}")))
            .set_json(serde_json::json!({
                "parent_id":   root_id,
                "file_name":   "single.bin",
                "total_size":  total_size,
                "content_hash": hash,
            }))
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::OK);
        test::read_body_json(resp).await
    };
    let session_id = prepared["session_id"].as_str().unwrap();
    assert_eq!(prepared["chunk_count"], 1);

    // CHUNK 0
    {
        let req = TestRequest::post()
            .uri(&format!("/api/v1/upload/{session_id}/chunk/0"))
            .insert_header(("Authorization", format!("Bearer {bearer}")))
            .insert_header(("Content-Type", "application/octet-stream"))
            .set_payload(Bytes::from_static(data))
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::OK, "chunk upload failed");
        let status: serde_json::Value = test::read_body_json(resp).await;
        assert!(status["missing_chunks"].as_array().unwrap().is_empty(), "still missing chunks after upload");
    }

    // COMPLETE
    {
        let req = TestRequest::post()
            .uri(&format!("/api/v1/upload/{session_id}/complete"))
            .insert_header(("Authorization", format!("Bearer {bearer}")))
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::CREATED, "complete failed: {}", resp.status());
        let node: serde_json::Value = test::read_body_json(resp).await;
        assert_eq!(node["name"], "single.bin");
        assert_eq!(node["size"], total_size);
    }
}

// ─── Test 3: STATUS shows missing chunks ──────────────────────────────────

#[actix_web::test]
async fn status_shows_missing_chunks() {
    let app = common::make_app(true).await;
    common::post_setup_complete(&app, "admin3").await;
    let (bearer, root_id) = setup_user_with_root(&app, "user3").await;

    // Build a 3-chunk file: 3 × 4 bytes with a 4-byte chunk_size to force 3 chunks.
    let chunk_size: u64 = 4;
    let data = b"AAAABBBBCCCC"; // 12 bytes → 3 chunks of 4
    let hash = sha256_hex(data);

    let prepared: serde_json::Value = {
        let req = TestRequest::post()
            .uri("/api/v1/upload/prepare")
            .insert_header(("Authorization", format!("Bearer {bearer}")))
            .set_json(serde_json::json!({
                "parent_id":   root_id,
                "file_name":   "three.bin",
                "total_size":  data.len() as u64,
                "content_hash": hash,
                "chunk_size":   chunk_size,
            }))
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::OK);
        test::read_body_json(resp).await
    };
    let session_id = prepared["session_id"].as_str().unwrap();
    assert_eq!(prepared["chunk_count"], 3);

    // STATUS before any chunk: all 3 missing.
    {
        let req = TestRequest::get()
            .uri(&format!("/api/v1/upload/{session_id}/status"))
            .insert_header(("Authorization", format!("Bearer {bearer}")))
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let status: serde_json::Value = test::read_body_json(resp).await;
        let missing = status["missing_chunks"].as_array().unwrap();
        assert_eq!(missing.len(), 3, "expected 3 missing, got {missing:?}");
    }

    // Upload chunk 1 only.
    {
        let req = TestRequest::post()
            .uri(&format!("/api/v1/upload/{session_id}/chunk/1"))
            .insert_header(("Authorization", format!("Bearer {bearer}")))
            .insert_header(("Content-Type", "application/octet-stream"))
            .set_payload(Bytes::from_static(b"BBBB"))
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::OK);
    }

    // STATUS after chunk 1: only 0 and 2 are missing.
    {
        let req = TestRequest::get()
            .uri(&format!("/api/v1/upload/{session_id}/status"))
            .insert_header(("Authorization", format!("Bearer {bearer}")))
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let status: serde_json::Value = test::read_body_json(resp).await;
        let missing: Vec<u64> = status["missing_chunks"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_u64().unwrap())
            .collect();
        assert!(missing.contains(&0), "chunk 0 should be missing");
        assert!(missing.contains(&2), "chunk 2 should be missing");
        assert!(!missing.contains(&1), "chunk 1 should NOT be missing");
    }
}

// ─── Test 4: Resume after partial upload ──────────────────────────────────

#[actix_web::test]
async fn upload_resume_after_partial() {
    let app = common::make_app(true).await;
    common::post_setup_complete(&app, "admin4").await;
    let (bearer, root_id) = setup_user_with_root(&app, "user4").await;

    let chunk_size: u64 = 4;
    let data: &[u8] = b"AAAABBBBCCCC";
    let hash = sha256_hex(data);

    let prepared: serde_json::Value = {
        let req = TestRequest::post()
            .uri("/api/v1/upload/prepare")
            .insert_header(("Authorization", format!("Bearer {bearer}")))
            .set_json(serde_json::json!({
                "parent_id":   root_id,
                "file_name":   "resume.bin",
                "total_size":  data.len() as u64,
                "content_hash": hash,
                "chunk_size":   chunk_size,
            }))
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::OK);
        test::read_body_json(resp).await
    };
    let session_id = prepared["session_id"].as_str().unwrap();

    // Upload only chunks 0 and 2 — simulating an interrupted transfer.
    for (idx, chunk_data) in [(0u32, b"AAAA".as_ref()), (2, b"CCCC".as_ref())] {
        let req = TestRequest::post()
            .uri(&format!("/api/v1/upload/{session_id}/chunk/{idx}"))
            .insert_header(("Authorization", format!("Bearer {bearer}")))
            .insert_header(("Content-Type", "application/octet-stream"))
            .set_payload(Bytes::copy_from_slice(chunk_data))
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::OK);
    }

    // COMPLETE should fail — chunk 1 still missing.
    {
        let req = TestRequest::post()
            .uri(&format!("/api/v1/upload/{session_id}/complete"))
            .insert_header(("Authorization", format!("Bearer {bearer}")))
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY, "expected failure with missing chunks");
    }

    // Resume: upload the missing chunk 1.
    {
        let req = TestRequest::post()
            .uri(&format!("/api/v1/upload/{session_id}/chunk/1"))
            .insert_header(("Authorization", format!("Bearer {bearer}")))
            .insert_header(("Content-Type", "application/octet-stream"))
            .set_payload(Bytes::from_static(b"BBBB"))
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::OK);
    }

    // COMPLETE should now succeed.
    {
        let req = TestRequest::post()
            .uri(&format!("/api/v1/upload/{session_id}/complete"))
            .insert_header(("Authorization", format!("Bearer {bearer}")))
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::CREATED, "second complete failed");
        let node: serde_json::Value = test::read_body_json(resp).await;
        assert_eq!(node["name"], "resume.bin");
    }
}

// ─── Test 5: CANCEL removes session ────────────────────────────────────────

#[actix_web::test]
async fn cancel_removes_session() {
    let app = common::make_app(true).await;
    common::post_setup_complete(&app, "admin5").await;
    let (bearer, root_id) = setup_user_with_root(&app, "user5").await;

    let data = b"cancelled";
    let hash = sha256_hex(data);

    let prepared: serde_json::Value = {
        let req = TestRequest::post()
            .uri("/api/v1/upload/prepare")
            .insert_header(("Authorization", format!("Bearer {bearer}")))
            .set_json(serde_json::json!({
                "parent_id":   root_id,
                "file_name":   "cancel.bin",
                "total_size":  data.len() as u64,
                "content_hash": hash,
            }))
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::OK);
        test::read_body_json(resp).await
    };
    let session_id = prepared["session_id"].as_str().unwrap();

    // CANCEL.
    {
        let req = TestRequest::delete()
            .uri(&format!("/api/v1/upload/{session_id}"))
            .insert_header(("Authorization", format!("Bearer {bearer}")))
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    }

    // Subsequent STATUS should 404.
    {
        let req = TestRequest::get()
            .uri(&format!("/api/v1/upload/{session_id}/status"))
            .insert_header(("Authorization", format!("Bearer {bearer}")))
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }
}

// ─── Test 6: chunk_size clamping ──────────────────────────────────────────

#[actix_web::test]
async fn chunk_size_clamp() {
    let app = common::make_app(true).await;
    common::post_setup_complete(&app, "admin6").await;
    let (bearer, root_id) = setup_user_with_root(&app, "user6").await;

    let data = b"x";
    let hash = sha256_hex(data);
    // Request a chunk size that is larger than the 64 MiB maximum.
    let over_limit: u64 = 128 * 1024 * 1024; // 128 MiB

    let req = TestRequest::post()
        .uri("/api/v1/upload/prepare")
        .insert_header(("Authorization", format!("Bearer {bearer}")))
        .set_json(serde_json::json!({
            "parent_id":   root_id,
            "file_name":   "x.bin",
            "total_size":  1u64,
            "content_hash": hash,
            "chunk_size":   over_limit,
        }))
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), StatusCode::OK);

    let prepared: serde_json::Value = test::read_body_json(resp).await;
    let returned_chunk_size = prepared["chunk_size"].as_u64().unwrap();
    assert!(
        returned_chunk_size <= 64 * 1024 * 1024,
        "chunk_size was not clamped: {returned_chunk_size}"
    );
}

// ─── Test 7: dedup instant_complete ───────────────────────────────────────

#[actix_web::test]
async fn dedup_instant_complete() {
    let app = common::make_app(true).await;
    common::post_setup_complete(&app, "admin7").await;
    let (bearer, root_id) = setup_user_with_root(&app, "user7").await;

    let data = b"dedup content";
    let hash = sha256_hex(data);
    let total_size = data.len() as u64;

    // First upload: full pipeline.
    {
        let prepared: serde_json::Value = {
            let req = TestRequest::post()
                .uri("/api/v1/upload/prepare")
                .insert_header(("Authorization", format!("Bearer {bearer}")))
                .set_json(serde_json::json!({
                    "parent_id":   root_id,
                    "file_name":   "dedup1.bin",
                    "total_size":  total_size,
                    "content_hash": hash,
                }))
                .to_request();
            let resp = test::call_service(&app, req).await;
            assert_eq!(resp.status(), StatusCode::OK);
            test::read_body_json(resp).await
        };
        let session_id = prepared["session_id"].as_str().unwrap();

        // Upload the single chunk.
        let req = TestRequest::post()
            .uri(&format!("/api/v1/upload/{session_id}/chunk/0"))
            .insert_header(("Authorization", format!("Bearer {bearer}")))
            .insert_header(("Content-Type", "application/octet-stream"))
            .set_payload(Bytes::from_static(data))
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::OK);

        // Complete.
        let req = TestRequest::post()
            .uri(&format!("/api/v1/upload/{session_id}/complete"))
            .insert_header(("Authorization", format!("Bearer {bearer}")))
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::CREATED);
    }

    // Second upload of the same content: should be instant_complete.
    {
        let req = TestRequest::post()
            .uri("/api/v1/upload/prepare")
            .insert_header(("Authorization", format!("Bearer {bearer}")))
            .set_json(serde_json::json!({
                "parent_id":   root_id,
                "file_name":   "dedup2.bin",
                "total_size":  total_size,
                "content_hash": hash,
            }))
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::OK);

        let prepared: serde_json::Value = test::read_body_json(resp).await;
        assert_eq!(
            prepared["instant_complete"], true,
            "expected instant_complete on second upload of same hash"
        );
        assert!(prepared["file_node"].is_object(), "file_node not present on dedup hit");
    }
}
