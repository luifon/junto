use axum::Router;
use axum::extract::{Request, State};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use junto_protocol::error::{ErrorBody, ErrorCode};
use tower_http::trace::{DefaultMakeSpan, DefaultOnResponse, TraceLayer};
use tracing::Level;
use utoipa::openapi::security::{HttpAuthScheme, HttpBuilder, SecurityScheme};
use utoipa::{Modify, OpenApi};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::error::ApiError;
use crate::state::AppState;

pub mod account;
pub mod audit_log;
pub mod auth;
pub mod channels;
pub mod invites;
pub mod moderation;
pub mod server_info;
pub mod server_roles;
pub mod workspace_roles;
pub mod workspaces;

pub const SESSION_SECURITY: &str = "session";

#[derive(OpenApi)]
#[openapi(
    info(
        title = "Junto server API",
        description = "The public application API of a Junto server. Every route is under the \
                       version prefix, except the server information route, which tells a \
                       client which versions the server supports.",
        license(name = "Apache-2.0", identifier = "Apache-2.0")
    ),
    components(schemas(ErrorBody)),
    modifiers(&SessionSecurity),
    tags(
        (name = "server", description = "What a server tells any caller"),
        (name = "auth", description = "Challenges, sessions and the first owner"),
        (name = "account", description = "The caller's own account and devices"),
        (name = "joining", description = "Invites and join requests"),
        (name = "moderation", description = "Accounts, removal, bans and recovery"),
        (name = "server roles", description = "Roles that hold server permissions"),
        (name = "workspaces", description = "Workspaces, their members and roles"),
        (name = "channels", description = "Categories, channels and channel overrides"),
        (name = "audit", description = "The audit log")
    )
)]
struct ApiDocument;

struct SessionSecurity;

impl Modify for SessionSecurity {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        let scheme = SecurityScheme::Http(
            HttpBuilder::new()
                .scheme(HttpAuthScheme::Bearer)
                .description(Some("The token of a session, from a login."))
                .build(),
        );
        openapi
            .components
            .get_or_insert_with(Default::default)
            .add_security_scheme(SESSION_SECURITY, scheme);
    }
}

fn documented_routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::with_openapi(ApiDocument::openapi())
        .routes(routes!(server_info::get_server_info))
        .routes(routes!(auth::create_challenge))
        .routes(routes!(auth::log_in))
        .routes(routes!(auth::log_out))
        .routes(routes!(auth::claim_owner))
        .routes(routes!(account::get_own_account))
        .routes(routes!(account::list_devices))
        .routes(routes!(account::rename_device, account::remove_device))
        .routes(routes!(account::list_recoveries))
        .routes(routes!(account::request_device_link))
        .routes(routes!(account::look_up_device_link))
        .routes(routes!(account::approve_device_link))
        .routes(routes!(invites::create_invite, invites::list_invites))
        .routes(routes!(invites::revoke_invite))
        .routes(routes!(invites::redeem_invite))
        .routes(routes!(invites::join))
        .routes(routes!(
            invites::request_to_join,
            invites::list_join_requests
        ))
        .routes(routes!(invites::join_request_status))
        .routes(routes!(invites::approve_join_request))
        .routes(routes!(invites::deny_join_request))
        .routes(routes!(moderation::list_accounts))
        .routes(routes!(moderation::remove_account))
        .routes(routes!(moderation::ban_account, moderation::unban_account))
        .routes(routes!(moderation::recover_account))
        .routes(routes!(
            server_roles::list_server_roles,
            server_roles::create_server_role
        ))
        .routes(routes!(server_roles::update_server_role))
        .routes(routes!(
            server_roles::assign_server_role,
            server_roles::unassign_server_role
        ))
        .routes(routes!(
            workspaces::list_workspaces,
            workspaces::create_workspace
        ))
        .routes(routes!(workspaces::update_workspace))
        .routes(routes!(workspaces::list_members))
        .routes(routes!(workspaces::add_member))
        .routes(routes!(
            workspace_roles::list_workspace_roles,
            workspace_roles::create_workspace_role
        ))
        .routes(routes!(workspace_roles::update_workspace_role))
        .routes(routes!(
            workspace_roles::assign_workspace_role,
            workspace_roles::unassign_workspace_role
        ))
        .routes(routes!(channels::create_category))
        .routes(routes!(channels::update_category))
        .routes(routes!(channels::list_channels, channels::create_channel))
        .routes(routes!(channels::update_channel, channels::delete_channel))
        .routes(routes!(channels::list_channel_overrides))
        .routes(routes!(channels::set_channel_override))
        .routes(routes!(audit_log::get_server_audit_log))
        .routes(routes!(audit_log::get_workspace_audit_log))
}

/// The OpenAPI document of every route that [`router`] serves.
pub fn openapi() -> utoipa::openapi::OpenApi {
    documented_routes().into_openapi()
}

pub fn router(state: AppState) -> Router {
    let (router, _) = documented_routes().split_for_parts();
    router
        .fallback(unknown_path)
        .method_not_allowed_fallback(wrong_method)
        .layer(middleware::from_fn_with_state(state.clone(), time_limit))
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(DefaultMakeSpan::new().level(Level::INFO))
                .on_response(DefaultOnResponse::new().level(Level::INFO)),
        )
        .with_state(state)
}

async fn unknown_path() -> ApiError {
    ApiError::not_found()
}

async fn wrong_method() -> ApiError {
    ApiError::new(
        ErrorCode::MethodNotAllowed,
        "the path does not support this method",
    )
}

async fn time_limit(State(app): State<AppState>, request: Request, next: Next) -> Response {
    let limit = app.config.limits.request_timeout.unsigned_abs();
    match tokio::time::timeout(limit, next.run(request)).await {
        Ok(response) => response,
        Err(_) => ApiError::timeout().into_response(),
    }
}
