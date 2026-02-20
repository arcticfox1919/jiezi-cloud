//! JTP/1 binary codec — encoding and decoding of [`Frame`] values.
//!
//! # Design notes
//!
//! The codec is **zero-copy** for the data portion of `ChunkData` frames:
//! the raw bytes are wrapped in a [`Bytes`] slice that shares the original
//! buffer without copying.
//!
//! Encoding writes into a [`BytesMut`] (growing it in-place) rather than
//! allocating a new [`Vec`] per frame.  Callers can then split off the
//! written bytes and hand them to the QUIC send API.
//!
//! # Decode contract
//!
//! [`FrameCodec::decode`] applies a *peek-then-consume* pattern:
//!
//! 1. If `buf` holds fewer than 5 bytes, return `Ok(None)` — wait for more
//!    data.
//! 2. Read the 5-byte header without advancing `buf`.
//! 3. If `buf` holds fewer than `5 + payload_len` bytes, return `Ok(None)`.
//! 4. Advance `buf` past `5 + payload_len` bytes and parse the payload.
//!
//! This means the caller can simply call `decode` in a loop until it returns
//! `None`, then refill the buffer.
//!
//! # Encode contract
//!
//! [`FrameCodec::encode`] **appends** to `buf`; the caller is responsible for
//! calling `buf.split_to(n)` or `buf.split()` to extract the bytes.

use bytes::{Buf, BufMut, Bytes, BytesMut};
use uuid::Uuid;

use super::frames::{
    frame_type, ByteRange, CancelFrame, ChunkAckFrame, ChunkDataFrame, CompleteAckFrame,
    CompleteFrame, DownloadCompleteFrame, DownloadInfoFrame, DownloadRequestFrame, ErrorCode,
    ErrorFrame, Frame, HelloAckFrame, HelloFrame, PingFrame, ProgressFrame, UploadAcceptFrame,
    UploadInstantFrame, UploadRequestFrame,
};

// ─── Error type ──────────────────────────────────────────────────────────────

/// Errors that can occur while decoding a JTP/1 frame.
#[derive(Debug, thiserror::Error)]
pub enum CodecError {
    #[error("unknown frame type: 0x{0:02X}")]
    UnknownFrameType(u8),
    #[error("payload too short: expected {expected} bytes, got {actual}")]
    PayloadTooShort { expected: usize, actual: usize },
    #[error("string field is not valid UTF-8: {0}")]
    InvalidUtf8(#[from] std::string::FromUtf8Error),
    #[error("payload length {0} exceeds maximum allowed size")]
    PayloadTooLarge(u32),
}

/// Maximum allowed payload size (64 MiB).  Frames larger than this are
/// rejected during decode to guard against memory exhaustion attacks.
pub const MAX_PAYLOAD_BYTES: u32 = 64 * 1024 * 1024;

// ─── Helpers ─────────────────────────────────────────────────────────────────

/// Return a `PayloadTooShort` error if the condition is false (macro helper).
macro_rules! ensure {
    ($cond:expr, $field:expr, $expected:expr, $actual:expr) => {
        if !$cond {
            return Err(CodecError::PayloadTooShort {
                expected: $expected,
                actual: $actual,
            });
        }
    };
}

/// Read a `[len: u16 BE][utf-8]` string from `buf`.
///
/// Advances `buf` by `2 + string_len` bytes.
/// Returns an error if the string bytes are not valid UTF-8 **or** if `buf`
/// is shorter than the declared length.
fn read_string(buf: &mut impl Buf) -> Result<String, CodecError> {
    ensure!(buf.remaining() >= 2, "string length prefix", 2, buf.remaining());
    let len = buf.get_u16() as usize;
    ensure!(buf.remaining() >= len, "string body", len, buf.remaining());
    let mut bytes = vec![0u8; len];
    buf.copy_to_slice(&mut bytes);
    Ok(String::from_utf8(bytes)?)
}

/// Write a `[len: u16 BE][utf-8]` string into `buf`.
fn write_string(buf: &mut BytesMut, s: &str) {
    let b = s.as_bytes();
    buf.put_u16(b.len() as u16);
    buf.put_slice(b);
}

/// Read a 16-byte UUID from `buf`.
fn read_uuid(buf: &mut impl Buf) -> Uuid {
    let mut b = [0u8; 16];
    buf.copy_to_slice(&mut b);
    Uuid::from_bytes(b)
}

/// Write a 16-byte UUID into `buf`.
fn write_uuid(buf: &mut BytesMut, id: Uuid) {
    buf.put_slice(id.as_bytes());
}

/// Read a 32-byte raw hash from `buf`.
fn read_hash(buf: &mut impl Buf) -> [u8; 32] {
    let mut h = [0u8; 32];
    buf.copy_to_slice(&mut h);
    h
}

// ─── FrameCodec ──────────────────────────────────────────────────────────────

/// Stateless JTP/1 frame codec.
pub struct FrameCodec;

impl FrameCodec {
    // ── Decode ───────────────────────────────────────────────────────────────

