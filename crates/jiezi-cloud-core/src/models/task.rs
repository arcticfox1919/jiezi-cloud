//! Background task models for tracking async, long-running operations.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::types::{TaskId, UserId};

// ─── TaskStatus ───────────────────────────────────────────────────────────────

/// Lifecycle state of a background task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    /// Waiting to be picked up by a worker.
    Pending,
    /// Currently executing.
    Running,
    /// Finished successfully.
    Completed,
    /// Failed with an error; see `error_message` on the task.
    Failed,
    /// Cancelled before completion.
    Cancelled,
}

impl TaskStatus {
    /// Return `true` if the task has reached a terminal state.
    pub fn is_terminal(self) -> bool {
        matches!(self, TaskStatus::Completed | TaskStatus::Failed | TaskStatus::Cancelled)
    }
}

// ─── TaskKind ─────────────────────────────────────────────────────────────────

/// The type of work a background task performs.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TaskKind {
    /// Index the content of a file for full-text search.
    FileIndex { file_id: String },
    /// Generate a thumbnail image for a file.
    ThumbnailGeneration { file_id: String },
    /// Transcode a video file to a target format.
    VideoTranscode { file_id: String, target_format: String },
    /// Rebuild the entire search index from the database.
    SearchIndexRebuild,
    /// Replicate a chunk to an additional storage backend.
    StorageReplication { chunk_hash: String, target_backend: String },
}

// ─── BackgroundTask ───────────────────────────────────────────────────────────

/// A trackable unit of async work.
///
/// Tasks are persisted in the database so they survive server restarts and can
/// be queried by the user or monitoring systems.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackgroundTask {
    pub id: TaskId,
    pub kind: TaskKind,
    pub status: TaskStatus,
    /// The user who triggered this task, if applicable.
    pub owner_id: Option<UserId>,
    /// Percentage of completion (0–100).
    pub progress: u8,
    /// Human-readable error message if `status == Failed`.
    pub error_message: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// Set when the task reaches a terminal state.
    pub completed_at: Option<DateTime<Utc>>,
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{TaskId, UserId};

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
}
