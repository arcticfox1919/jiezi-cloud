//! Download session handler.
//!
//! For each download session, the server:
//!
//! 1. Reads all file bytes via [`DownloadService::read_file`].
//! 2. Slices the bytes into fixed-size chunks (≤ `max_chunk_bytes` from config,
//!    defaults to 4 MiB).
//! 3. Sends `DOWNLOAD_INFO` on the control stream (total_size, chunk_count).
//! 4. Opens one server-initiated unidirectional QUIC stream per chunk and
//!    sends `CHUNK_DATA` on each.
//! 5. Sends `DOWNLOAD_COMPLETE` on the control stream.
//!
//! # Parallel chunk streaming
//!
//! Chunks are opened with bounded concurrency (default 8 parallel streams) so
//! the server does not open hundreds of streams at once for large files.

use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use bytes::Bytes;
use tokio::sync::{Mutex, Semaphore};
use tracing::{debug, info, warn};
use uuid::Uuid;

use jiezi_cloud_core::protocol::{
    Frame,
    frames::{ChunkDataFrame, DownloadCompleteFrame, ErrorCode, ErrorFrame, ProgressFrame},
};

use super::io::write_frame;
use super::session::SessionManager;

/// Maximum number of chunk streams open simultaneously per download session.
const DOWNLOAD_CONCURRENCY: usize = 8;

/// Default maximum chunk size (4 MiB).
pub const DEFAULT_MAX_CHUNK_BYTES: usize = 4 * 1024 * 1024;

/// Minimum interval between PROGRESS frames.
const PROGRESS_INTERVAL: Duration = Duration::from_millis(500);

// ─── Download session entry point ─────────────────────────────────────────────

/// Drive one complete download session to completion.
///
/// - `conn`          — the QUIC connection (for opening uni streams).
/// - `ctrl_send`     — locked send half of the control stream.
/// - `session_id`    — session identifier.
/// - `file_node_id`  — VFS node being downloaded.
/// - `file_bytes`    — **entire file content**, already loaded by the caller.
/// - `max_chunk`     — maximum bytes per JTP/1 chunk.
/// - `session_mgr`   — shared session state.
pub async fn run_download_session(
    conn: quinn::Connection,
    ctrl_send: Arc<Mutex<quinn::SendStream>>,
    session_id: Uuid,
    file_node_id: Uuid,
    file_bytes: Bytes,
    max_chunk: usize,
    session_mgr: Arc<SessionManager>,
) {
    if let Err(e) = do_download(
        conn,
        ctrl_send.clone(),
        session_id,
        file_node_id,
        file_bytes,
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
        let mut ctrl = ctrl_send.lock().await;
        let _ = write_frame(&mut *ctrl, &err_frame).await;
    }
}

async fn do_download(
    conn: quinn::Connection,
    ctrl_send: Arc<Mutex<quinn::SendStream>>,
    session_id: Uuid,
    file_node_id: Uuid,
    file_bytes: Bytes,
    max_chunk: usize,
    session_mgr: Arc<SessionManager>,
) -> Result<()> {
    // ── Slice into chunks ─────────────────────────────────────────────────────
    let total_size = file_bytes.len() as u64;
    let chunk_size = max_chunk.max(1);
    let chunks: Vec<(u32, u64, Bytes)> = file_bytes
        .chunks(chunk_size)
        .enumerate()
        .map(|(i, slice)| {
            let offset = (i * chunk_size) as u64;
            // `slice` is a `&[u8]` from chunks(); copy into Bytes.
            (i as u32, offset, Bytes::copy_from_slice(slice))
        })
        .collect();

    let chunk_count = chunks.len() as u32;

    info!(
        session_id = %session_id,
        total_bytes = total_size,
        chunk_count,
        "starting download session"
    );

    // ── Stream chunks with bounded concurrency ────────────────────────────────
    let sem = Arc::new(Semaphore::new(DOWNLOAD_CONCURRENCY));
    let mut tasks = tokio::task::JoinSet::new();
    let mut last_progress = Instant::now();

    for (chunk_index, byte_offset, data) in chunks {
        let permit = sem.clone().acquire_owned().await?;
        let conn2 = conn.clone();
        let sid = session_id;

        tasks.spawn(async move {
            let result = send_chunk_stream(conn2, sid, chunk_index, byte_offset, data).await;
            drop(permit);
            result
        });

        // Emit progress update on the control stream.
        if last_progress.elapsed() >= PROGRESS_INTERVAL {
            if let Some((chunks_sent, bytes_sent)) = session_mgr.download_progress(session_id) {
                let progress = Frame::Progress(ProgressFrame {
                    session_id,
                    bytes_done: bytes_sent,
                    chunks_done: chunks_sent,
                });
                let mut ctrl = ctrl_send.lock().await;
                let _ = write_frame(&mut *ctrl, &progress).await;
            }
            last_progress = Instant::now();
        }
    }

    // Wait for all chunk tasks.
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
    {
        let mut ctrl = ctrl_send.lock().await;
        write_frame(&mut *ctrl, &complete).await?;
    }

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
