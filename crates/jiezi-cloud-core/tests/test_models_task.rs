use chrono::Utc;
use jiezi_cloud_core::models::task::{BackgroundTask, TaskKind, TaskStatus};
use jiezi_cloud_core::types::{TaskId, UserId};

fn make_task(status: TaskStatus) -> BackgroundTask {
    BackgroundTask {
        id: TaskId::new(),
        kind: TaskKind::SearchIndexRebuild,
        status,
        owner_id: Some(UserId::new()),
        progress: 0,
        error_message: None,
        created_at: Utc::now(),
        updated_at: Utc::now(),
        completed_at: None,
    }
}

#[test]
fn test_terminal_statuses() {
    assert!(TaskStatus::Completed.is_terminal());
    assert!(TaskStatus::Failed.is_terminal());
    assert!(TaskStatus::Cancelled.is_terminal());
}

#[test]
fn test_non_terminal_statuses() {
    assert!(!TaskStatus::Pending.is_terminal());
    assert!(!TaskStatus::Running.is_terminal());
}

#[test]
fn test_task_status_serde_round_trip() {
    for status in [
        TaskStatus::Pending,
        TaskStatus::Running,
        TaskStatus::Completed,
        TaskStatus::Failed,
        TaskStatus::Cancelled,
    ] {
        let json = serde_json::to_string(&status).expect("serialize");
        let back: TaskStatus = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(status, back);
    }
}

#[test]
fn test_task_kind_file_index_serde() {
    let kind = TaskKind::FileIndex { file_id: "abc-123".into() };
    let json = serde_json::to_string(&kind).expect("serialize");
    assert!(json.contains("file_index"));
    let back: TaskKind = serde_json::from_str(&json).expect("deserialize");
    assert!(matches!(back, TaskKind::FileIndex { .. }));
}

#[test]
fn test_background_task_serde_round_trip() {
    let task = make_task(TaskStatus::Running);
    let json = serde_json::to_string(&task).expect("serialize task");
    let back: BackgroundTask = serde_json::from_str(&json).expect("deserialize task");
    assert_eq!(task.id, back.id);
    assert_eq!(task.status, back.status);
}
