//! Per-connection handler for the QUIC file-transfer server.
//!
//! # Architecture
//!
//! Each accepted QUIC connection gets its own `do_handle_connection` task.
//! The session business logic ([`perform_handshake`], [`dispatch_control_frame`])
//! is generic over [`JtpTransport`], so it can be unit-tested with an
//! in-memory [`ChannelTransport`] and later reused for the WebSocket fallback
//! without any duplication.
//!
//! # WebSocket fallback
//!
//! Drop-in: implement [`JtpTransport`] for a WebSocket sink/stream, then call:
//! ```text
//! handle_jtp_session(ws_transport, ctl_send, conn, state).await
//! ```
//!
//! # Clone usage rationale
//!
//! All `.clone()` calls below operate on `Arc<T>` — one atomic ref-count
//! increment, no allocation.
//!
//! | Cloned value             | Reason                                              |
//! |--------------------------|-----------------------------------------------------|
//! | `Arc<SessionManager>`    | Shared by control loop + chunk tasks                |
//! | `Arc<Mutex<SendStream>>` | Multiple tasks write COMPLETE/PROGRESS concurrently |
//! | `quinn::Connection`      | Connection is internally an `Arc`                   |
//! | `UploadService`          | Wraps `Arc<DB>`; only what chunk tasks need         |

use std::sync::Arc;

use anyhow::{Result, bail};
use tokio::sync::Mutex;
use tracing::{debug, info, warn};
use uuid::Uuid;

use jiezi_cloud_auth::JwtManager;
use jiezi_cloud_core::{
    models::backend::ReplicationPolicy,
    protocol::{
        Frame, JtpTransport,
        frames::{
            CompleteFrame, DownloadInfoFrame, ErrorCode, ErrorFrame, HelloAckFrame, PingFrame,
            UploadAcceptFrame, JTP_VERSION,
        },
    },
    types::FileId,
};
use jiezi_cloud_storage::UploadService;

use crate::state::AppState;

use super::download::{run_download_session, DEFAULT_MAX_CHUNK_BYTES};
use super::io::{write_frame, QuicControlTransport};
use super::session::SessionManager;
use super::upload::handle_upload_chunk;

// ─── Connection handler ───────────────────────────────────────────────────────

/// Handle one authenticated QUIC connection.
pub async fn handle_connection(conn: quinn::Connection, state: AppState) {
    let peer = conn.remote_address();
    info!(peer = %peer, "QUIC: new connection");

    if let Err(e) = do_handle_connection(conn, state).await {
        info!(peer = %peer, "QUIC: connection ended: {e}");
    }
}

async fn do_handle_connection(conn: quinn::Connection, state: AppState) -> Result<()> {
    let (ctl_send, ctl_recv) = conn
        .accept_bi()
        .await
        .map_err(|e| anyhow::anyhow!("control stream accept failed: {e}"))?;

    let mut transport = QuicControlTransport::new(ctl_send, ctl_recv);
    let ctl_send      = transport.shared_send();

    handle_jtp_session(&mut transport, ctl_send, &conn, &state).await
}

// ─── Transport-generic session handler ───────────────────────────────────────

/// Run a complete JTP/1 session on any [`JtpTransport`].
///
/// Uses [`QuicControlTransport`] in production; can be called with
/// [`ChannelTransport`] in unit tests, or with a WebSocket transport for
/// browser/firewall-restricted clients.
pub async fn handle_jtp_session<T: JtpTransport>(
    transport: &mut T,
    ctl_send: Arc<Mutex<quinn::SendStream>>,
    conn: &quinn::Connection,
    state: &AppState,
) -> Result<()> {
    perform_handshake(transport, &state.jwt).await?;

    // Capture the transport's concurrency capability before moving into the
    // dispatch loop.  QUIC returns 8; WebSocket / test channels return 1.
    let parallel_streams = transport.max_parallel_streams();

    let session_mgr = SessionManager::new();

    {
        let conn2      = conn.clone();
        let mgr2       = session_mgr.clone();
        let upload_svc = state.upload.clone();
        let ctl2       = ctl_send.clone();
        tokio::spawn(async move {
            accept_chunk_streams(conn2, mgr2, upload_svc, ctl2).await;
        });
    }

    loop {
        let frame = match transport.recv_frame().await {
            Ok(Some(f)) => f,
            Ok(None) => {
                debug!("control stream closed by peer");
                break;
            }
            Err(e) => {
                warn!("control stream read error: {e}");
                break;
            }
        };

        if let Err(e) =
            dispatch_control_frame(frame, conn, &ctl_send, &session_mgr, state, parallel_streams).await
        {
            warn!("dispatch error: {e}; closing connection");
            let mut s = ctl_send.lock().await;
            let _ = write_frame(&mut *s, &Frame::Error(ErrorFrame {
                session_id: Uuid::nil(),
                code: ErrorCode::Internal,
                message: e.to_string(),
            })).await;
            break;
        }
    }

    Ok(())
}