    /// Attempt to decode one frame from `buf`.
    ///
    /// - Returns `Ok(Some(frame))` if a complete frame was available.
    /// - Returns `Ok(None)` if more bytes are needed.
    /// - Returns `Err(CodecError)` if the frame is malformed.
    ///
    /// On success `buf` is advanced past the consumed bytes.  On `Ok(None)`
    /// `buf` is left unchanged so the caller can keep buffering.
    pub fn decode(buf: &mut BytesMut) -> Result<Option<Frame>, CodecError> {
        // Need at least the 5-byte header.
        if buf.len() < 5 {
            return Ok(None);
        }

        let frame_type = buf[0];
        let payload_len = u32::from_be_bytes([buf[1], buf[2], buf[3], buf[4]]);

        if payload_len > MAX_PAYLOAD_BYTES {
            return Err(CodecError::PayloadTooLarge(payload_len));
        }

        let total_len = 5 + payload_len as usize;
        if buf.len() < total_len {
            return Ok(None); // partial — wait for more data
        }

        // Consume the frame from buf.
        buf.advance(5);
        let payload_bytes = buf.split_to(payload_len as usize);
        let mut payload = &payload_bytes[..];

        let frame = Self::decode_payload(frame_type, &mut payload, payload_bytes.clone())?;
        Ok(Some(frame))
    }

