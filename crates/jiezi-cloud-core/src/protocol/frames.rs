//! JTP/1 — Jiezi Transfer Protocol v1, frame type definitions.
//!
//! # Protocol Overview
//!
//! JTP/1 is a binary, session-oriented file-transfer protocol that runs over
//! QUIC streams.  It provides:
//!
//! - **Resumable uploads** — server tells the client which chunks are missing;
//!   the client only retransmits those chunks.
//! - **Instant upload (deduplication)** — if the server already has a file
//!   with the same SHA-256 hash, `UPLOAD_INSTANT` is returned immediately and
//!   no bytes need to be transferred.
//! - **Parallel chunk streams** — a fixed pool of N bidirectional QUIC
//!   streams (negotiated in `UPLOAD_ACCEPT`, typically 8) is multiplexed across
//!   all chunks.  This avoids the per-chunk stream-open overhead while still
//!   saturating high-bandwidth links.  A sliding window of `window_size` chunks
//!   may be in-flight simultaneously; `CHUNK_ACK` advances the window rather
//!   than gating each send.
//! - **Byte-range downloads** — clients can request arbitrary byte ranges
//!   rather than always downloading the entire file.
//! - **Progress notifications** — the server sends `PROGRESS` frames
//!   periodically so clients can display progress bars without polling.
//! - **Graceful cancellation** — either party can cancel a session mid-way;
//!   in-flight streams are cleanly reset.
//!
//! # Stream Roles
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────┐
//! │                        QUIC Connection                          │
//! │                                                                 │
//! │  Stream 0   (client-bidi)  ←─ Control stream, persistent       │
//! │                                                                 │
//! │  Streams 4,8,…(client-bidi)  ←─ Upload chunk streams           │
//! │             one per in-flight chunk; closed after CHUNK_ACK     │
//! │                                                                 │
//! │  Streams 3,7,…(server-uni)   ←─ Download chunk streams         │
//! │             server opens one per chunk being pushed             │
//! └─────────────────────────────────────────────────────────────────┘
//! ```
//!
//! # Wire Format
//!
//! Every frame on every stream is preceded by a **5-byte header**:
//!
//! ```text
//! ┌──────────┬──────────────────────┬─────────────────────────┐
//! │ type: u8 │ payload_len: u32 BE  │ payload: [u8; len]      │
//! └──────────┴──────────────────────┴─────────────────────────┘
//!   1 byte        4 bytes                  variable
//! ```
//!
//! All multi-byte integers in payloads are **big-endian**.
//!
//! Strings are length-prefixed: `[len: u16 BE][utf-8 bytes]`.
//! The maximum string length is 65 535 bytes.
//!
//! Session IDs and file IDs are 16-byte UUID v4 values (no dashes).
//! Content hashes are 32-byte raw SHA-256 digests.
//!
//! # Session Lifecycle — Upload
//!
//! ```text
//! Client                              Server
//!   │── UPLOAD_REQUEST ──────────────▶│  examine content_hash
//!   │                                 │
//!   │◀── UPLOAD_INSTANT ──────────────│  (dedup hit — nothing to transfer)
//!   │             ── or ──            │
//!   │◀── UPLOAD_ACCEPT ───────────────│  { missing_chunks, parallel=8, window=16 }
//!   │                                 │
//!   │  (open `parallel` bidi streams, │
//!   │   send up to `window` chunks    │
//!   │   without waiting for ACK)      │
//!   │── [stream-1] CHUNK_DATA ───────▶│  chunk 0
//!   │── [stream-2] CHUNK_DATA ───────▶│  chunk 1  ← in-flight simultaneously
//!   │── [stream-3] CHUNK_DATA ───────▶│  chunk 2
//!   │◀── [stream-1] CHUNK_ACK ────────│  ok → advance window, reuse stream-1
//!   │── [stream-1] CHUNK_DATA ───────▶│  chunk 8
//!   │                                 │
//!   │◀── PROGRESS (periodic) ─────────│  (on control stream)
//!   │                                 │
//!   │◀── COMPLETE ────────────────────│  { file_node_id }
//!   │── COMPLETE_ACK ────────────────▶│
//! ```
//!
//! # Session Lifecycle — Download
//!
//! ```text
//! Client                              Server
//!   │── DOWNLOAD_REQUEST ────────────▶│  { byte_ranges=[] = full file }
//!   │◀── DOWNLOAD_INFO ───────────────│  { total_size, chunk_count, mime_type, parallel=8 }
//!   │                                 │
//!   │  (server opens `parallel`       │
//!   │   uni streams, reuses them      │
//!   │   across all chunks)            │
//!   │◀── [stream-1] CHUNK_DATA ───────│  chunk 0
//!   │◀── [stream-2] CHUNK_DATA ───────│  chunk 1  ← in-flight simultaneously
//!   │◀── [stream-3] CHUNK_DATA ───────│  chunk 2
//!   │                                 │
//!   │◀── PROGRESS (periodic) ─────────│  (on control stream)
//!   │                                 │
//!   │◀── DOWNLOAD_COMPLETE ───────────│
//!   │── COMPLETE_ACK ────────────────▶│
//! ```

