//! Jiezi Cloud server binary entry point.
//!
//! Starts the Actix-web HTTP/REST API server, the WebSocket notification hub,
//! and (optionally) the QUIC file transfer service.  All application modules
//! are wired together here via dependency injection.

use tracing::info;
use tracing_subscriber::{EnvFilter, fmt, prelude::*};

use jiezi_cloud_config::{AppConfig, TracingFormat};

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    // Load layered configuration: default.toml -> {env}.toml -> local.toml -> env vars.
    let cfg: AppConfig = jiezi_cloud_config::load().unwrap_or_else(|e| {
        eprintln!("FATAL: failed to load configuration: {e}");
        std::process::exit(1);
    });

    // Validate invariants (e.g. weak JWT secret in production).
    cfg.validate().unwrap_or_else(|e| {
        eprintln!("FATAL: invalid configuration: {e}");
        std::process::exit(1);
    });

    // Initialize tracing / structured logging using config values.
    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(&cfg.tracing.level));

    let registry = tracing_subscriber::registry().with(env_filter);

    match cfg.tracing.format {
        TracingFormat::Json => {
            registry.with(fmt::layer().json()).init();
        }
        TracingFormat::Compact => {
            registry.with(fmt::layer().compact()).init();
        }
        TracingFormat::Pretty => {
            registry.with(fmt::layer().pretty()).init();
        }
    }

    info!(
        environment = ?cfg.environment,
        bind = %cfg.server.bind_address(),
        "Jiezi Cloud server starting"
    );

    // TODO: wire database, run migrations, wire auth service, start HTTP server.
    // This will be implemented in Phase 5.

    Ok(())
}