    /// Parse payload bytes into a typed [`Frame`].
    ///
    /// `raw_payload` is a clone of the same byte slice that `payload` points
    /// into, used for zero-copy construction of `ChunkData.data`.
    fn decode_payload(
        frame_type: u8,
        payload: &mut &[u8],
        raw_payload: BytesMut,
    ) -> Result<Frame, CodecError> {
        use frame_type::*;

        match frame_type {
            // ── Handshake ─────────────────────────────────────────────────
            HELLO => {
                ensure!(payload.len() >= 1, "version", 1, payload.len());
                let version = payload.get_u8();
                let token = read_string(payload)?;
                Ok(Frame::Hello(HelloFrame { version, token }))
            }

            HELLO_ACK => {
                ensure!(payload.len() >= 1, "server_version", 1, payload.len());
                let server_version = payload.get_u8();
                Ok(Frame::HelloAck(HelloAckFrame { server_version }))
            }

            // ── Upload ────────────────────────────────────────────────────
            UPLOAD_REQUEST => {
                ensure!(payload.len() >= 16, "session_id", 16, payload.len());
                let session_id = read_uuid(payload);
                let file_name = read_string(payload)?;
                ensure!(payload.len() >= 8 + 32 + 4, "upload_request_tail", 44, payload.len());
                let total_size = payload.get_u64();
                let content_hash = read_hash(payload);
                let chunk_count = payload.get_u32();
                Ok(Frame::UploadRequest(UploadRequestFrame {
                    session_id,
                    file_name,
                    total_size,
                    content_hash,
                    chunk_count,
                }))
            }

            UPLOAD_ACCEPT => {
                // header: session_id(16) + missing_count(4) + parallel_streams(1) + window_size(1)
                ensure!(payload.len() >= 16 + 4 + 1 + 1, "upload_accept_head", 22, payload.len());
                let session_id = read_uuid(payload);
                let missing_count = payload.get_u32() as usize;
                ensure!(
                    payload.len() >= missing_count * 4 + 2,
                    "missing_chunks",
                    missing_count * 4 + 2,
                    payload.len()
                );
                let missing_chunks = (0..missing_count).map(|_| payload.get_u32()).collect();
                let parallel_streams = payload.get_u8();
                let window_size = payload.get_u8();
                Ok(Frame::UploadAccept(UploadAcceptFrame {
                    session_id,
                    missing_chunks,
                    parallel_streams,
                    window_size,
                }))
            }

            UPLOAD_INSTANT => {
                ensure!(payload.len() >= 32, "upload_instant", 32, payload.len());
                let session_id = read_uuid(payload);
                let file_node_id = read_uuid(payload);
                Ok(Frame::UploadInstant(UploadInstantFrame {
                    session_id,
                    file_node_id,
                }))
            }

            // ── Download ──────────────────────────────────────────────────
            DOWNLOAD_REQUEST => {
                ensure!(payload.len() >= 32 + 2, "download_request_head", 34, payload.len());
                let session_id = read_uuid(payload);
                let file_node_id = read_uuid(payload);
                let range_count = payload.get_u16() as usize;
                ensure!(
                    payload.len() >= range_count * 16,
                    "byte_ranges",
                    range_count * 16,
                    payload.len()
                );
                let byte_ranges = (0..range_count)
                    .map(|_| ByteRange {
                        start: payload.get_u64(),
                        end: payload.get_u64(),
                    })
                    .collect();
                Ok(Frame::DownloadRequest(DownloadRequestFrame {
                    session_id,
                    file_node_id,
                    byte_ranges,
                }))
            }

            DOWNLOAD_INFO => {
                ensure!(payload.len() >= 16 + 8 + 4, "download_info_head", 28, payload.len());
                let session_id = read_uuid(payload);
                let total_size = payload.get_u64();
                let chunk_count = payload.get_u32();
                let mime_type = read_string(payload)?;
                ensure!(payload.len() >= 1, "download_info_parallel", 1, payload.len());
                let parallel_streams = payload.get_u8();
                Ok(Frame::DownloadInfo(DownloadInfoFrame {
                    session_id,
                    total_size,
                    chunk_count,
                    mime_type,
                    parallel_streams,
                }))
            }

            // ── Progress ──────────────────────────────────────────────────
            PROGRESS => {
                ensure!(payload.len() >= 16 + 8 + 4, "progress", 28, payload.len());
                let session_id = read_uuid(payload);
                let bytes_done = payload.get_u64();
                let chunks_done = payload.get_u32();
                Ok(Frame::Progress(ProgressFrame {
                    session_id,
                    bytes_done,
                    chunks_done,
                }))
            }

            // ── Completion ────────────────────────────────────────────────
            COMPLETE => {
                ensure!(payload.len() >= 32, "complete", 32, payload.len());
                let session_id = read_uuid(payload);
                let file_node_id = read_uuid(payload);
                Ok(Frame::Complete(CompleteFrame {
                    session_id,
                    file_node_id,
                }))
            }

            COMPLETE_ACK => {
                ensure!(payload.len() >= 16, "complete_ack", 16, payload.len());
                let session_id = read_uuid(payload);
                Ok(Frame::CompleteAck(CompleteAckFrame { session_id }))
            }

            DOWNLOAD_COMPLETE => {
                ensure!(payload.len() >= 16, "download_complete", 16, payload.len());
                let session_id = read_uuid(payload);
                Ok(Frame::DownloadComplete(DownloadCompleteFrame { session_id }))
            }

            // ── Cancellation ──────────────────────────────────────────────
            CANCEL => {
                ensure!(payload.len() >= 16, "cancel_session_id", 16, payload.len());
                let session_id = read_uuid(payload);
                let reason = read_string(payload)?;
                Ok(Frame::Cancel(CancelFrame { session_id, reason }))
            }

            // ── Chunk data ────────────────────────────────────────────────
            CHUNK_DATA => {
                // Fixed header: 16 (session_id) + 4 (chunk_index) + 8 (byte_offset) = 28 bytes
                const FIXED: usize = 28;
                ensure!(payload.len() >= FIXED, "chunk_data_header", FIXED, payload.len());
                let session_id = read_uuid(payload);
                let chunk_index = payload.get_u32();
                let byte_offset = payload.get_u64();
                // Zero-copy: take the remaining bytes from raw_payload.
                // raw_payload starts at the beginning of the original payload,
                // so the data portion starts at offset FIXED.
                let data = Bytes::from(raw_payload).slice(FIXED..);
                Ok(Frame::ChunkData(ChunkDataFrame {
                    session_id,
                    chunk_index,
                    byte_offset,
                    data,
                }))
            }

            CHUNK_ACK => {
                ensure!(payload.len() >= 16 + 4 + 1, "chunk_ack", 21, payload.len());
                let session_id = read_uuid(payload);
                let chunk_index = payload.get_u32();
                let ok = payload.get_u8() != 0;
                Ok(Frame::ChunkAck(ChunkAckFrame {
                    session_id,
                    chunk_index,
                    ok,
                }))
            }

            // ── Meta ──────────────────────────────────────────────────────
            frame_type::PING => {
                ensure!(payload.len() >= 8, "ping_nonce", 8, payload.len());
                let nonce = payload.get_u64();
                Ok(Frame::Ping(PingFrame { nonce }))
            }

            frame_type::PONG => {
                ensure!(payload.len() >= 8, "pong_nonce", 8, payload.len());
                let nonce = payload.get_u64();
                Ok(Frame::Pong(PingFrame { nonce }))
            }

            ERROR => {
                ensure!(payload.len() >= 16 + 2, "error_head", 18, payload.len());
                let session_id = read_uuid(payload);
                let code = ErrorCode::from(payload.get_u16());
                let message = read_string(payload)?;
                Ok(Frame::Error(ErrorFrame {
                    session_id,
                    code,
                    message,
                }))
            }

            unknown => Err(CodecError::UnknownFrameType(unknown)),
        }
    }

