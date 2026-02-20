//! Shared helpers for all integration tests.
//!
//! Each integration test binary declares `mod common;` to pull this in.
//!
//! # What the helpers give you
//!
//! | Function            | Returns                                            |
//! |---------------------|----------------------------------------------------|
//! | `test_cfg()`        | `AppConfig` built from an inline TOML string       |
//! | `test_db()`         | Open in-memory SQLite DB with migrations applied   |
//! | `make_app(bool)`    | Fully-wired Actix-web test service                 |
//!
//! The `make_app` function parameter controls whether the first-run setup
//! wizard has already been completed:
//!
//! - `make_app(false)` — fresh DB, wizard guard **active** (all non-setup
//!   routes return 503).  Use this when you want to test the setup flow.
//!
//! - `make_app(true)` — wizard flag pre-set to `true`, so normal routes are
//!   accessible immediately without going through `/setup/complete` first.

#![allow(dead_code)]

use actix_http::Request;
use actix_web::{
    body::BoxBody,
    dev::{Service, ServiceResponse},
    middleware::from_fn,
    web, App,
};
use sea_orm::{ConnectOptions, Database, DatabaseConnection};
use sea_orm_migration::MigratorTrait;

use jiezi_cloud_config::AppConfig;
use jiezi_cloud_migration::Migrator;
use jiezi_cloud_server::{
    build_app_state,
    middleware::setup_guard::setup_guard,
    routes,
    routes::sse::EventBus,
    state::AppState,
};

// ─── Configuration ────────────────────────────────────────────────────────────

/// Build a minimal `AppConfig` from an inline TOML string.
///
/// Highlights:
/// - SQLite in-memory (`sqlite::memory:`) so tests never touch the filesystem.
/// - Single connection (`max_connections = 1`) so in-memory SQLite is shared
///   across all queries within the same test (each `DatabaseConnection`
///   object owns exactly one connection and therefore one isolated DB).
/// - Short JWT TTLs not required for tests, but kept at production defaults.
/// - Backup disabled — no need to create backup directories during tests.
pub fn test_cfg() -> AppConfig {
    let toml = r#"
        environment = "test"

        [server]
        host    = "127.0.0.1"
        port    = 8080
        workers = 1

        [database]
        url                     = "sqlite::memory:"
        max_connections         = 1
        min_connections         = 1
        connect_timeout_seconds = 5

        [database.backup]
        enabled          = false
        interval_minutes = 60
        keep_count       = 4
        dir              = "./test-tmp/db-backup"

        [auth]
        jwt_secret                = "test-secret-that-is-at-least-32-characters-long"
        access_token_ttl_seconds  = 900
        refresh_token_ttl_seconds = 2592000

        [storage]
        local_root       = "./test-tmp/files"
        max_upload_bytes = 10737418240

        [tracing]
        level  = "warn"
        format = "compact"
    "#;

    config::Config::builder()
        .add_source(config::File::from_str(toml, config::FileFormat::Toml))
        .build()
        .expect("test config build failed")
        .try_deserialize()
        .expect("test config deserialize failed")
}

// ─── Database ─────────────────────────────────────────────────────────────────

/// Open an in-memory SQLite database and run all pending migrations.
///
/// Each call returns a brand-new, fully-migrated, empty database — perfect
/// for test isolation without any filesystem footprint.
pub async fn test_db() -> DatabaseConnection {
    let mut opts = ConnectOptions::new("sqlite::memory:");
    // One connection = one in-memory database.  All queries in the test go
    // through this single connection, so there is no "second connection sees
    // an empty DB" problem that `sqlite::memory:` would otherwise cause with
    // a pool of size > 1.
    opts.max_connections(1).min_connections(1);

    let db = Database::connect(opts)
        .await
        .expect("test DB connect failed");

    Migrator::up(&db, None)
        .await
        .expect("test migrations failed");

    db
}

// ─── AppState factory ─────────────────────────────────────────────────────────

/// Build a fully-wired `AppState` backed by an in-memory database.
///
/// Pass `setup_completed = false` when your test exercises the setup wizard.
/// Pass `setup_completed = true` when you want to skip straight to auth.
pub async fn make_state(setup_completed: bool) -> AppState {
    let cfg = test_cfg();
    let db  = test_db().await;
    build_app_state(db, &cfg, setup_completed).await
}

// ─── Test service factory ─────────────────────────────────────────────────────

/// Build and initialise a fully-wired Actix-web test service.
///
/// The returned service includes:
/// - `setup_guard` middleware (gating on `state.setup_completed`)
/// - All `/api/v1/**` routes from [`routes::configure`]
/// - `GET /health` inline handler
/// - `EventBus` registered as `web::Data`
///
/// # Example
///
/// ```rust,ignore
/// let app   = make_app(true).await;          // setup already done
/// let req   = TestRequest::get().uri("/api/v1/auth/me")
///                 .insert_header(("Authorization", "Bearer <token>"))
///                 .to_request();
/// let resp  = test::call_service(&app, req).await;
/// assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
/// ```
pub async fn make_app(
    setup_completed: bool,
) -> impl Service<Request, Response = ServiceResponse<BoxBody>, Error = actix_web::Error> {
    let state     = make_state(setup_completed).await;
    let event_bus = web::Data::new(EventBus::default());

    actix_web::test::init_service(
        App::new()
            .app_data(web::Data::new(state))
            .app_data(event_bus)
            // Outermost middleware: blocks all non-setup routes until the
            // wizard is completed.  Must be the first `.wrap()` call so it
            // fires before routing, matching production order.
            .wrap(from_fn(setup_guard))
            .configure(routes::configure)
            // Inline health probe — bypassed by the setup guard.
            .route(
                "/health",
                web::get().to(|| async {
                    actix_web::HttpResponse::Ok().json(serde_json::json!({
                        "status":  "ok",
                        "version": "test"
                    }))
                }),
            ),
    )
    .await
}
