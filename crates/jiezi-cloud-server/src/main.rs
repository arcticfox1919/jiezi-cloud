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
use utoipa::openapi::security::{HttpAuthScheme, HttpBuilder, SecurityScheme};
use utoipa::{Modify, OpenApi};
use utoipa_swagger_ui::SwaggerUi;

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

    /// Adds the `bearer_auth` HTTP security scheme to the generated spec.
    struct BearerAuth;
    impl Modify for BearerAuth {
        fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
            openapi
                .components
                .get_or_insert_with(Default::default)
                .add_security_scheme(
                    "bearer_auth",
                    SecurityScheme::Http(
                        HttpBuilder::new()
                            .scheme(HttpAuthScheme::Bearer)
                            .bearer_format("JWT")
                            .description(Some(
                                "JWT access token — obtain from POST /api/v1/auth/login",
                            ))
                            .build(),
                    ),
                );
        }
    }

    #[derive(OpenApi)]
    #[openapi(
        info(
            title = "Jiezi Cloud API",
            version = "0.1.0",
            description = "Jiezi Cloud document management system REST API"
        ),
        modifiers(&BearerAuth),
        paths(
            // ── Setup ──────────────────────────────────────────────────────
            routes::setup::status,
            routes::setup::complete,
            // ── Auth ───────────────────────────────────────────────────────
            routes::auth::send_register_otp,
            routes::auth::register,
            routes::auth::login,
            routes::auth::refresh,
            routes::auth::me,
            routes::auth::update_me,
            routes::auth::send_change_password_otp,
            routes::auth::change_password,
            routes::auth::logout,
            routes::auth::list_sessions,
            routes::auth::revoke_session,
            routes::auth::forgot_password,
            routes::auth::reset_password,
            routes::auth::send_unlock_otp,
            routes::auth::unlock_account,
            // ── VFS / Files ────────────────────────────────────────────────
            routes::files::get_node,
            routes::files::list_children,
            routes::files::create_directory,
            routes::files::rename,
            routes::files::move_node,
            routes::files::copy_node,
            routes::files::soft_delete,
            routes::files::restore,
            routes::files::permanent_delete,
            routes::files::list_trash,
            routes::files::upload_file,
            routes::files::download_file,
            // ── Admin ──────────────────────────────────────────────────────
            routes::admin::list_users,
            routes::admin::get_user,
            routes::admin::change_role,
            routes::admin::set_status,
            routes::admin::reset_password,
            routes::admin::set_quota,
            routes::admin::delete_user,
        ),
        components(
            schemas(
                // Core ID types
                jiezi_cloud_core::types::UserId,
                jiezi_cloud_core::types::FileId,
                jiezi_cloud_core::types::SpaceId,
                jiezi_cloud_core::types::PageRequest,
                // User domain
                jiezi_cloud_core::models::user::Role,
                jiezi_cloud_core::models::user::User,
                jiezi_cloud_core::models::user::TokenPair,
                jiezi_cloud_core::models::user::SessionInfo,
                jiezi_cloud_core::models::user::RegisterRequest,
                jiezi_cloud_core::models::user::LoginRequest,
                jiezi_cloud_core::models::user::UpdateProfileRequest,
                jiezi_cloud_core::models::user::ChangeOwnPasswordRequest,
                jiezi_cloud_core::models::user::ChangeRoleRequest,
                jiezi_cloud_core::models::user::SetActiveRequest,
                jiezi_cloud_core::models::user::AdminResetPasswordRequest,
                jiezi_cloud_core::models::user::SetQuotaRequest,
                jiezi_cloud_core::models::user::SendOtpRequest,
                jiezi_cloud_core::models::user::ResetPasswordWithOtpRequest,
                jiezi_cloud_core::models::user::UnlockWithOtpRequest,
                // File domain
                jiezi_cloud_core::models::file::NodeType,
                jiezi_cloud_core::models::file::FileNode,
                jiezi_cloud_core::models::file::FileMetadata,
                // Setup schemas
                routes::setup::SetupStatusResponse,
                routes::setup::SetupCompleteRequest,
                routes::setup::SetupCompleteResponse,
                // Auth request bodies
                routes::auth::RefreshBody,
                routes::auth::LogoutBody,
                // File request bodies
                routes::files::CreateDirectoryBody,
                routes::files::RenameBody,
                routes::files::MoveBody,
                routes::files::CopyBody,
                routes::files::UploadQuery,
            )
        ),
        tags(
            (name = "setup",  description = "First-run setup wizard"),
            (name = "auth",   description = "Authentication & sessions"),
            (name = "files",  description = "Virtual file system — nodes, upload, download"),
            (name = "admin",  description = "Administrator user management"),
        )
    )]
    struct ApiDoc;

    // Build once and share via app_data so `serve_openapi` can return it.
    let openapi = ApiDoc::openapi();

    /// Handler: GET /api/v1/openapi.json — serves the raw OpenAPI 3.1 spec.
    async fn serve_openapi(spec: web::Data<utoipa::openapi::OpenApi>) -> impl Responder {
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
            // Share the OpenAPI spec so `serve_openapi` can return it.
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
                    .route("/events", web::get().to(routes::sse::events))
                    // GET /api/v1/openapi.json — machine-readable OpenAPI 3.1 spec.
                    // Must live inside the /api/v1 scope so actix-web matches it
                    // correctly after stripping the prefix.
                    .route("/openapi.json", web::get().to(serve_openapi)),
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
            // GET /swagger-ui/{_:.*}  — Swagger UI explorer
            .service(
                SwaggerUi::new("/swagger-ui/{_:.*}")
                    .url("/api/v1/openapi.json", openapi.clone()),
            )
    })
    .workers(workers)
    .bind(&bind_address)?
    .run()
    .await
}