// ─── Handshake (transport-generic) ───────────────────────────────────────────

/// `HELLO` → `HELLO_ACK` handshake on any [`JtpTransport`].
///
/// Validates the JWT embedded in the `HELLO` frame.  Returns `Err` if the
/// token is invalid or if the JTP version does not match.
///
/// Being generic over `T: JtpTransport` allows this to be tested from
/// `connection_tests.rs` using a [`ChannelTransport`] — no real network needed.
pub async fn perform_handshake<T: JtpTransport>(
    transport: &mut T,
    jwt: &Arc<JwtManager>,
) -> Result<()> {
    let frame = transport.recv_frame()
        .await?
        .ok_or_else(|| anyhow::anyhow!("peer closed before HELLO"))?;

    let hello = match frame {
        Frame::Hello(h) => h,
        _ => bail!("expected HELLO as first control frame"),
    };

    if hello.version != JTP_VERSION {
        transport.send_frame(&Frame::Error(ErrorFrame {
            session_id: Uuid::nil(),
            code: ErrorCode::VersionMismatch,
            message: format!(
                "server requires JTP/{JTP_VERSION}, client offered JTP/{}",
                hello.version
            ),
        })).await?;
        bail!("JTP version mismatch");
    }

    let claims = jwt
        .verify_access_token(&hello.token)
        .map_err(|e| anyhow::anyhow!("JWT verification failed: {e}"))?;

    transport.send_frame(&Frame::HelloAck(HelloAckFrame {
        server_version: JTP_VERSION,
    })).await?;

    info!(sub = %claims.sub, "QUIC: client authenticated");
    Ok(())
}

// ─── Control frame dispatcher ─────────────────────────────────────────────────

async fn dispatch_control_frame(
    frame: Frame,
    conn: &quinn::Connection,
    ctl_send: &Arc<Mutex<quinn::SendStream>>,
    session_mgr: &Arc<SessionManager>,
    state: &AppState,
    parallel_streams: u8,
) -> Result<()> {
    match frame {
        // ── Upload request ────────────────────────────────────────────────────
        Frame::UploadRequest(req) => {
            let session_id  = req.session_id;
            let chunk_count = req.chunk_count;
            let all_chunks: Vec<u32> = (0..chunk_count).collect();

            // Register the session.
            match session_mgr.insert_upload(
                session_id,
                req.file_name,
                req.total_size,
                req.content_hash,
                chunk_count,
            ) {
                Ok(()) => {}
                Err(ErrorCode::AlreadyExists) => {
                    // Idempotent — client may retry after a connection drop.
                    // Re-send UPLOAD_ACCEPT with all chunks as "missing".
                }
                Err(e) => return Err(anyhow::anyhow!("insert_upload failed: {:?}", e)),
            }

            let accept = Frame::UploadAccept(UploadAcceptFrame {
                session_id,
                missing_chunks: all_chunks,
                parallel_streams,
                window_size: 16,
            });
            let mut s = ctl_send.lock().await;
            write_frame(&mut *s, &accept).await?;
        }

        // ── Download request ──────────────────────────────────────────────────
        Frame::DownloadRequest(req) => {
            let session_id   = req.session_id;
            let file_node_id = req.file_node_id;
            let file_id      = FileId::from(file_node_id);

            // Read the full file content synchronously so we can send
            // accurate DOWNLOAD_INFO metadata before streaming begins.
            let file_bytes = state.download.read_file(&file_id).await
                .map_err(|e| anyhow::anyhow!("read_file({file_node_id}): {e}"))?;

            let total_size = file_bytes.len() as u64;
            let max_chunk  = DEFAULT_MAX_CHUNK_BYTES;
            let chunk_count = ((file_bytes.len() + max_chunk - 1) / max_chunk.max(1)) as u32;

            session_mgr.insert_download(session_id, file_node_id, total_size, chunk_count)
                .map_err(|e| anyhow::anyhow!("insert_download failed: {:?}", e))?;

            // Send DOWNLOAD_INFO.
            let info = Frame::DownloadInfo(DownloadInfoFrame {
                session_id,
                total_size,
                chunk_count,
                mime_type: "application/octet-stream".into(),
                parallel_streams,
            });
            {
                let mut s = ctl_send.lock().await;
                write_frame(&mut *s, &info).await?;
            }

            // Spawn the download session task.
            let conn2      = conn.clone();
            let ctl2       = ctl_send.clone();
            let mgr2       = session_mgr.clone();
            tokio::spawn(async move {
                run_download_session(
                    conn2, ctl2, session_id, file_node_id, file_bytes, max_chunk, mgr2,
                ).await;
            });
        }

        // ── Complete ACK ──────────────────────────────────────────────────────
        Frame::CompleteAck(ack) => {
            debug!(session_id = %ack.session_id, "COMPLETE_ACK received");
        }

        // ── Cancel ────────────────────────────────────────────────────────────
        Frame::Cancel(c) => {
            info!(session_id = %c.session_id, reason = %c.reason, "session cancelled by client");
            session_mgr.mark_cancelled(c.session_id, c.reason);
        }

        // ── Keepalive ─────────────────────────────────────────────────────────
        Frame::Ping(p) => {
            let pong = Frame::Pong(PingFrame { nonce: p.nonce });
            let mut s = ctl_send.lock().await;
            write_frame(&mut *s, &pong).await?;
        }

        unexpected => {
            warn!("unexpected control frame: type 0x{:02X}", unexpected.type_byte());
        }
    }

    Ok(())
}

