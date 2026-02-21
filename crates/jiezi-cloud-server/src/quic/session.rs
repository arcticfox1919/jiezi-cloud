//! JTP/1 session manager.
//!
//! A "session" represents a single in-progress upload or download negotiated
//! over the control stream.  Multiple sessions can coexist on the same QUIC
//! connection.
//!
//! # Session ID
//!
//! Session IDs are UUID v4 values chosen by the client.  The server trusts the
//! ID but enforces uniqueness per connection.
//!
//! # Design
//!
//! Upload sessions buffer incoming chunk bytes in a `BTreeMap<chunk_index,
//! Bytes>` so that they can be reassembled in order when all chunks have
//! arrived.  The full assembled bytes are then handed to `UploadService`.
//!
//! Download sessions track push progress only (all bytes live in storage).
//!
//! # Concurrency
//!
//! A single `DashMap` houses all session states.  `DashMap` uses lock-striping
//! so independent sessions never contend.  Share via `Arc<SessionManager>`.
//!
//! # Garbage collection
//!
//! Call [`SessionManager::spawn_gc`] once per connection to start a background
//! task that periodically reaps completed / stalled sessions.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::{Bytes, BytesMut};
use dashmap::DashMap;
use tokio::time;
use tracing::debug;
use uuid::Uuid;

use jiezi_cloud_core::protocol::frames::ErrorCode;

// ─── Session state ────────────────────────────────────────────────────────────

/// State of a single JTP/1 session.
#[derive(Debug)]
pub enum SessionState {
    /// Upload session: buffering incoming chunk bytes in memory.
    Uploading {
        file_name: String,
        total_size: u64,
        content_hash: [u8; 32],
        chunk_count: u32,
        /// Received chunk bytes, keyed by chunk index (0-based).
        /// When `received_chunks.len() == chunk_count`, the upload is ready
        /// to be assembled and persisted.
        received_chunks: BTreeMap<u32, Bytes>,
        created_at: Instant,
    },
    /// Download session: server is pushing chunk streams to the client.
    Downloading {
        file_node_id: Uuid,
        total_size: u64,
        chunk_count: u32,
        chunks_sent: u32,
        bytes_sent: u64,
        created_at: Instant,
    },
    /// Both sides have confirmed completion; the session can be reaped.
    Completed {
        file_node_id: Uuid,
        completed_at: Instant,
    },
    /// Session was cancelled or errored.
    Cancelled {
        reason: String,
        cancelled_at: Instant,
    },
}

impl SessionState {
    /// Returns `true` if this session is still actively transferring data.
    pub fn is_active(&self) -> bool {
        matches!(self, Self::Uploading { .. } | Self::Downloading { .. })
    }

    /// If this is an Uploading session, return true when all chunks have arrived.
    pub fn is_upload_complete(&self) -> bool {
        match self {
            Self::Uploading { chunk_count, received_chunks, .. } => {
                received_chunks.len() == *chunk_count as usize
            }
            _ => false,
        }
    }
}

// ─── Session manager ──────────────────────────────────────────────────────────

/// Per-connection session manager.
///
/// Manages the lifecycle of all JTP/1 sessions on one QUIC connection.
/// Each connection gets its own `SessionManager`.
///
/// Completed and cancelled sessions are kept for `TTL_SECS` seconds so that
/// latecomers (e.g. a `COMPLETE_ACK` arriving after GC) get a proper
/// `SessionExpired` error instead of `UnknownSession`.
pub struct SessionManager {
    sessions: DashMap<Uuid, SessionState>,
}

/// How long to keep completed / cancelled session records before reaping.
const TTL_SECS: u64 = 120;

/// How long to keep stalled (incomplete) upload sessions before GC.
const UPLOAD_STALL_SECS: u64 = 600;

/// GC cycle interval.
const GC_INTERVAL: Duration = Duration::from_secs(60);

impl SessionManager {
    /// Create an empty session manager.
    pub fn new() -> Self {
        Self {
            sessions: DashMap::new(),
        }
    }