use bytes::Bytes;
use uuid::Uuid;

// ─── Frame type constants ─────────────────────────────────────────────────────

/// Frame type byte values used in the 5-byte header.
///
/// Reserved ranges:
///   0x01–0x0F  Handshake
///   0x10–0x1F  Upload control (on control stream)
///   0x20–0x2F  Download control (on control stream)
///   0x30–0x3F  Progress / notifications
///   0x40–0x4F  Completion
///   0x50–0x5F  Cancellation
///   0x80–0x8F  Data (on dedicated chunk streams)
///   0xF0–0xFF  Meta (ping/pong/error)
pub mod frame_type {
    pub const HELLO:             u8 = 0x01;
    pub const HELLO_ACK:         u8 = 0x02;

    pub const UPLOAD_REQUEST:    u8 = 0x10;
    pub const UPLOAD_ACCEPT:     u8 = 0x11;
    pub const UPLOAD_INSTANT:    u8 = 0x12;  // dedup hit — no bytes needed

    pub const DOWNLOAD_REQUEST:  u8 = 0x20;
    pub const DOWNLOAD_INFO:     u8 = 0x21;

    pub const PROGRESS:          u8 = 0x30;

    pub const COMPLETE:          u8 = 0x40;
    pub const COMPLETE_ACK:      u8 = 0x41;
    pub const DOWNLOAD_COMPLETE: u8 = 0x42;

    pub const CANCEL:            u8 = 0x50;

    pub const CHUNK_DATA:        u8 = 0x80;
    pub const CHUNK_ACK:         u8 = 0x81;

    pub const PING:              u8 = 0xF0;
    pub const PONG:              u8 = 0xF1;
    pub const ERROR:             u8 = 0xFE;
}

// ─── Error codes ─────────────────────────────────────────────────────────────

/// Standardised error codes carried in [`ErrorFrame`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum ErrorCode {
    /// Internal server error not covered by a more specific code.
    Internal          = 0x0001,
    /// The JWT token is missing, expired, or has an invalid signature.
    Unauthorized      = 0x0010,
    /// The requested file or session does not exist.
    NotFound          = 0x0020,
    /// The file already exists and overwrite was not requested.
    AlreadyExists     = 0x0021,
    /// The chunk checksum did not match; client must retransmit.
    ChecksumMismatch  = 0x0030,
    /// The session has expired and been garbage-collected.
    SessionExpired    = 0x0031,
    /// The session ID in the frame is unknown.
    UnknownSession    = 0x0032,
    /// The payload length exceeds the server's configured maximum.
    PayloadTooLarge   = 0x0040,
    /// The JTP version in HELLO is not supported.
    VersionMismatch   = 0x0050,
    /// The server is shutting down or overloaded; try again later.
    Unavailable       = 0x00FF,
    /// Unknown error code received from the peer.
    Unknown           = 0xFFFF,
}

impl From<u16> for ErrorCode {
    fn from(v: u16) -> Self {
        match v {
            0x0001 => Self::Internal,
            0x0010 => Self::Unauthorized,
            0x0020 => Self::NotFound,
            0x0021 => Self::AlreadyExists,
            0x0030 => Self::ChecksumMismatch,
            0x0031 => Self::SessionExpired,
            0x0032 => Self::UnknownSession,
            0x0040 => Self::PayloadTooLarge,
            0x0050 => Self::VersionMismatch,
            0x00FF => Self::Unavailable,
            _      => Self::Unknown,
        }
    }
}

impl From<ErrorCode> for u16 {
    fn from(c: ErrorCode) -> Self { c as u16 }
}

// ─── Byte-range type ─────────────────────────────────────────────────────────

/// An inclusive byte range `[start, end]` used in download requests.
/// An empty `ranges` list in [`DownloadRequestFrame`] means *full file*.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ByteRange {
    /// First byte index (inclusive).
    pub start: u64,
    /// Last byte index (inclusive).
    pub end: u64,
}

// ─── Frame enum ──────────────────────────────────────────────────────────────

