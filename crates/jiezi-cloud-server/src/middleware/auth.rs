//! JWT authentication extractor for Actix-web handlers.
//!
//! Handlers that require a logged-in user add `auth: AuthUser` to their
//! parameter list.  The extractor reads `Authorization: Bearer <token>` from
//! the incoming request headers, calls [`AuthService::verify_token`], and
//! either injects the decoded [`Claims`] or short-circuits with a `401`.
//!
//! # Example
//!
//! ```rust,ignore
//! async fn me(auth: AuthUser) -> impl Responder {
//!     HttpResponse::Ok().json(&auth.0)   // auth.0 = Claims
//! }
//! ```

use std::future::Future;
use std::pin::Pin;

use actix_web::dev::Payload;
use actix_web::{web, FromRequest, HttpRequest};

use jiezi_cloud_core::error::AppError;
use jiezi_cloud_core::models::user::Claims;

use crate::error::ApiError;
use crate::state::AppState;

// ─── AuthUser extractor ───────────────────────────────────────────────────────

/// A verified user, extracted from the `Authorization: Bearer <token>` header.
///
/// After extraction `auth.0` contains the decoded [`Claims`].
pub struct AuthUser(pub Claims);

impl FromRequest for AuthUser {
    type Error = ApiError;
    type Future = Pin<Box<dyn Future<Output = Result<Self, Self::Error>>>>;

    fn from_request(req: &HttpRequest, _payload: &mut Payload) -> Self::Future {
        let req = req.clone();
        Box::pin(async move {
            let state = req
                .app_data::<web::Data<AppState>>()
                .ok_or_else(|| ApiError(AppError::Internal("missing AppState".into())))?;

            // Extract "Authorization: Bearer <token>"
            let auth_header = req
                .headers()
                .get("Authorization")
                .and_then(|v| v.to_str().ok())
                .ok_or_else(|| {
                    ApiError(AppError::Unauthorized(
                        "missing Authorization header".into(),
                    ))
                })?;

            let token = auth_header.strip_prefix("Bearer ").ok_or_else(|| {
                ApiError(AppError::Unauthorized("expected Bearer token".into()))
            })?;

            let claims = state
                .auth
                .verify_token(token)
                .await
                .map_err(ApiError::from)?;

            Ok(AuthUser(claims))
        })
    }
}
