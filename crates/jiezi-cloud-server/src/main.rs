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

    // ── Phase 5 TODO list (implement in order) ─────────────────────────────
    //
    // TODO(Phase 5 — step 1): Open SeaORM connection pool.
    //   let db = sea_orm::Database::connect(&cfg.database.url).await...;
    //
    // TODO(Phase 5 — step 2): Apply SQLite durability pragmas IMMEDIATELY after
    //   opening the pool (no-op for PostgreSQL/MySQL — SeaORM skips them):
    //     PRAGMA journal_mode = WAL;     -- crash-safe write-ahead log
    //     PRAGMA synchronous  = FULL;    -- fsync before every commit returns
    //     PRAGMA foreign_keys = ON;      -- enforce FK constraints at DB level
    //     PRAGMA busy_timeout = 5000;    -- wait 5 s if DB file is locked
    //   Use db.execute(Statement::from_string(DbBackend::Sqlite, ...)) for each.
    //   Gate the calls behind `if db.get_database_backend() == DbBackend::Sqlite`.
    //
    // TODO(Phase 5 — step 3): Run pending schema migrations.
    //   use jiezi_cloud_migration::Migrator;
    //   use sea_orm_migration::MigratorTrait;
    //   Migrator::up(&db, None).await...;
    //
    // TODO(Phase 5 — step 4): Run startup integrity check BEFORE binding the
    //   HTTP port (serve no traffic until the DB is confirmed healthy).
    //   let check = jiezi_cloud_integrity::startup::run(&db).await;
    //   for w in &check.warnings { warn!("{w}"); }
    //   if !check.is_ok() { for e in &check.errors { error!("{e}"); } exit(1); }
    //
    // TODO(Phase 5 — step 5): Start the automatic backup background task.
    //   if cfg.database.backup.enabled {
    //       jiezi_cloud_integrity::backup::start_background_task(
    //           db.clone(), cfg.database.clone());
    //   }
    //
    // TODO(Phase 5 — step 6): Start the chunk integrity patrol background task.
    //   jiezi_cloud_integrity::patrol::start_background_task(
    //       cfg.storage.local_root.clone(), db.clone());
    //
    // TODO(Phase 5 — step 7): Wire auth service, storage backend, VFS service.
    //
    // TODO(Phase 5 — step 8): Build and start Actix-web HttpServer.
    //   actix_web::HttpServer::new(|| App::new().configure(routes::configure))
    //       .workers(effective_workers)
    //       .bind(cfg.server.bind_address())?
    //       .run()
    //       .await

    Ok(())
}