    /// Spawn a background task that periodically garbage-collects expired
    /// sessions.  The task stops automatically when the `Arc` is the last
    /// remaining reference (i.e. the connection has been dropped).
    pub fn spawn_gc(self: &Arc<Self>) {
        let weak = Arc::downgrade(self);
        tokio::spawn(async move {
            let mut interval = time::interval(GC_INTERVAL);
            loop {
                interval.tick().await;
                match weak.upgrade() {
                    Some(mgr) => mgr.gc(),
                    None => break, // SessionManager dropped — stop GC.
                }
            }
        });
    }

    // ── Insert helpers ────────────────────────────────────────────────────────

    /// Register a new upload session.
    ///
    /// Returns `Err(ErrorCode::AlreadyExists)` if the session ID is already
    /// known (active or recently completed).
    pub fn insert_upload(
        &self,
        session_id: Uuid,
        file_name: String,
        total_size: u64,
        content_hash: [u8; 32],
        chunk_count: u32,
    ) -> Result<(), ErrorCode> {
        use dashmap::mapref::entry::Entry;
        match self.sessions.entry(session_id) {
            Entry::Occupied(_) => Err(ErrorCode::AlreadyExists),
            Entry::Vacant(v) => {
                v.insert(SessionState::Uploading {
                    file_name,
                    total_size,
                    content_hash,
                    chunk_count,
                    received_chunks: BTreeMap::new(),
                    created_at: Instant::now(),
                });
                Ok(())
            }
        }
    }

    /// Register a new download session.
    ///
    /// Returns `Err(ErrorCode::AlreadyExists)` if the session ID is already
    /// known.
    pub fn insert_download(
        &self,
        session_id: Uuid,
        file_node_id: Uuid,
        total_size: u64,
        chunk_count: u32,
    ) -> Result<(), ErrorCode> {
        use dashmap::mapref::entry::Entry;
        match self.sessions.entry(session_id) {
            Entry::Occupied(_) => Err(ErrorCode::AlreadyExists),
            Entry::Vacant(v) => {
                v.insert(SessionState::Downloading {
                    file_node_id,
                    total_size,
                    chunk_count,
                    chunks_sent: 0,
                    bytes_sent: 0,
                    created_at: Instant::now(),
                });
                Ok(())
            }
        }
    }

    // ── Chunk accounting ──────────────────────────────────────────────────────

    /// Store one received chunk in an upload session.
    ///
    /// Returns `Ok(true)` when all chunks have arrived and the file is ready
    /// to be assembled; `Ok(false)` if more chunks are still expected.
    pub fn store_chunk(
        &self,
        session_id: Uuid,
        chunk_index: u32,
        data: Bytes,
    ) -> Result<bool, ErrorCode> {
        let mut entry = self
            .sessions
            .get_mut(&session_id)
            .ok_or(ErrorCode::UnknownSession)?;

        match &mut *entry {
            SessionState::Uploading { received_chunks, chunk_count, .. } => {
                received_chunks.insert(chunk_index, data);
                Ok(received_chunks.len() == (*chunk_count) as usize)
            }
            _ => Err(ErrorCode::UnknownSession),
        }
    }

    /// Atomically remove a completed upload session and return its assembled
    /// file bytes.
    ///
    /// Uses [`DashMap::remove_if`] to avoid the TOCTOU race between checking
    /// completion and removing the entry — no other task can sneak in between.
    ///
    /// Returns `None` if the session is not found, not an upload, or not yet
    /// complete.
    pub fn take_assembled(
        &self,
        session_id: Uuid,
    ) -> Option<(String, Bytes, [u8; 32])> {
        // Atomic check-and-remove: only removes if the upload is complete.
        let (_, state) = self
            .sessions
            .remove_if(&session_id, |_, s| s.is_upload_complete())?;

        if let SessionState::Uploading { file_name, received_chunks, content_hash, .. } = state {
            let total: usize = received_chunks.values().map(|b| b.len()).sum();
            let mut buf = BytesMut::with_capacity(total);
            for chunk in received_chunks.values() {
                buf.extend_from_slice(chunk);
            }
            Some((file_name, buf.freeze(), content_hash))
        } else {
            None
        }
    }

