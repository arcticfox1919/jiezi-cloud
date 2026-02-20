//! [`WsTransport`] — JTP/1 transport backed by an `actix-ws` WebSocket
//! connection.
//!
//! Wraps [`actix_ws::Session`] (write half) and [`actix_ws::MessageStream`]
//! (read half).  Each binary WebSocket message carries exactly one JTP/1 frame.
//!
//! # Send-safety note
//!
//! `actix_ws::MessageStream` wraps Actix's HTTP payload which uses
//! `Rc<RefCell<>>` internally and is therefore **`!Send`**.  This means
//! `WsTransport` cannot implement the `JtpTransport` trait (which requires
//! `Send + 'static`), and it must be used exclusively on the actix worker
//! thread via `actix_rt::spawn` (not `tokio::spawn`).
//!
//! The WebSocket session logic in [`crate::ws::session`] calls
//! [`WsTransport::recv`] and [`WsTransport::send`] directly without going
//! through the generic `JtpTransport` abstraction.

use anyhow::{Result, anyhow};
use bytes::BytesMut;
use tokio_stream::StreamExt as _;

use jiezi_cloud_core::protocol::{Frame, FrameCodec};

/// Transport adapter that wraps an `actix-ws` WebSocket connection for JTP/1.
///
/// # Frame mapping
///
/// Each JTP/1 frame is encapsulated as a single **binary** WebSocket message.
/// The encoding is identical to the QUIC wire format (type byte + 4-byte
/// payload length + payload), so [`FrameCodec`] can be reused directly.
///
/// Ping / pong / text WebSocket messages are silently discarded — they do not
/// carry JTP/1 data.  A `Close` message causes [`recv`] to return `Ok(None)`.
///
/// # Not `Send`
///
/// `actix_ws::MessageStream` is `!Send`.  Use `actix_rt::spawn` (not
/// `tokio::spawn`) to run tasks that hold a `WsTransport`.
pub struct WsTransport {
    /// Write half: used to send data to the remote peer.
    pub session: actix_ws::Session,
    /// Read half: a `Stream` of incoming WebSocket messages.
    pub stream: actix_ws::MessageStream,
}

impl WsTransport {
    /// Create a new `WsTransport` from the two halves produced by
    /// [`actix_ws::handle`].
    pub fn new(session: actix_ws::Session, stream: actix_ws::MessageStream) -> Self {
        Self { session, stream }
    }

    /// Receive the next JTP/1 frame from the WebSocket connection.
    ///
    /// Returns `Ok(None)` on clean close.
    pub async fn recv(&mut self) -> Result<Option<Frame>> {
        loop {
            match self.stream.next().await {
                None => return Ok(None),

                Some(Err(e)) => return Err(anyhow!("WebSocket receive error: {e}")),

                Some(Ok(msg)) => match msg {
                    // Binary messages carry one JTP/1 frame.
                    actix_ws::Message::Binary(bytes) => {
                        let mut buf = BytesMut::from(bytes.as_ref());
                        let frame = FrameCodec::decode(&mut buf)
                            .map_err(|e| anyhow!("JTP/1 decode error: {e}"))?
                            .ok_or_else(|| anyhow!("incomplete JTP/1 frame in WebSocket message"))?;
                        return Ok(Some(frame));
                    }

                    // Peer wants to close — treat as clean end-of-stream.
                    actix_ws::Message::Close(_) => return Ok(None),

                    // Ping / pong / text are not part of JTP/1; skip them.
                    _ => continue,
                },
            }
        }
    }

    /// Encode `frame` and send it as a binary WebSocket message.
    pub async fn send(&mut self, frame: &Frame) -> Result<()> {
        let mut buf = BytesMut::new();
        FrameCodec::encode(frame, &mut buf);
        self.session
            .binary(buf.freeze())
            .await
            .map_err(|_| anyhow!("WsTransport: WebSocket session closed"))
    }
}
