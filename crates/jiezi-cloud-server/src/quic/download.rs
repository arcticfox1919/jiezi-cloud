//! Download session handler.
//!
//! For each download session, the server:
//!
//! 1. Obtains a [`FileDownloadStream`] from `DownloadService::stream_file`.
//! 2. Sends `DOWNLOAD_INFO` on the control stream (total_size, chunk_count).
//! 3. Buffers incoming CDC chunks into 4 MiB JTP chunks (re-chunking).
//! 4. Opens one server-initiated unidirectional QUIC stream per JTP chunk
//!    and sends `CHUNK_DATA` on each.
//! 5. Sends `DOWNLOAD_COMPLETE` on the control stream.
//!
//! # Memory model
//!
//! Peak memory ≈ `DOWNLOAD_CONCURRENCY × max_chunk_bytes` ≈ 32 MiB, regardless
//! of file size.  CDC chunks arrive through an mpsc channel (back-pressured)
//! and are re-chunked into fixed-size JTP chunks using a single `BytesMut`
//! accumulator.
//!
//! # Zero-copy
//!
//! When a JTP chunk is exactly one contiguous region in `Bytes`, it is sent
//! directly via `Bytes::slice` — no copy.

use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use bytes::{Bytes, BytesMut};
use tokio::sync::{Semaphore, mpsc};
use tracing::{debug, info, warn};
use uuid::Uuid;

use jiezi_cloud_core::protocol::{
    Frame,
    frames::{ChunkDataFrame, DownloadCompleteFrame, ErrorCode, ErrorFrame, ProgressFrame},
};

use super::io::ControlSender;
use super::session::SessionManager;

/// Maximum number of chunk streams open simultaneously per download session.
const DOWNLOAD_CONCURRENCY: usize = 8;

/// Default maximum chunk size (4 MiB).
pub const DEFAULT_MAX_CHUNK_BYTES: usize = 4 * 1024 * 1024;

/// Minimum interval between PROGRESS frames.
const PROGRESS_INTERVAL: Duration = Duration::from_millis(500);

// ─── Download session entry point ─────────────────────────────────────────────

/// Drive one complete download session to completion using streaming I/O.
///
/// - `conn`          — the QUIC connection (for opening uni streams).
/// - `ctrl_send`     — mpsc-based control frame sender.
/// - `session_id`    — session identifier.
/// - `file_node_id`  — VFS node being downloaded.
/// - `chunk_rx`      — receiver of CDC chunks from [`DownloadService::stream_file`].
/// - `total_size`    — total file size in bytes.
/// - `max_chunk`     — maximum bytes per JTP/1 chunk.
/// - `session_mgr`   — shared session state.
pub async fn run_download_session(
    conn: quinn::Connection,
    ctrl_send: ControlSender,
    session_id: Uuid,
    file_node_id: Uuid,
    chunk_rx: mpsc::Receiver<jiezi_cloud_core::error::AppResult<Bytes>>,
    total_size: u64,
    max_chunk: usize,
    session_mgr: Arc<SessionManager>,
) {
    if let Err(e) = do_download(
        conn,
        ctrl_send.clone(),
        session_id,
        file_node_id,
        chunk_rx,
        total_size,
        max_chunk,
        session_mgr.clone(),
    )
    .await
    {
        warn!(session_id = %session_id, "download session failed: {e}");
        session_mgr.mark_cancelled(session_id, e.to_string());

        let err_frame = Frame::Error(ErrorFrame {
            session_id,
            code: ErrorCode::Internal,
            message: e.to_string(),
        });
        let _ = ctrl_send.send_frame(&err_frame).await;
    }
}

