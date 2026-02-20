//! Middleware that appends standard HTTP security headers to every response.
//!
//! # Usage
//!
//! ```rust,ignore
//! use actix_web::middleware::from_fn;
//! use crate::middleware::security_headers::security_headers;
//!
//! App::new()
//!     .wrap(from_fn(security_headers))
//!     // ... other middleware / routes
//! ```
//!
//! The middleware is **additive-only**: if a handler already set a header (for
//! example a custom `Content-Security-Policy` on a specific route) the
//! existing value is preserved and the default is *not* overwritten.
//!
//! # Headers injected
//!
//! | Header                      | Default value                                          |
//! |-----------------------------|--------------------------------------------------------|
//! | `Strict-Transport-Security` | `max-age=31536000; includeSubDomains`                  |
//! | `X-Frame-Options`           | `DENY`                                                 |
//! | `X-Content-Type-Options`    | `nosniff`                                              |
//! | `X-XSS-Protection`          | `1; mode=block` *(legacy browsers)*                   |
//! | `Referrer-Policy`           | `strict-origin-when-cross-origin`                      |
//! | `Content-Security-Policy`   | `default-src 'self'; img-src 'self' data:; ...`        |
//! | `Permissions-Policy`        | `camera=(), microphone=(), geolocation=()`             |

use actix_web::{
    body::MessageBody,
    dev::{ServiceRequest, ServiceResponse},
    http::header::{HeaderName, HeaderValue},
    middleware::Next,
    Error,
};

/// Actix-web `from_fn` compatible middleware function.
///
/// Call with `.wrap(actix_web::middleware::from_fn(security_headers))` on the
/// top-level [`actix_web::App`].
pub async fn security_headers<B: MessageBody>(
    req: ServiceRequest,
    next: Next<B>,
) -> Result<ServiceResponse<B>, Error> {
    let mut res = next.call(req).await?;
    let headers = res.headers_mut();

    // Each entry is (header-name, default-value).
    // All names are lowercase-ASCII as required by the HTTP/2 spec.
    const DEFAULTS: &[(&str, &str)] = &[
        // Enforce HTTPS for 1 year; also covers sub-domains.
        (
            "strict-transport-security",
            "max-age=31536000; includeSubDomains",
        ),
        // Deny embedding this site in <frame>, <iframe>, or <object>.
        ("x-frame-options", "DENY"),
        // Prevent browsers from MIME-sniffing response content types.
        ("x-content-type-options", "nosniff"),
        // Legacy XSS auditor kept for very old browsers.
        ("x-xss-protection", "1; mode=block"),
        // Don't leak the full URL to third-party origins.
        ("referrer-policy", "strict-origin-when-cross-origin"),
        // Basic CSP: only allow content from same origin.  Extend in
        // production if you serve external fonts/CDN assets.
        (
            "content-security-policy",
            "default-src 'self'; img-src 'self' data:; font-src 'self'; \
             style-src 'self' 'unsafe-inline'; frame-ancestors 'none'",
        ),
        // Deny access to powerful browser APIs not needed by a file-sync app.
        (
            "permissions-policy",
            "camera=(), microphone=(), geolocation=(), payment=()",
        ),
    ];

    for &(name, value) in DEFAULTS {
        // Safety: all name literals are valid header names.
        let name = HeaderName::from_static(name);
        // Only inject if the handler has not already set this header.
        if !headers.contains_key(&name) {
            // Safety: all value literals are valid header values.
            headers.insert(name, HeaderValue::from_static(value));
        }
    }

    Ok(res)
}
