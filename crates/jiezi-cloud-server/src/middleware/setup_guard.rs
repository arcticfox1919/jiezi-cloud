//! Setup-guard middleware.
//!
//! Intercepts every incoming request and, when the first-run setup wizard has
//! not yet been completed, short-circuits with `503 Service Unavailable` and
//! a JSON body pointing the client to the setup endpoint.
//!
//! The check is a single atomic-boolean read (no database round-trip) so it
//! adds essentially zero overhead after setup is done.
//!
//! # Allowed paths (bypass the guard)
//!
//! | Path prefix / exact          | Reason                                  |
//! |------------------------------|-----------------------------------------|
//! | `/api/v1/setup`              | the wizard itself                       |
//! | `/api/v1/openapi.json`       | spec must be accessible before setup   |
//! | `/scalar`                    | API explorer must be accessible         |
//! | `/health`                    | liveness probe for container orchestr.  |

use std::sync::atomic::Ordering;

use actix_web::body::BoxBody;
use actix_web::dev::{ServiceRequest, ServiceResponse};
use actix_web::middleware::Next;
use actix_web::{web, Error, HttpResponse};

use crate::state::AppState;

/// Actix-web `from_fn` middleware function.
///
/// Register with:
/// ```rust,ignore
/// App::new()
///     .wrap(actix_web::middleware::from_fn(setup_guard))
/// ```
pub async fn setup_guard(
    req: ServiceRequest,
    next: Next<BoxBody>,
) -> Result<ServiceResponse<BoxBody>, Error> {
    let path = req.path();

    // Always let through: the setup wizard itself, API docs, health probe.
    if path.starts_with("/api/v1/setup")
        || path.starts_with("/scalar")
        || path.ends_with("/openapi.json")
        || path == "/health"
    {
        return next.call(req).await;
    }

    // Read the atomic flag -- zero database overhead.
    let setup_done = req
        .app_data::<web::Data<AppState>>()
        .map(|s| s.setup_completed.load(Ordering::Relaxed))
        .unwrap_or(true); // if state is missing, don't block (shouldn't happen)

    if !setup_done {
        let body = serde_json::json!({
            "code": 503,
            "message": "First-run setup is required before the server can be used.",
            "setup_status_url": "/api/v1/setup/status",
            "setup_complete_url": "/api/v1/setup/complete"
        });
        let resp = HttpResponse::ServiceUnavailable()
            .content_type("application/json")
            .json(body);
        return Ok(req.into_response(resp.map_into_boxed_body()));
    }

    next.call(req).await
}