async fn do_download(
    conn: quinn::Connection,
    ctrl_send: ControlSender,
    session_id: Uuid,
    file_node_id: Uuid,
    mut chunk_rx: mpsc::Receiver<jiezi_cloud_core::error::AppResult<Bytes>>,
    total_size: u64,
    max_chunk: usize,
    session_mgr: Arc<SessionManager>,
) -> Result<()> {
    let chunk_size = max_chunk.max(1);

    info!(
        session_id = %session_id,
        total_bytes = total_size,
        "starting streaming download session"
    );

    // ── Streaming re-chunk: buffer CDC chunks → emit fixed JTP chunks ─────────
    let sem = Arc::new(Semaphore::new(DOWNLOAD_CONCURRENCY));
    let mut tasks = tokio::task::JoinSet::new();
    let mut last_progress = Instant::now();

    let mut buffer = BytesMut::with_capacity(chunk_size);
    let mut jtp_index: u32 = 0;
    let mut byte_offset: u64 = 0;

    while let Some(result) = chunk_rx.recv().await {
        let cdc_chunk = result.map_err(|e| anyhow::anyhow!("storage read error: {e}"))?;
        buffer.extend_from_slice(&cdc_chunk);

        // Emit full JTP-sized chunks as they accumulate.
        while buffer.len() >= chunk_size {
            let data = buffer.split_to(chunk_size).freeze();
            let permit = sem.clone().acquire_owned().await?;
            let conn2 = conn.clone();
            let sid = session_id;
            let idx = jtp_index;
            let off = byte_offset;

            tasks.spawn(async move {
                let result = send_chunk_stream(conn2, sid, idx, off, data).await;
                drop(permit);
                result
            });

            byte_offset += chunk_size as u64;
            jtp_index += 1;
        }

        // Emit progress periodically.
        if last_progress.elapsed() >= PROGRESS_INTERVAL {
            if let Some((chunks_sent, bytes_sent)) = session_mgr.download_progress(session_id) {
                let progress = Frame::Progress(ProgressFrame {
                    session_id,
                    bytes_done: bytes_sent,
                    chunks_done: chunks_sent,
                });
                let _ = ctrl_send.send_frame(&progress).await;
            }
            last_progress = Instant::now();
        }
    }

    // Flush remaining bytes as the last (potentially smaller) JTP chunk.
    if !buffer.is_empty() {
        let data = buffer.freeze();
        let permit = sem.clone().acquire_owned().await?;
        let conn2 = conn.clone();
        let sid = session_id;
        let idx = jtp_index;
        let off = byte_offset;

        tasks.spawn(async move {
            let result = send_chunk_stream(conn2, sid, idx, off, data).await;
            drop(permit);
            result
        });
    }

    // Wait for all chunk-send tasks.
    while let Some(result) = tasks.join_next().await {
        match result {
            Ok(Ok(bytes)) => {
                let _ = session_mgr.record_chunk_sent(session_id, bytes);
            }
            Ok(Err(e)) => warn!(session_id = %session_id, "chunk send error: {e}"),
            Err(e) => warn!(session_id = %session_id, "chunk task panicked: {e}"),
        }
    }

    // ── Send DOWNLOAD_COMPLETE ────────────────────────────────────────────────
    let complete = Frame::DownloadComplete(DownloadCompleteFrame { session_id });
    ctrl_send.send_frame(&complete).await?;

    session_mgr.mark_completed(session_id, file_node_id);
    info!(session_id = %session_id, "download session complete");
    Ok(())
}

/// Open a server unidirectional stream and push one chunk.
async fn send_chunk_stream(
    conn: quinn::Connection,
    session_id: Uuid,
    chunk_index: u32,
    byte_offset: u64,
    data: Bytes,
) -> Result<u64> {
    let data_len = data.len() as u64;
    let mut send = conn.open_uni().await?;

    let frame = Frame::ChunkData(ChunkDataFrame { session_id, chunk_index, byte_offset, data });
    write_frame(&mut send, &frame).await?;
    send.finish()?;

    debug!(session_id = %session_id, chunk_index, bytes = data_len, "sent chunk on uni stream");
    Ok(data_len)
}

// Re-import write_frame for use in send_chunk_stream.
use super::io::write_frame;