    // ── Encode ───────────────────────────────────────────────────────────────

    /// Encode `frame` and **append** the result to `buf`.
    ///
    /// The caller can then extract the written bytes with
    /// `buf.split()` / `buf.split_to(n)`.
    pub fn encode(frame: &Frame, buf: &mut BytesMut) {
        // Reserve a placeholder for the length field; we'll fill it in later.
        let header_start = buf.len();
        buf.put_u8(frame.type_byte());
        buf.put_u32(0); // placeholder

        let payload_start = buf.len();
        Self::encode_payload(frame, buf);
        let payload_len = (buf.len() - payload_start) as u32;

        // Back-fill the length field.
        let len_bytes = payload_len.to_be_bytes();
        buf[header_start + 1..header_start + 5].copy_from_slice(&len_bytes);
    }

    fn encode_payload(frame: &Frame, buf: &mut BytesMut) {
        match frame {
            Frame::Hello(f) => {
                buf.put_u8(f.version);
                write_string(buf, &f.token);
            }
            Frame::HelloAck(f) => {
                buf.put_u8(f.server_version);
            }
            Frame::UploadRequest(f) => {
                write_uuid(buf, f.session_id);
                write_string(buf, &f.file_name);
                buf.put_u64(f.total_size);
                buf.put_slice(&f.content_hash);
                buf.put_u32(f.chunk_count);
            }
            Frame::UploadAccept(f) => {
                write_uuid(buf, f.session_id);
                buf.put_u32(f.missing_chunks.len() as u32);
                for &idx in &f.missing_chunks {
                    buf.put_u32(idx);
                }
                buf.put_u8(f.parallel_streams);
                buf.put_u8(f.window_size);
            }
            Frame::UploadInstant(f) => {
                write_uuid(buf, f.session_id);
                write_uuid(buf, f.file_node_id);
            }
            Frame::DownloadRequest(f) => {
                write_uuid(buf, f.session_id);
                write_uuid(buf, f.file_node_id);
                buf.put_u16(f.byte_ranges.len() as u16);
                for r in &f.byte_ranges {
                    buf.put_u64(r.start);
                    buf.put_u64(r.end);
                }
            }
            Frame::DownloadInfo(f) => {
                write_uuid(buf, f.session_id);
                buf.put_u64(f.total_size);
                buf.put_u32(f.chunk_count);
                write_string(buf, &f.mime_type);
                buf.put_u8(f.parallel_streams);
            }
            Frame::Progress(f) => {
                write_uuid(buf, f.session_id);
                buf.put_u64(f.bytes_done);
                buf.put_u32(f.chunks_done);
            }
            Frame::Complete(f) => {
                write_uuid(buf, f.session_id);
                write_uuid(buf, f.file_node_id);
            }
            Frame::CompleteAck(f) => {
                write_uuid(buf, f.session_id);
            }
            Frame::DownloadComplete(f) => {
                write_uuid(buf, f.session_id);
            }
            Frame::Cancel(f) => {
                write_uuid(buf, f.session_id);
                write_string(buf, &f.reason);
            }
            Frame::ChunkData(f) => {
                write_uuid(buf, f.session_id);
                buf.put_u32(f.chunk_index);
                buf.put_u64(f.byte_offset);
                buf.put_slice(&f.data);
            }
            Frame::ChunkAck(f) => {
                write_uuid(buf, f.session_id);
                buf.put_u32(f.chunk_index);
                buf.put_u8(if f.ok { 1 } else { 0 });
            }
            Frame::Ping(f) | Frame::Pong(f) => {
                buf.put_u64(f.nonce);
            }
            Frame::Error(f) => {
                write_uuid(buf, f.session_id);
                buf.put_u16(u16::from(f.code));
                write_string(buf, &f.message);
            }
        }
    }
}