    /// Returns `true` if all chunks have been received for an upload session.
    pub fn is_upload_complete(&self, session_id: Uuid) -> bool {
        self.sessions
            .get(&session_id)
            .map(|s| (*s).is_upload_complete())
            .unwrap_or(false)
    }

    /// Returns current upload progress as `(received_count, total_count)`.
    pub fn upload_progress(&self, session_id: Uuid) -> Option<(u32, u32)> {
        self.sessions.get(&session_id).and_then(|s| {
            if let SessionState::Uploading { received_chunks, chunk_count, .. } = &*s {
                Some((received_chunks.len() as u32, *chunk_count))
            } else {
                None
            }
        })
    }

    /// Advance download progress counters.
    pub fn record_chunk_sent(
        &self,
        session_id: Uuid,
        chunk_bytes: u64,
    ) -> Result<(u32, u64), ErrorCode> {
        let mut entry = self
            .sessions
            .get_mut(&session_id)
            .ok_or(ErrorCode::UnknownSession)?;

        if let SessionState::Downloading {
            ref mut chunks_sent,
            ref mut bytes_sent,
            ..
        } = *entry {
            *chunks_sent += 1;
            *bytes_sent += chunk_bytes;
            return Ok((*chunks_sent, *bytes_sent));
        }
        Err(ErrorCode::UnknownSession)
    }

    /// Returns current download progress as `(chunks_sent, bytes_sent)`.
    pub fn download_progress(&self, session_id: Uuid) -> Option<(u32, u64)> {
        self.sessions.get(&session_id).and_then(|s| {
            if let SessionState::Downloading { chunks_sent, bytes_sent, .. } = &*s {
                Some((*chunks_sent, *bytes_sent))
            } else {
                None
            }
        })
    }

    // ── State transitions ─────────────────────────────────────────────────────

    /// Mark a session as completed.
    pub fn mark_completed(&self, session_id: Uuid, file_node_id: Uuid) {
        self.sessions.insert(
            session_id,
            SessionState::Completed {
                file_node_id,
                completed_at: Instant::now(),
            },
        );
    }

    /// Mark a session as cancelled.
    pub fn mark_cancelled(&self, session_id: Uuid, reason: impl Into<String>) {
        self.sessions.insert(
            session_id,
            SessionState::Cancelled {
                reason: reason.into(),
                cancelled_at: Instant::now(),
            },
        );
    }

    // ── Lookup ────────────────────────────────────────────────────────────────

    /// Return the error code the client should receive when addressing this
    /// session after it has ended, or `None` if the session is still active.
    pub fn terminal_error(&self, session_id: Uuid) -> Option<ErrorCode> {
        self.sessions.get(&session_id).and_then(|s| match &*s {
            SessionState::Completed { .. } => None, // still valid to send CompleteAck
            SessionState::Cancelled { .. } => Some(ErrorCode::SessionExpired),
            SessionState::Uploading { .. } | SessionState::Downloading { .. } => None,
        })
    }

    // ── Garbage collection ────────────────────────────────────────────────────

    /// Reap terminal sessions older than [`TTL_SECS`] and stalled upload
    /// sessions older than [`UPLOAD_STALL_SECS`].
    fn gc(&self) {
        let terminal_cutoff = Duration::from_secs(TTL_SECS);
        let stall_cutoff    = Duration::from_secs(UPLOAD_STALL_SECS);
        let before = self.sessions.len();
        self.sessions.retain(|_, state| match state {
            SessionState::Completed { completed_at, .. } => completed_at.elapsed() < terminal_cutoff,
            SessionState::Cancelled { cancelled_at, .. } => cancelled_at.elapsed() < terminal_cutoff,
            SessionState::Uploading  { created_at, .. }  => created_at.elapsed() < stall_cutoff,
            SessionState::Downloading { created_at, .. } => created_at.elapsed() < stall_cutoff,
        });
        let reaped = before - self.sessions.len();
        if reaped > 0 {
            debug!(reaped, remaining = self.sessions.len(), "session GC cycle");
        }
    }

    /// Returns the number of currently active sessions.
    pub fn active_count(&self) -> usize {
        self.sessions.iter().filter(|e| e.is_active()).count()
    }
}

impl Default for SessionManager {
    fn default() -> Self {
        Self::new()
    }
}
