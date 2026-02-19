//! Jiezi Cloud server binary entry point.
//!
//! Starts the Actix-web HTTP/REST API server, the WebSocket notification hub,
//! and (optionally) the QUIC file transfer service.  All application modules
//! are wired together here via dependency injection.

use tracing::info;
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    // Initialize tracing / structured logging.
    tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .with(fmt::layer())
        .init();

    info!("Jiezi Cloud server starting…");

    // TODO: load config, wire modules, start HTTP server.
    // This will be implemented in Phase 5.

    Ok(())
}
