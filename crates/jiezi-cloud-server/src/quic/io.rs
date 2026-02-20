//! Low-level QUIC stream I/O helpers for JTP/1 frames.
//!
//! Provides two things:
//!
//! 1. [`read_frame`] / [`write_frame`] — standalone async helpers used by chunk
//!    streams (each chunk lives on its own bidi/uni stream).
//!
//! 2. [`QuicControlTransport`] — implements [`JtpTransport`] for the QUIC
//!    control bidi stream (stream 0).  The session-logic layer is generic over
//!    `T: JtpTransport`; swapping to WebSocket only requires a different impl.
//!
//! # Protocol reminder
//!
//! Every JTP/1 frame is:
//! ```text
//! [type: u8][payload_len: u32 BE][payload: bytes]
//! ```
//! The total minimum read to determine the full frame length is **5 bytes**
//! (the header).

use std::sync::Arc;

use anyhow::{Context, Result, bail};
use bytes::BytesMut;
use quinn::{RecvStream, SendStream};
use tokio::sync::Mutex;

use jiezi_cloud_core::protocol::{Frame, FrameCodec, JtpTransport};

// ─── Standalone frame helpers (used by chunk streams) ─────────────────────────

/// Read exactly one JTP/1 frame from a `quinn` receive stream.
///
/// Returns `Ok(None)` if the stream was cleanly closed (zero bytes readable).
pub async fn read_frame(recv: &mut RecvStream) -> Result<Option<Frame>> {
    // ── Read the 5-byte header ────────────────────────────────────────────────
    let mut header = [0u8; 5];
    match recv.read_exact(&mut header).await {
        Ok(()) => {}
        Err(quinn::ReadExactError::FinishedEarly(_)) => return Ok(None),
        Err(e) => return Err(e).context("reading frame header"),
    }

    let payload_len = u32::from_be_bytes([header[1], header[2], header[3], header[4]]) as usize;

    if payload_len > jiezi_cloud_core::protocol::MAX_PAYLOAD_BYTES as usize {
        bail!("frame payload ({payload_len} bytes) exceeds MAX_PAYLOAD_BYTES");
    }

    // ── Read the payload ──────────────────────────────────────────────────────
    let mut payload = BytesMut::zeroed(payload_len);
    recv.read_exact(&mut payload)
        .await
        .context("reading frame payload")?;

    // ── Decode (re-combine header + payload so codec sees the full 5+N bytes) ─
    let mut combined = BytesMut::with_capacity(5 + payload_len);
    combined.extend_from_slice(&header);
    combined.extend_from_slice(&payload);

    FrameCodec::decode(&mut combined)
        .map_err(|e| anyhow::anyhow!("frame decode error: {e}"))?
        .ok_or_else(|| anyhow::anyhow!("decoder returned None after full read"))
        .map(Some)
}

/// Encode and write one JTP/1 frame to a `quinn` send stream.
pub async fn write_frame(send: &mut SendStream, frame: &Frame) -> Result<()> {
    let mut buf = BytesMut::new();
    FrameCodec::encode(frame, &mut buf);
    send.write_all(&buf).await.context("writing frame")?;
    Ok(())
}

// ─── QuicControlTransport ─────────────────────────────────────────────────────

/// [`JtpTransport`] implementation wrapping the QUIC control bidi stream.
///
/// `send` is an `Arc<Mutex<SendStream>>` so the same send half can be shared
/// with chunk-completion tasks that also need to write `COMPLETE` / `ERROR`
/// frames on the control stream.
pub struct QuicControlTransport {
    recv: RecvStream,
    /// Shared write half — also held by chunk-stream finaliser tasks.
    pub send: Arc<Mutex<SendStream>>,
}

impl QuicControlTransport {
    /// Wrap a quinn bidi stream accepted from the peer.
    pub fn new(send: SendStream, recv: RecvStream) -> Self {
        Self { recv, send: Arc::new(Mutex::new(send)) }
    }

    /// Return a clone of the shared send handle so it can be given to tasks
    /// that need to write frames outside the main control loop.
    pub fn shared_send(&self) -> Arc<Mutex<SendStream>> {
        Arc::clone(&self.send)
    }
}

impl JtpTransport for QuicControlTransport {
    async fn recv_frame(&mut self) -> Result<Option<Frame>> {
        read_frame(&mut self.recv).await
    }

    async fn send_frame(&mut self, frame: &Frame) -> Result<()> {
        let mut s = self.send.lock().await;
        write_frame(&mut *s, frame).await
    }

    /// QUIC supports independent streams; 8 parallel chunk streams provides
    /// good throughput on high-BDP links without excessive overhead.
    fn max_parallel_streams(&self) -> u8 {
        8
    }
}
