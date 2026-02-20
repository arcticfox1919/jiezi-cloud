use std::str::FromStr;

use jiezi_cloud_core::types::{
    BackendId, ChunkId, FileId, HealthStatus, PageRequest, PageResponse, SpaceId, UserId,
};
use uuid::Uuid;

// ── Newtype ID tests ──────────────────────────────────────────────────────────

#[test]
fn test_new_ids_are_unique() {
    let a = UserId::new();
    let b = UserId::new();
    assert_ne!(a, b, "successive IDs must be distinct");
}

#[test]
fn test_uuid_v7_ids_are_time_ordered() {
    let a = UserId::new();
    std::thread::sleep(std::time::Duration::from_millis(2));
    let b = UserId::new();
    assert!(a < b, "UUIDv7 IDs must be monotonically increasing");
}

#[test]
fn test_id_json_round_trip() {
    let id = FileId::new();
    let json = serde_json::to_string(&id).expect("serialize");
    let id2: FileId = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(id, id2);
}

#[test]
fn test_id_display_and_from_str_round_trip() {
    let id = SpaceId::new();
    let s = id.to_string();
    let id2 = SpaceId::from_str(&s).expect("parse");
    assert_eq!(id, id2);
}

#[test]
fn test_id_from_uuid() {
    let raw = Uuid::now_v7();
    let file_id = FileId::from(raw);
    assert_eq!(file_id.into_inner(), raw);
}

#[test]
fn test_id_copy_semantics() {
    let id = ChunkId::new();
    let copy = id; // Copy, not move
    assert_eq!(id, copy);
}

// ── BackendId tests ───────────────────────────────────────────────────────────

#[test]
fn test_backend_id_new_and_as_str() {
    let id = BackendId::new("primary-disk");
    assert_eq!(id.as_str(), "primary-disk");
}

#[test]
fn test_backend_id_display() {
    let id = BackendId::from("s3-backup");
    assert_eq!(id.to_string(), "s3-backup");
}

#[test]
fn test_backend_id_json_round_trip() {
    let id = BackendId::new("webdav-nas");
    let json = serde_json::to_string(&id).expect("serialize");
    let id2: BackendId = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(id, id2);
}

// ── PageRequest tests ─────────────────────────────────────────────────────────

#[test]
fn test_page_request_offset_first_page() {
    let req = PageRequest::new(1, 20);
    assert_eq!(req.offset(), 0);
}

#[test]
fn test_page_request_offset_second_page() {
    let req = PageRequest::new(2, 20);
    assert_eq!(req.offset(), 20);
}

#[test]
fn test_page_request_offset_third_page_smaller_size() {
    let req = PageRequest::new(3, 10);
    assert_eq!(req.offset(), 20);
}

#[test]
fn test_page_request_limit_matches_per_page() {
    let req = PageRequest::new(5, 50);
    assert_eq!(req.limit(), 50);
}

// ── PageResponse tests ────────────────────────────────────────────────────────

#[test]
fn test_page_response_total_pages_exact_fit() {
    let resp: PageResponse<i32> = PageResponse::new(vec![], 100, 1, 20);
    assert_eq!(resp.total_pages(), 5);
}

#[test]
fn test_page_response_total_pages_with_remainder() {
    let resp: PageResponse<i32> = PageResponse::new(vec![], 101, 1, 20);
    assert_eq!(resp.total_pages(), 6);
}

#[test]
fn test_page_response_total_pages_empty() {
    let resp: PageResponse<i32> = PageResponse::new(vec![], 0, 1, 20);
    assert_eq!(resp.total_pages(), 0);
}

#[test]
fn test_page_response_has_next() {
    let resp: PageResponse<i32> = PageResponse::new(vec![], 100, 1, 20);
    assert!(resp.has_next());

    let last_page: PageResponse<i32> = PageResponse::new(vec![], 100, 5, 20);
    assert!(!last_page.has_next());
}

// ── HealthStatus tests ────────────────────────────────────────────────────────

#[test]
fn test_health_status_is_healthy() {
    assert!(HealthStatus::Healthy.is_healthy());
    assert!(!HealthStatus::Degraded { reason: "disk almost full".into() }.is_healthy());
    assert!(!HealthStatus::Unhealthy { reason: "disk offline".into() }.is_healthy());
}

#[test]
fn test_health_status_json_round_trip() {
    let status = HealthStatus::Degraded { reason: "high latency".into() };
    let json = serde_json::to_string(&status).expect("serialize");
    let status2: HealthStatus = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(status, status2);
}
