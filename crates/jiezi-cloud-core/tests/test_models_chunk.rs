use chrono::Utc;
use jiezi_cloud_core::models::chunk::{ChunkInfo, StorageRef, UploadSession};
use jiezi_cloud_core::types::{BackendId, FileId};

fn make_session(completed: u32, total: u32) -> UploadSession {
    UploadSession {
        upload_id: "sess-abc123".into(),
        file_id: FileId::new(),
        total_size: 1_000_000,
        completed_chunks: completed,
        total_chunks: total,
        created_at: Utc::now(),
    }
}

#[test]
fn test_chunk_info_serde_round_trip() {
    let chunk = ChunkInfo { hash: "deadbeef".into(), size: 4096, offset: 0, index: 0 };
    let json = serde_json::to_string(&chunk).expect("serialize");
    let back: ChunkInfo = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(chunk, back);
}

#[test]
fn test_upload_session_is_complete_when_all_chunks_received() {
    let session = make_session(10, 10);
    assert!(session.is_complete());
}

#[test]
fn test_upload_session_not_complete_while_in_progress() {
    let session = make_session(7, 10);
    assert!(!session.is_complete());
}

#[test]
fn test_upload_session_progress_range() {
    let session = make_session(5, 10);
    let p = session.progress();
    assert!((p - 0.5).abs() < f64::EPSILON);
}

#[test]
fn test_upload_session_progress_zero_total() {
    let session = make_session(0, 0);
    assert_eq!(session.progress(), 1.0);
}

#[test]
fn test_storage_ref_serde_round_trip() {
    let sr = StorageRef {
        chunk_hash: "abc123".into(),
        backend_id: BackendId::new("local"),
        key: "ab/abc123456".into(),
        size: 4096,
    };
    let json = serde_json::to_string(&sr).expect("serialize");
    let back: StorageRef = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(sr.chunk_hash, back.chunk_hash);
    assert_eq!(sr.key, back.key);
}
