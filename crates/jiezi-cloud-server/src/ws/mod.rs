//! WebSocket fallback transport for JTP/1.
//!
//! Exposes:
//! - [`WsTransport`] — implements [`JtpTransport`] over an `actix-ws`
//!   WebSocket connection (single ordered channel, `max_parallel_streams = 1`).
//! - [`ws_transfer`] — Actix-web handler for `POST /api/v1/transfer/ws` that
//!   performs the HTTP → WebSocket upgrade and runs a full JTP/1 session.
//!
//! # Why WebSocket?
//!
//! QUIC requires UDP, which is blocked by many corporate firewalls and is
//! unavailable in browsers.  The WebSocket fallback lets any environment that
//! can reach the HTTP port transfer files with full JTP/1 semantics.
//!
//! # Limitations compared to QUIC
//!
//! | Property              | QUIC              | WebSocket         |
//! |-----------------------|-------------------|-------------------|
//! | Head-of-line blocking | None (per-stream) | Whole connection  |
//! | Parallel chunk streams| 8                 | 1                 |
//! | Overhead              | Low               | Higher (framing)  |
//!
//! Because WebSocket is a single ordered byte channel, chunks are serialised
//! one at a time (`parallel_streams = 1`).  The JTP/1 session layer handles
//! this transparently via the `max_parallel_streams` hint.

pub mod transport;

pub use transport::WsTransport;

use actix_web::{HttpRequest, HttpResponse, web};
use tracing::info;

use crate::middleware::auth::AuthUser;
use crate::quic::connection::run_ws_session;
use crate::state::AppState;

/// `POST /api/v1/transfer/ws`
///
/// Upgrades an HTTP connection to WebSocket and runs a full JTP/1 session.
///
/// The client must authenticate with a valid `Authorization: Bearer <token>`
/// header (same as the REST API) before sending a `HELLO` frame.  The JWT is
/// re-validated inside the session runner via the `HELLO` frame token field,
/// providing a second authentication factor that is transport-agnostic.
pub async fn ws_transfer(
    req:   HttpRequest,
    body:  web::Payload,
    state: web::Data<AppState>,
    _auth: AuthUser,
) -> Result<HttpResponse, actix_web::Error> {
    let (response, session, msg_stream) = actix_ws::handle(&req, body)?;

    let state_inner = state.get_ref().clone();

    // Run the JTP/1 session in a separate task so the handler can return the
    // 101 response immediately.
    actix_rt::spawn(async move {
        let mut transport = WsTransport::new(session, msg_stream);

        if let Err(e) = run_ws_session(&mut transport, &state_inner).await {
            info!("WS: session ended: {e}");
        }
    });

    Ok(response)
}
