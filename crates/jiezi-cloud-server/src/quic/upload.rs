//! Upload chunk stream handler.
//!
//! Each in-flight chunk arrives on its own QUIC bidirectional stream.
//! This module reads a single `CHUNK_DATA` frame from the client, buffers the
//! chunk bytes in the in-memory [`SessionManager`], and responds with
//! `CHUNK_ACK { ok }`.
//!
//! When `ok = false` (session unknown, or other transient error) the client
//! must retransmit the chunk on a new stream.
//!
//! Final assembly and persistence (calling `UploadService::store_file`) happen
//! in the connection handler once all chunks have been acknowledged.

use std::sync::Arc;

use tracing::{debug, warn};
use uuid::Uuid;

use jiezi_cloud_core::protocol::{
    Frame,
    frames::{ChunkAckFrame, ErrorCode, ErrorFrame},
};

use super::io::{read_frame, write_frame};
use super::session::SessionManager;

// ─── Chunk stream handler ─────────────────────────────────────────────────────

/// Handle one upload chunk stream: `CHUNK_DATA` ← bidi → `CHUNK_ACK`.
///
/// Returns `(session_id, chunk_index, data_len, all_chunks_received)` on
/// success, or `None` on any error (the stream is closed regardless).
pub async fn handle_upload_chunk(
    mut send: quinn::SendStream,
    mut recv: quinn::RecvStream,
    session_mgr: Arc<SessionManager>,
) -> Option<(Uuid, u32, u64, bool)> {
    // ── Read CHUNK_DATA ───────────────────────────────────────────────────────
    let frame = match read_frame(&mut recv).await {
        Ok(Some(f)) => f,
        Ok(None) => {
            warn!("upload chunk stream closed before sending CHUNK_DATA");
            return None;
        }
        Err(e) => {
            warn!("error reading upload chunk stream: {e}");
            return None;
        }
    };

    let chunk = match frame {
        Frame::ChunkData(c) => c,
        other => {
            warn!("unexpected frame type on upload chunk stream: type=0x{:02X}", other.type_byte());
            let _ = write_frame(
                &mut send,
                &Frame::Error(ErrorFrame {
                    session_id: Uuid::nil(),
                    code: ErrorCode::Internal,
                    message: "expected CHUNK_DATA on chunk stream".into(),
                }),
            )
            .await;
            return None;
        }
    };

    let session_id  = chunk.session_id;
    let chunk_index = chunk.chunk_index;
    let data_len    = chunk.data.len() as u64;

    debug!(
        session_id = %session_id,
        chunk_index,
        bytes = data_len,
        "received CHUNK_DATA"
    );

    // ── Buffer the chunk ──────────────────────────────────────────────────────
    let (ok, all_done) = match session_mgr.store_chunk(session_id, chunk_index, chunk.data) {
        Ok(all_done) => (true, all_done),
        Err(e) => {
            warn!(
                session_id = %session_id,
                chunk_index,
                "store_chunk failed: {:?}", e
            );
            (false, false)
        }
    };

    // ── Send CHUNK_ACK ────────────────────────────────────────────────────────
    let ack = Frame::ChunkAck(ChunkAckFrame { session_id, chunk_index, ok });
    if let Err(e) = write_frame(&mut send, &ack).await {
        warn!("failed to send CHUNK_ACK: {e}");
    }
    let _ = send.finish();

    if ok {
        Some((session_id, chunk_index, data_len, all_done))
    } else {
        None
    }
}