// ─── Chunk stream acceptor ────────────────────────────────────────────────────

/// Accept incoming client bidi streams (upload chunk streams) in a loop.
///
/// When the last chunk of an upload session is received, this task reassembles
/// the file, calls `UploadService::store_file`, and sends `COMPLETE` on the
/// control stream.
async fn accept_chunk_streams(
    conn: quinn::Connection,
    session_mgr: Arc<SessionManager>,
    upload_svc: UploadService,
    ctl_send: Arc<Mutex<quinn::SendStream>>,
) {
    loop {
        match conn.accept_bi().await {
            Ok((send, recv)) => {
                let mgr  = session_mgr.clone();
                let svc  = upload_svc.clone();
                let ctrl = ctl_send.clone();
                tokio::spawn(async move {
                    if let Some((session_id, _chunk_index, _bytes, all_done)) =
                        handle_upload_chunk(send, recv, mgr.clone()).await
                    {
                        if all_done {
                            finalize_upload(session_id, mgr, svc, ctrl).await;
                        }
                    }
                });
            }
            Err(quinn::ConnectionError::ApplicationClosed { .. }) => {
                debug!("chunk stream acceptor: connection closed");
                break;
            }
            Err(e) => {
                warn!("chunk stream accept error: {e}");
                break;
            }
        }
    }
}

/// Reassemble buffered chunks, persist via `UploadService`, and send `COMPLETE`.
async fn finalize_upload(
    session_id: Uuid,
    session_mgr: Arc<SessionManager>,
    upload_svc: UploadService,
    ctl_send: Arc<Mutex<quinn::SendStream>>,
) {
    let Some((file_name, assembled_bytes, _hash)) = session_mgr.take_assembled(session_id) else {
        warn!(session_id = %session_id, "finalize_upload called but session not complete");
        return;
    };

    let file_id  = FileId::new();
    let policy   = ReplicationPolicy::default();

    match upload_svc.store_file(&file_id, assembled_bytes, &policy).await {
        Ok(_info) => {
            let file_node_id = file_id.into_inner();
            session_mgr.mark_completed(session_id, file_node_id);
            info!(
                session_id = %session_id,
                file_id = %file_node_id,
                file_name = %file_name,
                "upload complete"
            );
            let complete = Frame::Complete(CompleteFrame { session_id, file_node_id });
            let mut s = ctl_send.lock().await;
            let _ = write_frame(&mut *s, &complete).await;
        }
        Err(e) => {
            warn!(session_id = %session_id, "store_file failed: {e}");
            session_mgr.mark_cancelled(session_id, e.to_string());
            let err = Frame::Error(ErrorFrame {
                session_id,
                code: ErrorCode::Internal,
                message: e.to_string(),
            });
            let mut s = ctl_send.lock().await;
            let _ = write_frame(&mut *s, &err).await;
        }
    }
}


