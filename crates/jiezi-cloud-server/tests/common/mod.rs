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
        jwt_private_key_pem       = "GENERATE"
        access_token_ttl_seconds  = 900
        refresh_token_ttl_seconds = 2592000

        [storage]
        local_root       = "./test-tmp/files"
        max_upload_bytes = 10737418240

        [security]
        max_login_attempts         = 5
        lockout_duration_secs      = 900
        auth_rate_limit_per_minute = 20
        cors_allowed_origins       = []
        max_password_bytes         = 128

        [email]
        enabled               = false
        smtp_host             = "localhost"
        smtp_port             = 587
        smtp_username         = ""
        smtp_password         = ""
        from_address          = "no-reply@test.local"
        from_name             = "Test"
        verification_required = false
        otp_ttl_secs          = 600

        [quic]
        enabled = false

        [tracing]
        level  = "warn"
        format = "compact"
    "#;

    // ── NOTE: if you add new top-level config keys, mirror them here. ──────────

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
// ─── HTTP helper functions (used by every test file) ─────────────────────────

/// POST JSON to `uri`, return the raw `ServiceResponse`.
pub async fn test_post_json<S, B>(
    app: &S,
    uri: &str,
    body: serde_json::Value,
) -> actix_web::dev::ServiceResponse<B>
where
    S: actix_web::dev::Service<
        actix_http::Request,
        Response = actix_web::dev::ServiceResponse<B>,
        Error = actix_web::Error,
    >,
{
    let req = actix_web::test::TestRequest::post()
        .uri(uri)
        .set_json(body)
        .to_request();
    actix_web::test::call_service(app, req).await
}

/// POST `/api/v1/setup/complete` for `username`; returns the raw response.
pub async fn post_setup_complete<S, B>(
    app: &S,
    username: &str,
) -> actix_web::dev::ServiceResponse<B>
where
    S: actix_web::dev::Service<
        actix_http::Request,
        Response = actix_web::dev::ServiceResponse<B>,
        Error = actix_web::Error,
    >,
{
    test_post_json(
        app,
        "/api/v1/setup/complete",
        serde_json::json!({
            "admin_username":       username,
            "admin_email":          format!("{username}@example.com"),
            "admin_password":       "Admin-Pass-1234!",
            "site_name":            "Test Jiezi Cloud",
            "registration_enabled": true
        }),
    )
    .await
}

/// POST `/api/v1/auth/register` and assert 201 Created.
pub async fn register_user<S, B>(
    app: &S,
    username: &str,
    email: &str,
    password: &str,
)
where
    S: actix_web::dev::Service<
        actix_http::Request,
        Response = actix_web::dev::ServiceResponse<B>,
        Error = actix_web::Error,
    >,
{
    let resp = test_post_json(
        app,
        "/api/v1/auth/register",
        serde_json::json!({ "username": username, "email": email, "password": password }),
    )
    .await;
    assert_eq!(
        resp.status(),
        actix_web::http::StatusCode::CREATED,
        "register_user({username}) failed with {}",
        resp.status()
    );
}

/// POST `/api/v1/auth/login` without asserting; returns the raw response.
pub async fn login_user<S, B>(
    app: &S,
    username: &str,
    password: &str,
) -> actix_web::dev::ServiceResponse<B>
where
    S: actix_web::dev::Service<
        actix_http::Request,
        Response = actix_web::dev::ServiceResponse<B>,
        Error = actix_web::Error,
    >,
{
    test_post_json(
        app,
        "/api/v1/auth/login",
        serde_json::json!({ "credential": username, "password": password }),
    )
    .await
}

/// POST `/api/v1/auth/login`, assert 200 OK, return the parsed token-pair JSON.
pub async fn do_login<S, B>(
    app: &S,
    username: &str,
    password: &str,
) -> serde_json::Value
where
    S: actix_web::dev::Service<
        actix_http::Request,
        Response = actix_web::dev::ServiceResponse<B>,
        Error = actix_web::Error,
    >,
    B: actix_web::body::MessageBody + Unpin,
{
    let resp = login_user(app, username, password).await;
    assert_eq!(
        resp.status(),
        actix_web::http::StatusCode::OK,
        "do_login({username}) failed with {}",
        resp.status()
    );
    actix_web::test::read_body_json(resp).await
}