//! Outbound email service for Jiezi Cloud authentication flows.
//!
//! Wraps `lettre` to send 6-digit OTP codes via SMTP for three purposes:
//! - **Registration**: prove email ownership before the account is created.
//! - **Password reset**: verify identity before allowing a new password.
//! - **Account unlock**: self-service unlock after too many failed logins.
//!
//! # Development mode
//!
//! When `email.enabled = false` (the default) the service does **not**
//! connect to any SMTP server.  Instead it logs the OTP code at `INFO`
//! level so developers can copy it from the terminal:
//!
//! ```text
//! INFO jiezi_cloud_auth::email: [DEV] OTP for alice@example.com (register): 483920
//! ```

use lettre::{
    AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor,
    message::header::ContentType,
    transport::smtp::{authentication::Credentials, Error as SmtpError},
};
use tracing::{info, warn};

/// Error type for email operations.
#[derive(Debug, thiserror::Error)]
pub enum EmailError {
    #[error("SMTP address parse error: {0}")]
    AddressParse(String),
    #[error("message build error: {0}")]
    Build(#[from] lettre::error::Error),
    #[error("SMTP transport error: {0}")]
    Transport(String),
    #[error("SMTP configuration error: {0}")]
    Config(String),
}

impl From<SmtpError> for EmailError {
    fn from(e: SmtpError) -> Self {
        EmailError::Transport(e.to_string())
    }
}

/// Slim email service — either SMTP-backed or no-op (dev mode).
#[derive(Clone)]
pub struct EmailService {
    /// `Some(transport)` when `email.enabled = true`.
    transport:    Option<AsyncSmtpTransport<Tokio1Executor>>,
    from_address: String,
    from_name:    String,
}

impl EmailService {
    /// Construct an email service from config values.
    ///
    /// Returns `Ok(service)` even when `enabled = false`; SMTP credentials
    /// are not validated in that case.
    pub fn new(
        enabled:       bool,
        smtp_host:     &str,
        smtp_port:     u16,
        smtp_username: &str,
        smtp_password: &str,
        from_address:  String,
        from_name:     String,
    ) -> Result<Self, EmailError> {
        let transport = if enabled {
            let creds = Credentials::new(
                smtp_username.to_owned(),
                smtp_password.to_owned(),
            );

            // Use STARTTLS on 587; implicit TLS on 465; plain on anything else.
            let builder = if smtp_port == 465 {
                AsyncSmtpTransport::<Tokio1Executor>::relay(smtp_host)
                    .map_err(|e: SmtpError| EmailError::Config(e.to_string()))?
            } else {
                AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(smtp_host)
                    .map_err(|e: SmtpError| EmailError::Config(e.to_string()))?
            };

            Some(
                builder
                    .port(smtp_port)
                    .credentials(creds)
                    .build(),
            )
        } else {
            None
        };

        Ok(Self { transport, from_address, from_name })
    }

    /// Send a 6-digit OTP code to `to_address`.
    ///
    /// `purpose` is one of `"register"`, `"reset_password"`, or `"unlock"`.
    /// In dev mode (no SMTP) the code is logged at INFO level and the call
    /// always succeeds.
    pub async fn send_otp(
        &self,
        to_address: &str,
        to_name:    &str,
        code:       &str,
        purpose:    &str,
    ) -> Result<(), EmailError> {
        let (subject, body) = build_email_content(to_name, code, purpose);

        if let Some(transport) = &self.transport {
            // ── SMTP send ────────────────────────────────────────────────────
            let from: lettre::message::Mailbox =
                format!("{} <{}>", self.from_name, self.from_address)
                    .parse()
                    .map_err(|e: lettre::address::AddressError| {
                        EmailError::AddressParse(e.to_string())
                    })?;

            let to: lettre::message::Mailbox = format!("{to_name} <{to_address}>")
                .parse()
                .map_err(|e: lettre::address::AddressError| {
                    EmailError::AddressParse(e.to_string())
                })?;

            let msg = Message::builder()
                .from(from)
                .to(to)
                .subject(subject)
                .header(ContentType::TEXT_PLAIN)
                .body(body)?;

            match transport.send(msg).await {
                Err(e) => warn!(
                    error   = %e,
                    to      = to_address,
                    purpose,
                    "Failed to send OTP email via SMTP"
                ),
                Ok(_)  => {}
            }
        } else {
            // ── Dev / no-op mode ──────────────────────────────────────────────
            info!(
                to      = to_address,
                purpose,
                code,
                "[DEV] email OTP (email.enabled = false)"
            );
        }

        Ok(())
    }
}

// ── Email content builder ─────────────────────────────────────────────────────

fn build_email_content(to_name: &str, code: &str, purpose: &str) -> (String, String) {
    match purpose {
        "register" => (
            "Your Jiezi Cloud registration code".to_owned(),
            format!(
                "Hi {to_name},\n\n\
                 Your email verification code is:\n\n\
                     {code}\n\n\
                 Enter this code in the registration form to complete sign-up.\n\
                 The code expires in 10 minutes.\n\n\
                 If you did not attempt to register, you can safely ignore this email.\n\n\
                 — Jiezi Cloud"
            ),
        ),
        "reset_password" => (
            "Your Jiezi Cloud password-reset code".to_owned(),
            format!(
                "Hi {to_name},\n\n\
                 Your password-reset code is:\n\n\
                     {code}\n\n\
                 Enter this code along with your new password to complete the reset.\n\
                 The code expires in 15 minutes.\n\n\
                 If you did not request a password reset, please change your password\n\
                 immediately and contact the administrator.\n\n\
                 — Jiezi Cloud"
            ),
        ),
        "unlock" => (
            "Your Jiezi Cloud account-unlock code".to_owned(),
            format!(
                "Hi {to_name},\n\n\
                 Your account-unlock code is:\n\n\
                     {code}\n\n\
                 Enter this code to unlock your account after too many failed login attempts.\n\
                 The code expires in 15 minutes.\n\n\
                 If you did not request an unlock, your account may be under attack.\n\
                 Contact the administrator immediately.\n\n\
                 — Jiezi Cloud"
            ),
        ),
        "change_password" => (
            "Your Jiezi Cloud password-change code".to_owned(),
            format!(
                "Hi {to_name},\n\n\
                 Your password-change verification code is:\n\n\
                     {code}\n\n\
                 Enter this code along with your new password to complete the change.\n\
                 The code expires in 10 minutes.\n\n\
                 If you did not request a password change, your account may be compromised.\n\
                 Change your password immediately and contact the administrator.\n\n\
                 — Jiezi Cloud"
            ),
        ),
        other => (
            format!("Jiezi Cloud verification code ({other})"),
            format!("Hi {to_name},\n\nYour verification code is: {code}\n\n— Jiezi Cloud"),
        ),
    }
}