// ─── WebSocket session entry point ────────────────────────────────────────────

/// Run a complete JTP/1 session over a WebSocket transport (no QUIC involved).
///
/// Unlike the QUIC path, a WebSocket connection is a single ordered channel so
/// all frames — handshake, upload/download control, and chunk data — travel
/// over the same connection in strict sequence.
///
/// This function is called from `crate::ws::ws_transfer` after the HTTP →
/// WebSocket upgrade has been completed.  It is intentionally separate from
/// [`handle_jtp_session`] to avoid contaminating the generic QUIC path with
/// WebSocket-specific behaviour.
pub async fn run_ws_session(
    transport: &mut crate::ws::WsTransport,
    state: &AppState,
) -> Result<()> {
    // ── Handshake ─────────────────────────────────────────────────────────────
    let hello_frame = transport
        .recv()
        .await?
        .ok_or_else(|| anyhow::anyhow!("peer closed before HELLO"))?;

    let hello = match hello_frame {
        Frame::Hello(h) => h,
        _ => anyhow::bail!("expected HELLO as first frame over WebSocket"),
    };

    if hello.version != JTP_VERSION {
        let _ = transport.send(&Frame::Error(ErrorFrame {
            session_id: Uuid::nil(),
            code: ErrorCode::VersionMismatch,
            message: format!(
                "server requires JTP/{JTP_VERSION}, client offered JTP/{}",
                hello.version
            ),
        })).await;
        anyhow::bail!("JTP version mismatch");
    }

    let claims = state.jwt
        .verify_access_token(&hello.token)
        .map_err(|e| anyhow::anyhow!("JWT verification failed: {e}"))?;
    info!(sub = %claims.sub, "WS: client authenticated");

    transport.send(&Frame::HelloAck(HelloAckFrame {
        server_version: JTP_VERSION,
    })).await?;

    let parallel_streams: u8 = 1;
    let session_mgr = Arc::new(SessionManager::new());

    loop {
        let frame = match transport.recv().await {
            Ok(Some(f)) => f,
            Ok(None) => {
                debug!("WS control stream closed by peer");
                break;
            }
            Err(e) => {
                warn!("WS stream read error: {e}");
                break;
            }
        };

        if let Err(e) =
            dispatch_ws_frame(frame, transport, &session_mgr, state, parallel_streams).await
        {
            warn!("WS dispatch error: {e}; closing session");
            let _ = transport.send(&Frame::Error(ErrorFrame {
                session_id: uuid::Uuid::nil(),
                code: jiezi_cloud_core::protocol::frames::ErrorCode::Internal,
                message: e.to_string(),
            })).await;
            break;
        }
    }

    Ok(())
}

