//! Jiezi Cloud server binary entry point.
//!
//! Starts the Actix-web HTTP/REST API server.  WebSocket and QUIC file
//! transfer are planned for Phases 6–7.
//!
//! # Startup sequence
//!
//! 1. Load layered configuration.
//! 2. Open SeaORM connection pool.
//! 3. Apply SQLite durability pragmas (WAL / synchronous=FULL / …).
//! 4. Run pending schema migrations.
//! 5. Fast startup integrity check — abort if the DB is corrupt.
//! 6. Spawn background tasks (auto-backup).
//! 7. Wire service objects (Auth, VFS).
//! 8. Bind and run Actix-web `HttpServer`.

use actix_cors::Cors;
use actix_governor::{Governor, GovernorConfigBuilder};
use actix_web::http::header;
use actix_web::middleware::from_fn;
use actix_web::{web, App, HttpServer, Responder};
use sea_orm::{ConnectOptions, ConnectionTrait, Database, DbBackend, Statement};
use std::sync::Arc;
use std::time::Duration;
use tracing::{error, info, warn};
use tracing_actix_web::TracingLogger;
use tracing_subscriber::{fmt, prelude::*, EnvFilter};
use utoipa::OpenApi;
use utoipa_scalar::{Scalar, Servable as _};

use jiezi_cloud_config::{AppConfig, TracingFormat};
use jiezi_cloud_migration::Migrator;
use sea_orm_migration::MigratorTrait;

use jiezi_cloud_server::{
    build_app_state,
    middleware::security_headers::security_headers,
    middleware::setup_guard::setup_guard,
    repository::settings::SystemSettingsRepository,
    routes,
    routes::sse::EventBus,
};

