//! OpenAPI 3.1 specification definition for Jiezi Cloud.
//!
//! Extracted to a top-level module so the server binary can output the spec
//! with `--export-openapi` without opening a database connection.

use utoipa::openapi::security::{HttpAuthScheme, HttpBuilder, SecurityScheme};
use utoipa::{Modify, OpenApi};

use jiezi_cloud_server::routes;

/// Injects the `bearer_auth` HTTP Bearer security scheme into the spec.
pub(crate) struct BearerAuth;

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
        routes::files::create_root,
        routes::files::list_roots,
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
/// Root OpenAPI document collector — aggregates all routes and schemas.
pub(crate) struct ApiDoc;