/// Control-frame dispatcher for the WebSocket path.
///
/// Similar to [`dispatch_control_frame`] but handles chunk data inline (no
/// independent QUIC streams) and sends download chunks inline rather than
/// via a spawned task.
async fn dispatch_ws_frame(
    frame: Frame,
    transport: &mut crate::ws::WsTransport,
    session_mgr: &Arc<SessionManager>,
    state: &AppState,
    parallel_streams: u8,
) -> Result<()> {
    match frame {
        // ── Upload request ────────────────────────────────────────────────────
        Frame::UploadRequest(req) => {
            let session_id  = req.session_id;
            let chunk_count = req.chunk_count;
            let all_chunks: Vec<u32> = (0..chunk_count).collect();

            match session_mgr.insert_upload(
                session_id,
                req.file_name,
                req.total_size,
                req.content_hash,
                chunk_count,
            ) {
                Ok(()) | Err(ErrorCode::AlreadyExists) => {}
                Err(e) => return Err(anyhow::anyhow!("insert_upload failed: {:?}", e)),
            }

            transport.send(&Frame::UploadAccept(UploadAcceptFrame {
                session_id,
                missing_chunks: all_chunks,
                parallel_streams,
                window_size: 16,
            })).await?;
        }

        // ── Chunk data (WebSocket: inline, no dedicated stream) ───────────────
        Frame::ChunkData(chunk) => {
            let session_id  = chunk.session_id;
            let chunk_index = chunk.chunk_index;

            // Record the chunk bytes.
            let all_done = match session_mgr.store_chunk(session_id, chunk_index, chunk.data) {
                Ok(done) => done,
                Err(e) => {
                    transport.send(&Frame::ChunkAck(jiezi_cloud_core::protocol::frames::ChunkAckFrame {
                        session_id,
                        chunk_index,
                        ok: false,
                    })).await?;
                    return Err(anyhow::anyhow!("store_chunk failed: {:?}", e));
                }
            };

            transport.send(&Frame::ChunkAck(jiezi_cloud_core::protocol::frames::ChunkAckFrame {
                session_id,
                chunk_index,
                ok: true,
            })).await?;

            // If all chunks received, finalise the upload.
            if all_done {
                if let Some((file_name, assembled, _hash)) = session_mgr.take_assembled(session_id) {
                    let file_id = FileId::new();
                    match state.upload.store_file(&file_id, assembled, &ReplicationPolicy::default()).await {
                        Ok(_) => {
                            let file_node_id = file_id.into_inner();
                            session_mgr.mark_completed(session_id, file_node_id);
                            info!(
                                session_id = %session_id,
                                file_id    = %file_node_id,
                                file_name  = %file_name,
                                "WS: upload complete"
                            );
                            transport.send(&Frame::Complete(CompleteFrame {
                                session_id,
                                file_node_id,
                            })).await?;
                        }
                        Err(e) => {
                            session_mgr.mark_cancelled(session_id, e.to_string());
                            transport.send(&Frame::Error(ErrorFrame {
                                session_id,
                                code: ErrorCode::Internal,
                                message: e.to_string(),
                            })).await?;
                        }
                    }
                }
            }
        }

        // ── Download request ──────────────────────────────────────────────────
        Frame::DownloadRequest(req) => {
            let session_id   = req.session_id;
            let file_node_id = req.file_node_id;
            let file_id      = FileId::from(file_node_id);

            let file_bytes = state.download.read_file(&file_id).await
                .map_err(|e| anyhow::anyhow!("read_file({file_node_id}): {e}"))?;

            let total_size  = file_bytes.len() as u64;
            let max_chunk   = DEFAULT_MAX_CHUNK_BYTES;
            let chunk_count = ((file_bytes.len() + max_chunk - 1) / max_chunk.max(1)) as u32;

            session_mgr.insert_download(session_id, file_node_id, total_size, chunk_count)
                .map_err(|e| anyhow::anyhow!("insert_download failed: {:?}", e))?;

            transport.send(&Frame::DownloadInfo(DownloadInfoFrame {
                session_id,
                total_size,
                chunk_count,
                mime_type: "application/octet-stream".into(),
                parallel_streams,
            })).await?;

            // Send all chunks inline (no QUIC streams available).
            for (index, chunk) in file_bytes.chunks(max_chunk).enumerate() {
                let byte_offset = (index * max_chunk) as u64;
                transport.send(&Frame::ChunkData(jiezi_cloud_core::protocol::frames::ChunkDataFrame {
                    session_id,
                    chunk_index: index as u32,
                    byte_offset,
                    data: bytes::Bytes::copy_from_slice(chunk),
                })).await?;
            }

            transport.send(&Frame::DownloadComplete(
                jiezi_cloud_core::protocol::frames::DownloadCompleteFrame { session_id }
            )).await?;
        }

        // ── Complete ACK ──────────────────────────────────────────────────────
        Frame::CompleteAck(ack) => {
            debug!(session_id = %ack.session_id, "WS: COMPLETE_ACK received");
        }

        // ── Cancel ────────────────────────────────────────────────────────────
        Frame::Cancel(c) => {
            info!(session_id = %c.session_id, reason = %c.reason, "WS: session cancelled");
            session_mgr.mark_cancelled(c.session_id, c.reason);
        }

        // ── Keepalive ─────────────────────────────────────────────────────────
        Frame::Ping(p) => {
            transport.send(&Frame::Pong(PingFrame { nonce: p.nonce })).await?;
        }

        unexpected => {
            warn!("WS: unexpected control frame: type 0x{:02X}", unexpected.type_byte());
        }
    }

    Ok(())
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
#[path = "connection_tests.rs"]
mod tests;