// ─── Entry point ──────────────────────────────────────────────────────────────

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    // ── Step 1: Load configuration ────────────────────────────────────────────
    let cfg: AppConfig = jiezi_cloud_config::load().unwrap_or_else(|e| {
        eprintln!("FATAL: failed to load configuration: {e}");
        std::process::exit(1);
    });

    cfg.validate().unwrap_or_else(|e| {
        eprintln!("FATAL: invalid configuration: {e}");
        std::process::exit(1);
    });

    // ── Tracing / structured logging ──────────────────────────────────────────
    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(&cfg.tracing.level));

    let registry = tracing_subscriber::registry().with(env_filter);
    match cfg.tracing.format {
        TracingFormat::Json => registry.with(fmt::layer().json()).init(),
        TracingFormat::Compact => registry.with(fmt::layer().compact()).init(),
        TracingFormat::Pretty => registry.with(fmt::layer().pretty()).init(),
    }

    info!(
        environment = ?cfg.environment,
        bind = %cfg.server.bind_address(),
        "Jiezi Cloud starting"
    );

    // ── Step 2: Open database connection pool ─────────────────────────────────
    let mut conn_opts = ConnectOptions::new(&cfg.database.url);
    conn_opts
        .max_connections(cfg.database.max_connections)
        .min_connections(cfg.database.min_connections)
        .connect_timeout(Duration::from_secs(
            cfg.database.connect_timeout_seconds,
        ));

    let db = Database::connect(conn_opts).await.unwrap_or_else(|e| {
        error!(error = %e, "Failed to connect to database");
        std::process::exit(1);
    });

    // ── Step 3: Apply SQLite durability pragmas ────────────────────────────────
    // These are no-ops for PostgreSQL / MySQL — SeaORM skips them.
    if db.get_database_backend() == DbBackend::Sqlite {
        for pragma in &[
            "PRAGMA journal_mode = WAL",
            "PRAGMA synchronous  = FULL",
            "PRAGMA foreign_keys = ON",
            "PRAGMA busy_timeout = 5000",
        ] {
            db.execute(Statement::from_string(DbBackend::Sqlite, *pragma))
                .await
                .unwrap_or_else(|e| {
                    error!(error = %e, pragma, "Failed to apply SQLite pragma");
                    std::process::exit(1);
                });
        }
        info!("SQLite durability pragmas applied (WAL, synchronous=FULL)");
    }

    // ── Step 4: Run pending schema migrations ─────────────────────────────────
    Migrator::up(&db, None).await.unwrap_or_else(|e| {
        error!(error = %e, "Database migration failed");
        std::process::exit(1);
    });
    info!("Database migrations applied");

    // ── Step 5: Startup integrity check ───────────────────────────────────────
    // Run before binding the HTTP port so we serve no traffic on a corrupt DB.
    let check = jiezi_cloud_integrity::startup::run(&db).await;
    for w in &check.warnings {
        warn!("startup check: {w}");
    }
    if !check.is_ok() {
        for e in &check.errors {
            error!("startup check: {e}");
        }
        error!("Startup integrity check failed — aborting");
        std::process::exit(1);
    }
    info!("Startup integrity check passed");

    // ── Step 6: Background tasks ──────────────────────────────────────────────
    if cfg.database.backup.enabled {
        let _backup_handle = jiezi_cloud_integrity::backup::start_background_task(
            db.clone(),
            cfg.database.clone(),
        );
        info!("Database auto-backup task started");
    }

    // TODO(Phase 5): Start chunk integrity patrol once patrol::start_background_task
    // is implemented in jiezi-cloud-integrity.

    // ── Step 7: Read first-run setup flag then wire service objects ──────────
    let setup_done = SystemSettingsRepository::new(db.clone())
        .get_bool("setup_completed")
        .await
        .unwrap_or(false);

    if !setup_done {
        warn!(
            setup_url = "/api/v1/setup/status",
            "First-run setup not yet completed -- all API routes are gated"
        );
    } else {
        info!("First-run setup already completed");
    }

    let app_state = build_app_state(db, &cfg, setup_done).await;

    // ── Step 8a: Start QUIC file-transfer server ──────────────────────────────
    if cfg.quic.enabled {
        let quic_state = app_state.clone();
        let quic_cfg   = cfg.quic.clone();
        match jiezi_cloud_server::quic::QuicServer::new(&quic_cfg, quic_state).await {
            Ok(quic_server) => {
                match quic_server.local_addr() {
                    Ok(addr) => info!(bind = %addr, "QUIC file-transfer server started (JTP/1)"),
                    Err(_)   => info!("QUIC file-transfer server started (JTP/1)"),
                }
                tokio::spawn(async move { quic_server.run().await });
            }
            Err(e) => {
                error!(error = %e, "Failed to start QUIC server — continuing without it");
            }
        }
    } else {
        info!("QUIC file-transfer server disabled in config");
    }

    // ── Step 8b: Build per-IP rate limiter for authentication routes ──────────
    // Uses a token-bucket algorithm: each IP may send at most
    // `auth_rate_limit_per_minute` requests per minute to `/api/v1/auth/**`.
    // On burst the client gets a 429 Too Many Requests with a Retry-After
    // header — effectively a brute-force guard independent of the DB-level
    // account lockout.
    let auth_rate_secs = {
        let per_minute = cfg.security.auth_rate_limit_per_minute.max(1) as u64;
        // Ceil-divide so 20/min → 3 s/req, 60/min → 1 s/req.
        60u64.div_ceil(per_minute)
    };
    let auth_rate_cfg = GovernorConfigBuilder::default()
        .seconds_per_request(auth_rate_secs)
        // Allow a short burst equal to the configured per-minute quota so
        // legitimate page-loads (which trigger a few auth checks at once)
        // are not rejected immediately.
        .burst_size(cfg.security.auth_rate_limit_per_minute)
        .use_headers() // Emit `RateLimit-*` and `Retry-After` response headers.
        .finish()
        .expect("invalid auth rate-limiter configuration");

    // CORS allowed origins come from config; share via Arc so the closure
    // (called once per worker) can read the list without cloning the Vec.
    let cors_origins: Arc<Vec<String>> =
        Arc::new(cfg.security.cors_allowed_origins.clone());

    // ── Step 8: Build and run Actix-web HttpServer ────────────────────────────
    let workers = if cfg.server.workers == 0 {
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4)
    } else {
        cfg.server.workers
    };

    let bind_address = cfg.server.bind_address();
    info!(workers, bind = %bind_address, "Starting HTTP server");

    // ── OpenAPI specification ─────────────────────────────────────────────────
    // `#[derive(OpenApi)]` lives here at the binary boundary so it can
    // reference paths from all sub-crates once their handlers are annotated.
    //
    // TODO(openapi-paths): as route handlers gain `#[utoipa::path]`, add them
    //   to the `paths(...)` list below.
    // TODO(openapi-security): add `securitySchemes` for Bearer JWT once all
    //   protected handlers carry `#[utoipa::path(security(...))]` annotations.
    #[derive(OpenApi)]
    #[openapi(
        info(
            title = "Jiezi Cloud API",
            version = "0.1.0",
            description = "Jiezi Cloud document management system REST API"
        )
    )]
    struct ApiDoc;

    // Build once; `openapi_json` is cheap to clone (Arc-backed internally).
    let openapi = ApiDoc::openapi();

    /// Handler: GET /api/v1/openapi.json — serves the raw OpenAPI 3.1 spec.
    async fn serve_openapi(
        spec: web::Data<utoipa::openapi::OpenApi>,
    ) -> impl Responder {
        web::Json(spec.as_ref().clone())
    }

    let event_bus = web::Data::new(EventBus::default());

    HttpServer::new(move || {
        // ── Security: CORS ────────────────────────────────────────────────────
        // Built inside the closure because `Cors` is not `Clone`.
        let cors = if cors_origins.is_empty() {
            // No origins configured: permissive mode for development /
            // home-server single-host deployments.
            Cors::permissive()
        } else {
            // Restrict to explicitly-listed origins, allow usual auth headers,
            // support credentialed requests for cookie / Bearer flows.
            let mut c = Cors::default()
                .allowed_methods(vec!["GET", "POST", "PUT", "PATCH", "DELETE", "OPTIONS"])
                .allowed_headers(vec![
                    header::AUTHORIZATION,
                    header::CONTENT_TYPE,
                    header::ACCEPT,
                ])
                .supports_credentials()
                .max_age(3600);
            for origin in cors_origins.iter() {
                c = c.allowed_origin(origin);
            }
            c
        };

        // Per-IP token-bucket guard for the auth scope.
        let auth_governor = Governor::new(&auth_rate_cfg);
        App::new()
            .app_data(web::Data::new(app_state.clone()))
            // Shared SSE event bus — inject into EventBus::publish callers.
            .app_data(event_bus.clone())
            // Share the OpenAPI spec so `serve_openapi` can access it.
            .app_data(web::Data::new(openapi.clone()))
            .app_data(
                // Return JSON errors for malformed request bodies.
                web::JsonConfig::default().error_handler(|err, _req| {
                    let msg = err.to_string();
                    actix_web::error::InternalError::from_response(
                        err,
                        actix_web::HttpResponse::UnprocessableEntity().json(
                            serde_json::json!({ "code": 422, "message": msg }),
                        ),
                    )
                    .into()
                }),
            )            // setup_guard: short-circuits with 503 if the first-run wizard is
            // not done.  Runs after CORS / security-header layers so browsers
            // still receive those headers even during the setup phase.
            .wrap(from_fn(setup_guard))
            // Standard HTTP security headers (HSTS, CSP, X-Frame-Options, …).
            .wrap(from_fn(security_headers))
            // CORS: handles OPTIONS pre-flight and injects Allow-Origin headers.
            .wrap(cors)
            // Per-request tracing span — emits structured log fields:
            // request_id (UUID v7), http.method, http.target,
            // http.status_code, elapsed_milliseconds.
            .wrap(TracingLogger::default())
            // Route registration: the auth scope carries the per-IP rate
            // limiter; all other scopes are wired without it.
            // The integration-test harness uses `routes::configure` (which has
            // no governor) so tests are never rate-limited.
            .service(
                web::scope("/api/v1")
                    .service(web::scope("/setup").configure(routes::setup::configure))
                    .service(
                        web::scope("/auth")
                            .wrap(auth_governor)
                            .configure(routes::auth::configure),
                    )
                    .service(web::scope("/files").configure(routes::files::configure))
                    .service(web::scope("/admin").configure(routes::admin::configure))
                    .route("/events", web::get().to(routes::sse::events)),
            )
            // GET /health  — liveness probe for container orchestrators (Docker,
            // Kubernetes).  Always returns 200 OK with a small JSON body.
            // Bypassed by setup_guard so probes work before setup is done.
            .route("/health", web::get().to(|| async {
                actix_web::HttpResponse::Ok().json(serde_json::json!({
                    "status": "ok",
                    "version": env!("CARGO_PKG_VERSION")
                }))
            }))
            // GET /api/v1/openapi.json  — machine-readable OpenAPI 3.1 spec
            .route(
                "/api/v1/openapi.json",
                web::get().to(serve_openapi),
            )
            // GET /scalar  — interactive API explorer (Scalar UI)
            .service(Scalar::with_url("/scalar", ApiDoc::openapi()))
    })
    .workers(workers)
    .bind(&bind_address)?
    .run()
    .await
}