/// All JTP/1 frames.
///
/// Frames are sent on the **control stream** (stream 0) unless noted otherwise.
/// [`Frame::ChunkData`] and [`Frame::ChunkAck`] travel on dedicated chunk streams.
#[derive(Debug, Clone)]
pub enum Frame {
    // ── Handshake ────────────────────────────────────────────────────────
    /// Client → Server on control stream, immediately after the connection is
    /// established.  Must be the very first frame on stream 0.
    Hello(HelloFrame),
    /// Server → Client; connection accepted, ready to accept sessions.
    HelloAck(HelloAckFrame),

    // ── Upload ───────────────────────────────────────────────────────────
    /// Client → Server; announces intent to upload a file.
    UploadRequest(UploadRequestFrame),
    /// Server → Client; upload approved, lists which chunks are still needed.
    UploadAccept(UploadAcceptFrame),
    /// Server → Client; dedup hit — the server already has this exact file
    /// content.  No chunks need to be sent; jump straight to `CompleteAck`.
    UploadInstant(UploadInstantFrame),

    // ── Download ─────────────────────────────────────────────────────────
    /// Client → Server; requests a file (or specific byte ranges).
    DownloadRequest(DownloadRequestFrame),
    /// Server → Client; confirms the download, provides metadata.
    DownloadInfo(DownloadInfoFrame),

    // ── Progress ─────────────────────────────────────────────────────────
    /// Server → Client on the control stream; sent periodically during both
    /// uploads and downloads so the client can show a progress bar.
    Progress(ProgressFrame),

    // ── Completion ───────────────────────────────────────────────────────
    /// Server → Client (upload); all chunks received and the file VFS record
    /// has been created.
    Complete(CompleteFrame),
    /// Client → Server (upload) or Server → Client (download); acknowledges
    /// receipt of a `Complete` or `DownloadComplete` notification.
    CompleteAck(CompleteAckFrame),
    /// Server → Client (download); all chunks have been pushed.
    DownloadComplete(DownloadCompleteFrame),

    // ── Cancellation ─────────────────────────────────────────────────────
    /// Either side → the other; cancels a specific session.  In-flight chunk
    /// streams for the session should be reset after this frame.
    Cancel(CancelFrame),

    // ── Chunk data (on dedicated chunk streams) ───────────────────────────
    /// Client → Server (upload) **or** Server → Client (download).
    /// Always the first (and typically only) frame on a chunk stream.
    ChunkData(ChunkDataFrame),
    /// Server → Client on the *same* bidi chunk stream (upload only).
    /// Tells the client whether the chunk was accepted or must be retransmitted.
    ChunkAck(ChunkAckFrame),

    // ── Meta ─────────────────────────────────────────────────────────────
    /// Keepalive ping sent by either party.
    Ping(PingFrame),
    /// Keepalive pong; echoes the nonce from the corresponding [`Frame::Ping`].
    Pong(PingFrame),
    /// Remote party encountered an error.  If `session_id` is `Uuid::nil()`,
    /// the error is connection-level and the connection should be closed.
    Error(ErrorFrame),
}

impl Frame {
    /// Returns the [`frame_type`] byte for this frame variant.
    pub fn type_byte(&self) -> u8 {
        use frame_type::*;
        match self {
            Self::Hello(_)            => HELLO,
            Self::HelloAck(_)         => HELLO_ACK,
            Self::UploadRequest(_)    => UPLOAD_REQUEST,
            Self::UploadAccept(_)     => UPLOAD_ACCEPT,
            Self::UploadInstant(_)    => UPLOAD_INSTANT,
            Self::DownloadRequest(_)  => DOWNLOAD_REQUEST,
            Self::DownloadInfo(_)     => DOWNLOAD_INFO,
            Self::Progress(_)         => PROGRESS,
            Self::Complete(_)         => COMPLETE,
            Self::CompleteAck(_)      => COMPLETE_ACK,
            Self::DownloadComplete(_) => DOWNLOAD_COMPLETE,
            Self::Cancel(_)           => CANCEL,
            Self::ChunkData(_)        => CHUNK_DATA,
            Self::ChunkAck(_)         => CHUNK_ACK,
            Self::Ping(_)             => PING,
            Self::Pong(_)             => PONG,
            Self::Error(_)            => ERROR,
        }
    }
}

// ─── Individual frame payloads ────────────────────────────────────────────────

/// JTP/1 protocol version understood by this implementation.
pub const JTP_VERSION: u8 = 1;

