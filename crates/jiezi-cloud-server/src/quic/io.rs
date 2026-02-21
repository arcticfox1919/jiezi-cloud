//! Low-level QUIC stream I/O helpers for JTP/1 frames.
//!
//! Provides three things:
//!
//! 1. [`read_frame`] / [`write_frame`] — standalone async helpers used by chunk
//!    streams (each chunk lives on its own bidi/uni stream).
//!
//! 2. [`ControlSender`] — a cheap-to-`Clone` handle that serialises multiple
//!    producers' frames through a single mpsc channel to a dedicated writer
//!    task.  Eliminates the `Arc<Mutex<SendStream>>` contention pattern.
//!
//! 3. [`QuicControlTransport`] — implements [`JtpTransport`] for the QUIC
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

use anyhow::{Context, Result, bail};
use bytes::{Bytes, BytesMut};
use quinn::{RecvStream, SendStream};
use tokio::sync::mpsc;
use tracing::warn;

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

// ─── ControlSender ────────────────────────────────────────────────────────────

/// Channel capacity for control frame writes.
///
/// 64 buffered frames is generous; with default settings this never fills up
/// unless the peer stalls acknowledgments.
const CONTROL_CHANNEL_CAP: usize = 64;

/// A cheap-to-`Clone` handle for sending frames on the control stream.
///
/// Internally an mpsc channel whose receiver is drained by a dedicated writer
/// task.  Multiple tasks can call [`send_frame`](ControlSender::send_frame)
/// concurrently without lock contention.
#[derive(Clone)]
pub struct ControlSender {
    tx: mpsc::Sender<Bytes>,
}

impl ControlSender {
    /// Encode `frame` and enqueue it for the writer task.
    pub async fn send_frame(&self, frame: &Frame) -> Result<()> {
        let mut buf = BytesMut::new();
        FrameCodec::encode(frame, &mut buf);
        self.tx
            .send(buf.freeze())
            .await
            .map_err(|_| anyhow::anyhow!("control stream writer closed"))?;
        Ok(())
    }
}

/// Spawn a dedicated writer task that drains the mpsc channel and writes
/// serialised frames to the QUIC `SendStream`.
///
/// Returns a [`ControlSender`] that any number of tasks can share.
pub fn spawn_control_writer(mut send: SendStream) -> ControlSender {
    let (tx, mut rx) = mpsc::channel::<Bytes>(CONTROL_CHANNEL_CAP);
    tokio::spawn(async move {
        while let Some(data) = rx.recv().await {
            if let Err(e) = send.write_all(&data).await {
                warn!("control stream write error: {e}");
                break;
            }
        }
        // Channel closed — finish the QUIC stream gracefully.
        let _ = send.finish();
    });
    ControlSender { tx }
}

// ─── QuicControlTransport ─────────────────────────────────────────────────────

/// [`JtpTransport`] implementation wrapping the QUIC control bidi stream.
///
/// Reads come directly from the [`RecvStream`]; writes go through a
/// [`ControlSender`] (mpsc channel → dedicated writer task).
pub struct QuicControlTransport {
    recv: RecvStream,
    /// Shared write handle — also held by chunk-stream finaliser tasks and
    /// download session tasks.
    sender: ControlSender,
}

impl QuicControlTransport {
    /// Wrap a quinn bidi stream accepted from the peer.
    ///
    /// Spawns a dedicated writer task that owns the `SendStream`; all writes
    /// go through the returned [`ControlSender`].
    pub fn new(send: SendStream, recv: RecvStream) -> Self {
        let sender = spawn_control_writer(send);
        Self { recv, sender }
    }

    /// Return a clone of the shared [`ControlSender`] so it can be given to
    /// tasks that need to write frames outside the main control loop.
    pub fn shared_sender(&self) -> ControlSender {
        self.sender.clone()
    }
}

impl JtpTransport for QuicControlTransport {
    async fn recv_frame(&mut self) -> Result<Option<Frame>> {
        read_frame(&mut self.recv).await
    }

    async fn send_frame(&mut self, frame: &Frame) -> Result<()> {
        self.sender.send_frame(frame).await
    }

    /// QUIC supports independent streams; 8 parallel chunk streams provides
    /// good throughput on high-BDP links without excessive overhead.
    fn max_parallel_streams(&self) -> u8 {
        8
    }
}
