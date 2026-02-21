//! QUIC file-transfer server (JTP/1).
//!
//! This module provides [`QuicServer`], which binds a UDP port, terminates TLS
//! 1.3, and handles the JieziTransfer Protocol v1 (JTP/1) for high-performance
//! file uploads and downloads.
//!
//! # Entry point
//!
//! ```rust,ignore
//! use jiezi_cloud_server::quic::QuicServer;
//!
//! // Inside your async main / server startup:
//! let quic = QuicServer::new(&cfg, state.clone()).await?;
//! tokio::spawn(async move { quic.run().await });
//! ```
//!
//! # Feature flag
//!
//! The QUIC server is only started when `cfg.quic.enabled = true`.  The
//! default in `config/default.toml` is `false` so that the server can be run
//! without configuring a UDP port during onboarding / HTTP-only deployments.

use std::net::SocketAddr;

use anyhow::{Context, Result};
use tracing::{error, info};

use jiezi_cloud_config::QuicConfig;

use crate::state::AppState;

pub mod connection;
pub mod download;
pub mod io;
pub mod session;
pub mod tls;
pub mod upload;

// ─── QuicServer ───────────────────────────────────────────────────────────────

/// The JTP/1 / QUIC transport server.
pub struct QuicServer {
    endpoint: quinn::Endpoint,
    state: AppState,
}

impl QuicServer {
    /// Create and bind the QUIC endpoint.
    ///
    /// Does **not** start accepting connections; call [`QuicServer::run`] for that.
    ///
    /// # Errors
    ///
    /// Returns an error if TLS setup fails, or if the UDP socket cannot be bound.
    pub async fn new(cfg: &QuicConfig, state: AppState) -> Result<Self> {
        let server_tls = tls::build_server_tls(cfg)
            .context("QUIC: TLS configuration failed")?;

        let addr: SocketAddr = format!("{}:{}", cfg.listen_addr, cfg.port)
            .parse()
            .with_context(|| {
                format!(
                    "QUIC: invalid listen address '{}:{}'",
                    cfg.listen_addr, cfg.port
                )
            })?;

        let endpoint = quinn::Endpoint::server(server_tls, addr)
            .with_context(|| format!("QUIC: failed to bind UDP socket on {addr}"))?;

        info!(addr = %addr, "QUIC: endpoint bound — JTP/1 ready");

        Ok(Self { endpoint, state })
    }

    /// Accept connections in a loop until the endpoint is closed.
    ///
    /// This method runs indefinitely; wrap it in `tokio::spawn` and keep the
    /// returned handle to cancel it on shutdown.
    pub async fn run(self) {
        info!("QUIC: accepting connections");

        while let Some(incoming) = self.endpoint.accept().await {
            let state = self.state.clone();
            tokio::spawn(async move {
                match incoming.await {
                    Ok(conn) => {
                        connection::handle_connection(conn, state).await;
                    }
                    Err(e) => {
                        error!("QUIC: connection setup failed: {e}");
                    }
                }
            });
        }

        info!("QUIC: endpoint closed");
    }

    /// Gracefully stop the endpoint.
    ///
    /// In-flight connections receive a `CONNECTION_CLOSE` with application
    /// error code 0.  New connections are rejected.
    pub fn close(&self) {
        self.endpoint.close(0u32.into(), b"server shutdown");
    }

    /// Returns the local address the endpoint is bound to.
    pub fn local_addr(&self) -> Result<SocketAddr> {
        self.endpoint.local_addr().context("QUIC: local_addr")
    }
}