/// `HELLO` frame — first frame sent by the client on the control stream.
///
/// Wire layout:
/// ```text
/// [version: u8][token_len: u16][token: utf-8]
/// ```
#[derive(Debug, Clone)]
pub struct HelloFrame {
    /// Protocol version; must equal [`JTP_VERSION`].
    pub version: u8,
    /// JWT access token (UTF-8).  The server validates this before accepting
    /// any other frames.
    pub token: String,
}

/// `HELLO_ACK` frame — server accepts the connection.
///
/// Wire layout:
/// ```text
/// [server_version: u8]
/// ```
#[derive(Debug, Clone)]
pub struct HelloAckFrame {
    /// Server's JTP version.
    pub server_version: u8,
}

/// `UPLOAD_REQUEST` frame — client announces an upload session.
///
/// Wire layout:
/// ```text
/// [session_id: 16][file_name_len: u16][file_name: utf-8]
/// [total_size: u64][content_hash: 32][chunk_count: u32]
/// ```
///
/// `content_hash` is the SHA-256 of the **full file** (pre-chunking).
/// The server may use it for instant-dedup before accepting any chunks.
#[derive(Debug, Clone)]
pub struct UploadRequestFrame {
    /// Client-generated session identifier (UUID v4).
    pub session_id: Uuid,
    /// File name (basename, not a full path).
    pub file_name: String,
    /// Total file size in bytes.
    pub total_size: u64,
    /// SHA-256 of the complete file, as 32 raw bytes.
    pub content_hash: [u8; 32],
    /// Number of CDC chunks the file has been split into.
    pub chunk_count: u32,
}

/// `UPLOAD_ACCEPT` frame — server approves an upload; lists missing chunks.
///
/// Wire layout:
/// ```text
/// [session_id: 16][missing_count: u32][missing_chunk_indices: u32 × N]
/// [parallel_streams: u8][window_size: u8]
/// ```
///
/// An empty `missing_chunks` list means every chunk is already present on the
/// server (dedup at chunk level); the client should send `COMPLETE_ACK`
/// immediately.
///
/// # Parallel stream / sliding-window protocol
///
/// The client opens exactly `parallel_streams` bidirectional QUIC streams and
/// reuses them for the entire upload.  It may have up to `window_size` chunks
/// in-flight (sent but not yet ACKed) across all streams.  Each `CHUNK_ACK`
/// advances the window by one, freeing the slot for the next chunk.  Streams
/// are never closed and reopened mid-session; only the frame payload changes.
#[derive(Debug, Clone)]
pub struct UploadAcceptFrame {
    pub session_id: Uuid,
    /// Sorted list of chunk indices (0-based) the server still needs.
    pub missing_chunks: Vec<u32>,
    /// Number of parallel bidirectional QUIC streams the client should open
    /// for this session.  Typical value: 8.  Range: 1–16.
    pub parallel_streams: u8,
    /// Sliding-window size in chunks: how many chunks may be in-flight
    /// (sent but not yet ACKed) at any moment.  Typical value: 16.
    pub window_size: u8,
}

/// `UPLOAD_INSTANT` frame — full-file dedup hit; no chunks need to be sent.
///
/// Wire layout: `[session_id: 16][file_node_id: 16]`
#[derive(Debug, Clone)]
pub struct UploadInstantFrame {
    pub session_id: Uuid,
    /// The `FileNodeId` of the existing file record.
    pub file_node_id: Uuid,
}

/// `DOWNLOAD_REQUEST` frame.
///
/// Wire layout:
/// ```text
/// [session_id: 16][file_node_id: 16]
/// [range_count: u16]([start: u64][end: u64]) × N
/// ```
///
/// `range_count = 0` means download the full file.
#[derive(Debug, Clone)]
pub struct DownloadRequestFrame {
    pub session_id: Uuid,
    /// The VFS file node to download.
    pub file_node_id: Uuid,
    /// List of inclusive byte ranges to download; empty = entire file.
    pub byte_ranges: Vec<ByteRange>,
}

/// `DOWNLOAD_INFO` frame — server confirms the download and provides metadata.
///
/// Wire layout:
/// ```text
/// [session_id: 16][total_size: u64][chunk_count: u32]
/// [mime_type_len: u16][mime_type: utf-8][parallel_streams: u8]
/// ```
///
/// # Parallel stream protocol
///
/// The server opens exactly `parallel_streams` unidirectional QUIC streams and
/// round-robins chunks across them for the entire download.  Streams are
/// reused rather than opened once per chunk, bounding the total stream count
/// to a small constant regardless of file size.
#[derive(Debug, Clone)]
pub struct DownloadInfoFrame {
    pub session_id: Uuid,
    /// Total size of the (possibly range-filtered) content to be transferred.
    pub total_size: u64,
    /// Total number of chunks that will be pushed.
    pub chunk_count: u32,
    /// MIME type of the file (e.g. `"video/mp4"`).
    pub mime_type: String,
    /// Number of parallel unidirectional QUIC streams the server will open
    /// for this session.  Typical value: 8.  Range: 1–16.
    pub parallel_streams: u8,
}

