//! Server-Sent Events (SSE) endpoint for real-time server-to-client push.
//!
//! # Why SSE instead of WebSocket
//!
//! All notifications in Jiezi Cloud flow strictly in one direction:
//! the server tells clients what happened (file.uploaded, sync.progress, ...).
//! SSE (text/event-stream) is the idiomatic HTTP primitive for this use-case:
//!
//! - No protocol upgrade -- a plain GET over the existing HTTP/2 connection.
//! - HTTP/2 multiplexes many SSE streams over one TCP connection for free.
//! - Browsers expose the native EventSource API; zero extra JS libs needed.
//! - Flutter reads chunked HTTP bodies with the http package.
//! - Automatic reconnection is spec-defined (retry: field + Last-Event-ID).
//!
//! # Implementation
//!
//! EventBus holds a single tokio::sync::broadcast::Sender<Arc<SseEvent>>.
//! Every subscriber (one per open connection) receives all events; it filters
//! for those addressed to its user_id.  This avoids per-user state while
//! keeping the implementation free of locks in the hot path.
//!
//! Subscribers are BroadcastStream adapters that convert the broadcast receiver
//! into a futures::Stream, which Actix-web's .streaming() responder consumes.
//!
//! # TODO items
//!
//! - TODO(sse-last-event-id): honour Last-Event-ID header on reconnect to
//!   replay missed events from a small ring-buffer.
//! - TODO(sse-admin-push): call EventBus::publish from VFS / share service
//!   after file mutations so clients receive live updates without polling.

use std::sync::Arc;

use actix_web::web::Bytes;
use actix_web::{web, HttpRequest, HttpResponse, ResponseError};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt as _;
use tracing::debug;

use crate::{error::ApiError, state::AppState};

// --- Event types -------------------------------------------------------------

/// A domain event delivered over the SSE stream.
///
/// The `user_id` field is used by subscribers to filter events meant for them.
/// The `kind` field becomes the SSE `event:` line so the client can branch with
/// `eventSource.addEventListener("file.uploaded", handler)`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SseEvent {
    /// Target user (UUID string). Subscribers discard events for other users.
    pub user_id: String,
    /// SSE `event:` type, e.g. "file.uploaded", "sync.progress".
    pub kind: String,
    /// Arbitrary JSON payload for this event type.
    pub payload: serde_json::Value,
    /// Monotonically increasing event ID for Last-Event-ID replay support.
    pub id: u64,
}

impl SseEvent {
    /// Format the event as a valid text/event-stream frame.
    ///
    /// Output:
    /// ```text
    /// id: 42
    /// event: file.uploaded
    /// data: {"name":"report.pdf","size":204800}
    /// retry: 3000
    ///
    /// ```
    fn to_sse_frame(&self) -> String {
        let data = serde_json::to_string(&self.payload).unwrap_or_default();
        format!(
            "id: {}\nevent: {}\ndata: {}\nretry: 3000\n\n",
            self.id, self.kind, data
        )
    }
}

// --- EventBus ----------------------------------------------------------------

/// Shared event bus backed by a tokio::sync::broadcast channel.
///
/// Register as `web::Data<EventBus>` in `App::app_data` so that service-layer
/// code can call [`EventBus::publish`] after file/share mutations.
pub struct EventBus {
    tx: broadcast::Sender<Arc<SseEvent>>,
    next_id: std::sync::atomic::AtomicU64,
}

impl std::fmt::Debug for EventBus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EventBus")
            .field("receiver_count", &self.tx.receiver_count())
            .finish()
    }
}

impl Default for EventBus {
    fn default() -> Self {
        // Capacity 256: if the broadcast buffer fills up, lagged receivers get
        // RecvError::Lagged and drop missed events.  Increase for bursty loads.
        let (tx, _) = broadcast::channel(256);
        Self {
            tx,
            next_id: std::sync::atomic::AtomicU64::new(1),
        }
    }
}

impl EventBus {
    /// Push an event to every open SSE connection for `user_id`.
    ///
    /// No-ops silently when no subscribers are connected.
    pub fn publish(
        &self,
        user_id: impl Into<String>,
        kind: impl Into<String>,
        payload: serde_json::Value,
    ) {
        let id = self
            .next_id
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

        let event = Arc::new(SseEvent {
            user_id: user_id.into(),
            kind: kind.into(),
            payload,
            id,
        });

        // send() only errors when receiver_count == 0; that is fine.
        let _ = self.tx.send(event);
    }

    /// Create a new subscription.  Returns a broadcast::Receiver that the SSE
    /// handler converts into a streaming HTTP response.
    pub fn subscribe(&self) -> broadcast::Receiver<Arc<SseEvent>> {
        self.tx.subscribe()
    }
}

// --- Handler -----------------------------------------------------------------

/// `GET /api/v1/events` -- open an SSE stream for the authenticated user.
///
/// Required header: `Authorization: Bearer <access_token>`
///
/// Response:
/// ```text
/// HTTP/1.1 200 OK
/// Content-Type: text/event-stream
/// Cache-Control: no-cache
/// X-Accel-Buffering: no
/// ```
/// followed by an infinite stream of SSE frames until the client disconnects.
///
/// The `retry: 3000` hint tells clients to wait 3 s before reconnecting.
pub async fn events(
    req: HttpRequest,
    state: web::Data<AppState>,
    bus: web::Data<EventBus>,
) -> HttpResponse {
    // --- Authenticate --------------------------------------------------------
    let token_result: Result<String, ApiError> = (|| {
        let header = req
            .headers()
            .get(actix_web::http::header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| {
                ApiError(jiezi_cloud_core::error::AppError::Unauthorized(
                    "missing Authorization header".into(),
                ))
            })?;
        let token = header.strip_prefix("Bearer ").ok_or_else(|| {
            ApiError(jiezi_cloud_core::error::AppError::Unauthorized(
                "expected Bearer token".into(),
            ))
        })?;
        Ok(token.to_owned())
    })();

    let token = match token_result {
        Ok(t) => t,
        Err(e) => return e.error_response(),
    };

    let claims = match state.auth.verify_token(&token).await {
        Ok(c) => c,
        Err(e) => return ApiError(e).error_response(),
    };

    let user_id = claims.sub.clone();
    debug!(user_id = %user_id, "SSE connection opened");

    // --- Build streaming response --------------------------------------------
    let rx = bus.subscribe();

    // BroadcastStream converts the broadcast receiver into a futures::Stream.
    // Items are Result<Arc<SseEvent>, BroadcastStreamRecvError>.
    let stream = BroadcastStream::new(rx)
        // Drop lagged-receiver errors (some events were missed; that is OK).
        .filter_map(|r| r.ok())
        // Only forward events addressed to this user.
        .filter(move |ev| ev.user_id == user_id)
        // Serialize into a valid SSE frame.
        .map(|ev| -> Result<Bytes, std::convert::Infallible> {
            Ok(Bytes::from(ev.to_sse_frame()))
        });

    HttpResponse::Ok()
        .content_type("text/event-stream")
        .insert_header(("Cache-Control", "no-cache"))
        // Prevents nginx from buffering the stream before forwarding it.
        .insert_header(("X-Accel-Buffering", "no"))
        .streaming(stream)
        .map_into_boxed_body()
}
