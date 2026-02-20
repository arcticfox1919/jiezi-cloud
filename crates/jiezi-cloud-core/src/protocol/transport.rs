//! JTP/1 transport abstraction.
//!
//! Defines [`JtpTransport`], a thin async trait over any byte channel that can
//! carry JTP/1 frames.  Concrete implementations are provided for:
//!
//! | Transport          | Struct                  | Location              |
//! |--------------------|-------------------------|-----------------------|
//! | QUIC (production)  | `QuicControlTransport`  | `jiezi-cloud-server`  |
//! | WebSocket (fallback)| `WsTransport`          | `jiezi-cloud-server`  |
//! | In-memory (testing) | `ChannelTransport`     | `jiezi-cloud-core`    |
//!
//! # Why is this trait in `jiezi-cloud-core`?
//!
//! The JTP/1 session-level logic (handshake, frame dispatch, session
//! management) is pure business logic and should be tested without any network
//! stack.  Placing the trait in the core crate lets tests in every crate
//! instantiate a `ChannelTransport` without pulling in quinn or tokio-tungstenite.
//!
//! # Fallback behaviour
//!
//! When a QUIC connection cannot be established (e.g. UDP blocked by a
//! firewall, or a WebBrowser client), the server falls back to JTP/1 over
//! WebSocket.  Because every JTP/1 frame already carries a `session_id` and
//! `chunk_index`, the frames are self-routing on a single WebSocket connection
//! (no per-chunk substreams are needed).
//!
//! ```text
//! Client                    Server
//!   │── WS upgrade ─────────▶│  POST /api/v1/transfer/ws
//!   │◀── 101 Switching ───────│
//!   │── HELLO (JTP/1) ───────▶│  (binary WS frame = JTP/1 frame)
//!   │◀── HELLO_ACK ───────────│
//!   │── UPLOAD_REQUEST ───────▶│
//!   │◀── UPLOAD_ACCEPT ────────│
//!   │── CHUNK_DATA ───────────▶│  (all chunks in-order on same WS connection)
//!   │◀── CHUNK_ACK ────────────│
//!   │◀── COMPLETE ─────────────│
//!   │── COMPLETE_ACK ─────────▶│
//! ```

use std::future::Future;

use anyhow::Result;

use super::frames::Frame;

// ─── Core trait ───────────────────────────────────────────────────────────────

/// An async, bidirectional transport for JTP/1 [`Frame`]s.
///
/// Implementors wrap a concrete I/O channel (e.g. a QUIC control stream, a
/// WebSocket connection, or an in-memory channel pair).  All session-level
/// logic (`perform_handshake`, `dispatch_control_frame`, …) is generic over
/// `T: JtpTransport`, enabling unit tests to run without a real network.
///
/// # Object safety
///
/// This trait uses `impl Trait` return types (RPITIT), which are not
/// object-safe.  Use `impl JtpTransport` in generics rather than
/// `dyn JtpTransport`.  For storing boxed transports use the helper
/// [`BoxedTransport`].
pub trait JtpTransport: Send + 'static {
    /// Receive the next frame from the transport.
    ///
    /// Returns `Ok(None)` when the remote peer cleanly closes the channel.
    fn recv_frame(&mut self) -> impl Future<Output = Result<Option<Frame>>> + Send + '_;

    /// Send a single frame on the transport.
    fn send_frame<'a>(&'a mut self, frame: &'a Frame) -> impl Future<Output = Result<()>> + Send + 'a;

    /// Maximum number of parallel chunk streams this transport can usefully
    /// exploit.
    ///
    /// - QUIC: returns 8 (independent streams saturate high-BDP links).
    /// - WebSocket / in-memory: returns 1 (single ordered byte channel;
    ///   opening multiple "streams" provides no benefit).
    ///
    /// The server embeds this value in `UPLOAD_ACCEPT.parallel_streams` and
    /// `DOWNLOAD_INFO.parallel_streams` so the client opens the right number
    /// of concurrent chunk streams.
    fn max_parallel_streams(&self) -> u8 {
        1
    }
}

// ─── In-memory transport (testing only) ──────────────────────────────────────

/// A pair of `tokio::sync::mpsc` channels used as an in-memory JTP/1
/// transport.  Useful for unit-testing the session-logic layer without any
/// real network stack.
///
/// # Example
///
/// ```rust,no_run
/// use jiezi_cloud_core::protocol::transport::ChannelTransport;
/// use jiezi_cloud_core::protocol::frames::{Frame, HelloFrame, JTP_VERSION};
/// use jiezi_cloud_core::protocol::JtpTransport;
///
/// # async fn run() {
/// let (mut client, mut server) = ChannelTransport::pair(32);
///
/// // The client sends a HELLO frame.
/// client.send_frame(&Frame::Hello(HelloFrame {
///     version: JTP_VERSION,
///     token: "test-token".into(),
/// })).await.unwrap();
///
/// // The server receives it.
/// let frame = server.recv_frame().await.unwrap().unwrap();
/// assert!(matches!(frame, Frame::Hello(_)));
/// # }
/// ```
pub struct ChannelTransport {
    tx: tokio::sync::mpsc::Sender<Frame>,
    rx: tokio::sync::mpsc::Receiver<Frame>,
}

impl ChannelTransport {
    /// Create a connected pair: `(a, b)` where frames sent by `a` arrive at
    /// `b` and vice versa.
    pub fn pair(buffer: usize) -> (Self, Self) {
        let (tx_a, rx_b) = tokio::sync::mpsc::channel(buffer);
        let (tx_b, rx_a) = tokio::sync::mpsc::channel(buffer);
        (
            Self { tx: tx_a, rx: rx_a },
            Self { tx: tx_b, rx: rx_b },
        )
    }
}

impl JtpTransport for ChannelTransport {
    async fn recv_frame(&mut self) -> Result<Option<Frame>> {
        Ok(self.rx.recv().await)
    }

    async fn send_frame(&mut self, frame: &Frame) -> Result<()> {
        self.tx.send(frame.clone()).await
            .map_err(|_| anyhow::anyhow!("ChannelTransport: receiver dropped"))
    }
}
