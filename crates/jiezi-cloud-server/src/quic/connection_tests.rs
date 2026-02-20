//! Unit tests for the connection handshake and session logic.
//!
//! Uses [`ChannelTransport`] (in-memory mpsc channel) so no real QUIC or
//! network setup is needed.

use std::{sync::Arc, time::Duration};

use jiezi_cloud_auth::JwtManager;
use jiezi_cloud_core::{
    models::user::Role,
    protocol::{
        ChannelTransport, Frame, JtpTransport,
        frames::{ErrorCode, HelloFrame, JTP_VERSION},
    },
    types::UserId,
};

use super::perform_handshake;

// ─── Helpers ──────────────────────────────────────────────────────────────────

/// Build a minimal [`JwtManager`] backed by a freshly-generated ES256 key pair.
fn test_jwt() -> Arc<JwtManager> {
    Arc::new(
        JwtManager::generate(Duration::from_secs(900), Duration::from_secs(86400))
            .expect("JwtManager::generate"),
    )
}

/// Issue a valid access token for a fixed subject.
fn valid_token(jwt: &JwtManager) -> String {
    let user_id = UserId::new();
    let (token, _) = jwt
        .generate_access_token(&user_id, Role::Member)
        .expect("generate_access_token");
    token
}

// ─── Tests ────────────────────────────────────────────────────────────────────

/// Happy path: client sends HELLO with a valid JWT → server replies HELLO_ACK.
#[tokio::test]
async fn handshake_valid_token() {
    let jwt = test_jwt();
    let token = valid_token(&jwt);

    // client side → server side
    let (mut client, mut server) = ChannelTransport::pair(8);

    // Client sends HELLO.
    client
        .send_frame(&Frame::Hello(HelloFrame {
            version: JTP_VERSION,
            token,
        }))
        .await
        .unwrap();

    // Run handshake on the server transport.
    perform_handshake(&mut server, &jwt).await.unwrap();

    // Server must have replied HELLO_ACK.
    let reply = client.recv_frame().await.unwrap().expect("expected HELLO_ACK");
    assert!(
        matches!(&reply, Frame::HelloAck(ack) if ack.server_version == JTP_VERSION),
        "expected HelloAck, got {reply:?}"
    );
}

/// Wrong JTP version → server sends ERROR(VersionMismatch) and returns Err.
#[tokio::test]
async fn handshake_version_mismatch() {
    let jwt = test_jwt();
    let token = valid_token(&jwt);

    let (mut client, mut server) = ChannelTransport::pair(8);

    client
        .send_frame(&Frame::Hello(HelloFrame {
            version: 0xFF, // unknown version
            token,
        }))
        .await
        .unwrap();

    let result = perform_handshake(&mut server, &jwt).await;
    assert!(result.is_err(), "handshake should fail on version mismatch");

    // Client should have received an ERROR frame.
    let err_frame = client.recv_frame().await.unwrap().expect("expected ERROR frame");
    assert!(
        matches!(&err_frame, Frame::Error(e) if e.code == ErrorCode::VersionMismatch),
        "expected VersionMismatch error, got {err_frame:?}"
    );
}

/// Invalid JWT → handshake returns Err (no response frame is mandated).
#[tokio::test]
async fn handshake_invalid_token() {
    let jwt = test_jwt();

    let (mut client, mut server) = ChannelTransport::pair(8);

    client
        .send_frame(&Frame::Hello(HelloFrame {
            version: JTP_VERSION,
            token: "not.a.valid.jwt".into(),
        }))
        .await
        .unwrap();

    let result = perform_handshake(&mut server, &jwt).await;
    assert!(result.is_err(), "handshake should fail on invalid JWT");
}

/// Peer closes stream before sending HELLO → Err("peer closed before HELLO").
#[tokio::test]
async fn handshake_peer_closed() {
    let jwt = test_jwt();
    let (client, mut server) = ChannelTransport::pair(8);

    // Drop client without sending anything → server recv_frame() returns None.
    drop(client);

    let result = perform_handshake(&mut server, &jwt).await;
    assert!(result.is_err());
    let msg = result.unwrap_err().to_string();
    assert!(msg.contains("peer closed"), "unexpected message: {msg}");
}