/// `PROGRESS` frame — server-to-client progress notification.
///
/// Wire layout: `[session_id: 16][bytes_done: u64][chunks_done: u32]`
///
/// Sent on the control stream.  Rate is throttled by the server (≤ once per
/// 500 ms by default) so as not to overwhelm the control stream.
#[derive(Debug, Clone)]
pub struct ProgressFrame {
    pub session_id: Uuid,
    /// Number of bytes transferred so far.
    pub bytes_done: u64,
    /// Number of chunks fully acknowledged so far.
    pub chunks_done: u32,
}

/// `COMPLETE` frame — server confirms a successful upload.
///
/// Wire layout: `[session_id: 16][file_node_id: 16]`
#[derive(Debug, Clone)]
pub struct CompleteFrame {
    pub session_id: Uuid,
    /// Newly created (or existing, for dedup) `FileNodeId`.
    pub file_node_id: Uuid,
}

/// `COMPLETE_ACK` frame — client acknowledges a `COMPLETE` or
/// `DOWNLOAD_COMPLETE` frame.
///
/// Wire layout: `[session_id: 16]`
#[derive(Debug, Clone)]
pub struct CompleteAckFrame {
    pub session_id: Uuid,
}

/// `DOWNLOAD_COMPLETE` frame — server confirms all download chunks sent.
///
/// Wire layout: `[session_id: 16]`
#[derive(Debug, Clone)]
pub struct DownloadCompleteFrame {
    pub session_id: Uuid,
}

/// `CANCEL` frame — cancels a session.
///
/// Wire layout: `[session_id: 16][reason_len: u16][reason: utf-8]`
///
/// `session_id = Uuid::nil()` cancels **all** active sessions on this
/// connection (useful for clean shutdown).
#[derive(Debug, Clone)]
pub struct CancelFrame {
    pub session_id: Uuid,
    /// Human-readable cancellation reason (may be empty).
    pub reason: String,
}

/// `CHUNK_DATA` frame — carries the raw bytes of one CDC chunk.
///
/// Wire layout:
/// ```text
/// [session_id: 16][chunk_index: u32][byte_offset: u64][data: remaining bytes]
/// ```
///
/// The frame occupies the **entire payload** after the header: the data field
/// extends to the end of the payload (i.e. `data.len() = payload_len - 28`).
///
/// Sent by the client (on a client-opened bidi stream) for uploads, or by the
/// server (on a server-opened uni stream) for downloads.
#[derive(Debug, Clone)]
pub struct ChunkDataFrame {
    pub session_id: Uuid,
    /// Zero-based index of this chunk within the session.
    pub chunk_index: u32,
    /// Byte offset of this chunk's first byte within the complete file.
    pub byte_offset: u64,
    /// Raw chunk bytes.
    pub data: Bytes,
}

/// `CHUNK_ACK` frame — server acknowledgment for a single uploaded chunk.
///
/// Wire layout: `[session_id: 16][chunk_index: u32][ok: u8]`
///
/// Sent on the **same bidirectional stream** as the corresponding
/// [`ChunkDataFrame`].  If `ok = 0`, the chunk checksum was invalid and the
/// client must retransmit.
#[derive(Debug, Clone)]
pub struct ChunkAckFrame {
    pub session_id: Uuid,
    pub chunk_index: u32,
    /// `true` = chunk accepted.  `false` = checksum mismatch, retransmit.
    pub ok: bool,
}

/// `PING` / `PONG` frame.
///
/// Wire layout: `[nonce: u64]`
///
/// The pong echoes the nonce verbatim.  Both frames share this struct;
/// the type byte distinguishes them.
#[derive(Debug, Clone)]
pub struct PingFrame {
    /// Random value chosen by the sender; echoed in the corresponding pong.
    pub nonce: u64,
}

/// `ERROR` frame.
///
/// Wire layout:
/// ```text
/// [session_id: 16][code: u16][message_len: u16][message: utf-8]
/// ```
///
/// If `session_id = Uuid::nil()`, the error is connection-level; the receiver
/// should close the connection.  Otherwise only the named session is affected.
#[derive(Debug, Clone)]
pub struct ErrorFrame {
    /// All-zeros = connection-level error.
    pub session_id: Uuid,
    pub code: ErrorCode,
    /// Human-readable description (English, for logging).
    pub message: String,
}
