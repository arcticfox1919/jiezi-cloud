//! Core primitive types shared across all Jiezi Cloud crates.
//!
//! # Design decisions
//!
//! - **Newtype IDs** — wrapping [`Uuid`] prevents accidentally passing a
//!   `FileId` where a `UserId` is expected, catching entire classes of bugs at
//!   compile time.
//! - **UUIDv7** — time-ordered UUIDs are database-friendly (good B-tree
//!   locality) and client-sortable without an extra timestamp column.
//! - **Pagination** — `PageRequest` / `PageResponse` are generic to keep
//!   presentation consistent across every listing endpoint.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ─── Newtype ID macro ─────────────────────────────────────────────────────────

/// Generates a newtype wrapper around [`Uuid`] with all standard derives,
/// a `new()` constructor (UUIDv7), display and `FromStr` implementations.
macro_rules! newtype_uuid_id {
    (
        $(#[$attr:meta])*
        $name:ident
    ) => {
        $(#[$attr])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        pub struct $name(pub Uuid);

        impl $name {
            /// Create a new time-ordered UUIDv7 identifier.
            pub fn new() -> Self {
                Self(Uuid::now_v7())
            }

            /// Return the inner raw [`Uuid`] value.
            #[inline]
            pub fn into_inner(self) -> Uuid {
                self.0
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{}", self.0)
            }
        }

        impl std::str::FromStr for $name {
            type Err = uuid::Error;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Uuid::parse_str(s).map(Self)
            }
        }

        impl From<Uuid> for $name {
            fn from(id: Uuid) -> Self {
                Self(id)
            }
        }
    };
}

// ─── Domain ID types ─────────────────────────────────────────────────────────

newtype_uuid_id!(
    /// Identifies a user account.
    UserId
);

newtype_uuid_id!(
    /// Identifies a file or directory node in the virtual file system.
    FileId
);

newtype_uuid_id!(
    /// Identifies a collaborative space (personal or shared).
    SpaceId
);

newtype_uuid_id!(
    /// Identifies a share link.
    ShareId
);

newtype_uuid_id!(
    /// Identifies a deduplicated content chunk.
    ChunkId
);

newtype_uuid_id!(
    /// Identifies a background task.
    TaskId
);

// ─── Non-UUID ID types ────────────────────────────────────────────────────────

/// Identifier for a physical storage backend (human-readable slug).
///
/// Example: `"primary-disk"`, `"s3-backup"`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct BackendId(pub String);

impl BackendId {
    /// Create a backend ID from any string-like value.
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// Return the identifier as a string slice.
    #[inline]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for BackendId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<String> for BackendId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for BackendId {
    fn from(s: &str) -> Self {
        Self(s.to_owned())
    }
}

// ─── Health status ────────────────────────────────────────────────────────────

/// Health status reported by subsystems such as storage backends and the DB pool.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum HealthStatus {
    /// The subsystem is operating normally.
    Healthy,
    /// The subsystem is operational but partially impaired.
    Degraded {
        /// Human-readable description of the degraded condition.
        reason: String,
    },
    /// The subsystem is not operational.
    Unhealthy {
        /// Human-readable description of the failure.
        reason: String,
    },
}

impl HealthStatus {
    /// Return `true` if the status is [`HealthStatus::Healthy`].
    pub fn is_healthy(&self) -> bool {
        matches!(self, HealthStatus::Healthy)
    }
}

// ─── Pagination ───────────────────────────────────────────────────────────────

/// Pagination parameters sent by the client in query strings or request bodies.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageRequest {
    /// 1-based page number.
    pub page: u32,
    /// Maximum number of items per page.
    pub per_page: u32,
}

impl PageRequest {
    /// Create a new page request.
    pub fn new(page: u32, per_page: u32) -> Self {
        Self { page, per_page }
    }

    /// Calculate the SQL `OFFSET` for this page.
    #[inline]
    pub fn offset(&self) -> u64 {
        u64::from(self.page.saturating_sub(1)) * u64::from(self.per_page)
    }

    /// Return the SQL `LIMIT` (same as `per_page`).
    #[inline]
    pub fn limit(&self) -> u64 {
        u64::from(self.per_page)
    }
}

impl Default for PageRequest {
    fn default() -> Self {
        Self { page: 1, per_page: 20 }
    }
}

/// A page of results returned from a listing operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageResponse<T> {
    /// Items on this page.
    pub items: Vec<T>,
    /// Total number of matching items across all pages.
    pub total: u64,
    /// Current page number (matches the request).
    pub page: u32,
    /// Items per page (matches the request).
    pub per_page: u32,
}

impl<T> PageResponse<T> {
    /// Construct a page response.
    pub fn new(items: Vec<T>, total: u64, page: u32, per_page: u32) -> Self {
        Self { items, total, page, per_page }
    }

    /// Calculate the total number of pages.
    pub fn total_pages(&self) -> u64 {
        if self.per_page == 0 {
            return 0;
        }
        self.total.div_ceil(u64::from(self.per_page))
    }

    /// Return `true` if there is a next page.
    pub fn has_next(&self) -> bool {
        u64::from(self.page) < self.total_pages()
    }
}

// ─── Permission types ─────────────────────────────────────────────────────────

/// A reference to a resource on which an action may be checked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ResourceRef {
    /// A file or directory node.
    File(FileId),
    /// A collaborative or personal space.
    Space(SpaceId),
    /// A share link.
    Share(ShareId),
    /// A system-level resource (e.g.  server configuration).
    System,
}

/// An action a user may attempt to perform.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    /// Read / download the resource.
    Read,
    /// Write / modify the resource.
    Write,
    /// Delete the resource.
    Delete,
    /// Share the resource with others.
    Share,
    /// Administrative operations (e.g. space management).
    Admin,
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    // ── Newtype ID tests ──────────────────────────────────────────────────────

    #[test]
    fn test_new_ids_are_unique() {
        let a = UserId::new();
        let b = UserId::new();
        assert_ne!(a, b, "successive IDs must be distinct");
    }

    #[test]
    fn test_uuid_v7_ids_are_time_ordered() {
        // UUIDv7 embeds a millisecond timestamp in the most-significant bits,
        // so later calls should produce lexicographically larger values.
        let a = UserId::new();
        // Introduce a tiny sleep so the timestamp portion differs.
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

    // ── BackendId tests ───────────────────────────────────────────────────────

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

    // ── PageRequest tests ─────────────────────────────────────────────────────

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

    // ── PageResponse tests ────────────────────────────────────────────────────

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

    // ── HealthStatus tests ────────────────────────────────────────────────────

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
}
