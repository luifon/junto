use axum::Router;
use axum::body::Body;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::Method;
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use futures_util::StreamExt;
use junto_protocol::error::{ErrorBody, ErrorCode};
use tower_http::trace::{DefaultOnResponse, TraceLayer};
use tracing::Level;
use utoipa::openapi::security::{HttpAuthScheme, HttpBuilder, SecurityScheme};
use utoipa::{Modify, OpenApi};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use std::net::SocketAddr;

use crate::bodies::{Refused, Reservation};
use crate::error::ApiError;
use crate::rate_limit::{client_address, limiter_key};
use crate::state::AppState;

pub mod access_tokens;
pub mod account;
pub mod archive;
pub mod attachments;
pub mod audit_log;
pub mod auth;
pub mod calls;
pub mod channels;
pub mod exports;
pub mod glossary;
pub mod health;
pub mod internal;
pub mod invites;
pub mod layout;
pub mod messages;
pub mod moderation;
pub mod notifications;
pub mod pins;
pub mod recordings;
pub mod retention;
pub mod search;
pub mod server_info;
pub mod server_roles;
pub mod snapshot;
pub mod stream;
pub mod structure;
pub mod threads;
pub mod transcripts;
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
        (name = "access tokens", description = "Tokens of administrators for other programs"),
        (name = "structure", description = "The whole structure of the server and changes \
                                            of it in one request"),
        (name = "workspaces", description = "Workspaces, their members and roles"),
        (name = "channels", description = "Categories, channels and channel overrides"),
        (name = "calls", description = "Calls in voice channels and direct messages"),
        (name = "recordings", description = "Recordings of calls in voice channels, their \
                                             files, their transcripts and the glossary"),
        (name = "exports", description = "Exports of a channel or of a direct-message \
                                          conversation as one ZIP file"),
        (name = "audit", description = "The audit log"),
        (name = "events", description = "The event stream and the snapshot it continues")
    )
)]
struct ApiDocument;

struct SessionSecurity;

impl Modify for SessionSecurity {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        let scheme = SecurityScheme::Http(
            HttpBuilder::new()
                .scheme(HttpAuthScheme::Bearer)
                .description(Some(
                    "The token of a session, from a login. The routes that an access token \
                     reaches (ADR 0210) take the access token in the same header.",
                ))
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
        .routes(routes!(health::get_health))
        .routes(routes!(threads::history, threads::reply))
        .routes(routes!(threads::fork))
        .routes(routes!(threads::read_state, threads::mark_read))
        .routes(routes!(threads::unread))
        .routes(routes!(threads::set_follow, threads::remove_follow))
        .routes(routes!(threads::followed))
        .routes(routes!(pins::list))
        .routes(routes!(pins::pin, pins::unpin))
        .routes(routes!(search::search))
        .routes(routes!(
            notifications::get_settings,
            notifications::set_settings
        ))
        .routes(routes!(
            notifications::get_default,
            notifications::set_default,
            notifications::remove_default
        ))
        .routes(routes!(notifications::list))
        .routes(routes!(notifications::mark_all_read))
        .routes(routes!(notifications::mark_read))
        .routes(routes!(retention::get_workspace, retention::set_workspace))
        .routes(routes!(retention::get_direct, retention::set_direct))
        .routes(routes!(retention::get_recording, retention::set_recording))
        .routes(routes!(attachments::start_upload))
        .routes(routes!(attachments::get_limits, attachments::set_limits))
        .routes(routes!(attachments::get_upload, attachments::cancel_upload))
        .routes(routes!(attachments::put_chunk))
        .routes(routes!(attachments::complete_upload))
        .routes(routes!(attachments::download))
        .routes(routes!(
            messages::channel_history,
            messages::send_channel_message
        ))
        .routes(routes!(
            messages::conversation_history,
            messages::send_conversation_message
        ))
        .routes(routes!(
            messages::read_channel_messages,
            messages::channel_message_state
        ))
        .routes(routes!(
            messages::read_conversation_messages,
            messages::conversation_message_state
        ))
        .routes(routes!(
            messages::get_message,
            messages::edit_message,
            messages::delete_message
        ))
        .routes(routes!(messages::add_reaction))
        .routes(routes!(messages::remove_reaction))
        .routes(routes!(
            messages::open_conversation,
            messages::list_conversations
        ))
        .routes(routes!(auth::create_challenge))
        .routes(routes!(auth::log_in))
        .routes(routes!(auth::log_out))
        .routes(routes!(auth::claim_owner))
        .routes(routes!(
            account::get_own_account,
            account::update_own_account
        ))
        .routes(routes!(account::set_avatar, account::remove_avatar))
        .routes(routes!(account::get_profile))
        .routes(routes!(account::get_avatar))
        .routes(routes!(account::list_devices))
        .routes(routes!(account::rename_device, account::remove_device))
        .routes(routes!(account::list_recoveries))
        .routes(routes!(account::list_recovery_notices))
        .routes(routes!(account::acknowledge_recovery_notice))
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
        .routes(routes!(
            server_roles::update_server_role,
            server_roles::delete_server_role
        ))
        .routes(routes!(
            server_roles::assign_server_role,
            server_roles::unassign_server_role
        ))
        .routes(routes!(
            access_tokens::create_access_token,
            access_tokens::list_access_tokens
        ))
        .routes(routes!(access_tokens::revoke_access_token))
        .routes(routes!(structure::get_structure))
        .routes(routes!(structure::apply_structure_changes))
        .routes(routes!(
            workspaces::list_workspaces,
            workspaces::create_workspace
        ))
        .routes(routes!(
            workspaces::update_workspace,
            workspaces::delete_workspace
        ))
        .routes(routes!(workspaces::list_members))
        .routes(routes!(workspaces::add_member))
        .routes(routes!(
            workspace_roles::list_workspace_roles,
            workspace_roles::create_workspace_role
        ))
        .routes(routes!(
            workspace_roles::update_workspace_role,
            workspace_roles::delete_workspace_role
        ))
        .routes(routes!(
            workspace_roles::assign_workspace_role,
            workspace_roles::unassign_workspace_role
        ))
        .routes(routes!(channels::create_category))
        .routes(routes!(
            channels::update_category,
            channels::delete_category
        ))
        .routes(routes!(channels::list_channels, channels::create_channel))
        .routes(routes!(channels::update_channel, channels::delete_channel))
        .routes(routes!(archive::archive_channel, archive::restore_channel))
        .routes(routes!(archive::list_archived_channels))
        .routes(routes!(
            archive::get_archive_period,
            archive::set_archive_period
        ))
        .routes(routes!(exports::export_channel))
        .routes(routes!(exports::export_conversation))
        .routes(routes!(exports::list_exports))
        .routes(routes!(exports::get_export, exports::remove_export))
        .routes(routes!(exports::get_export_file))
        .routes(routes!(channels::get_channel_permissions))
        .routes(routes!(channels::list_channel_overrides))
        .routes(routes!(channels::set_channel_override))
        .routes(routes!(calls::get_own_call, calls::leave_call))
        .routes(routes!(calls::report_call_state))
        .routes(routes!(calls::report_screen_share, calls::end_screen_share))
        .routes(routes!(calls::raise_hand, calls::lower_hand))
        .routes(routes!(calls::report_speaking))
        .routes(routes!(calls::get_channel_call))
        .routes(routes!(calls::update_call_settings))
        .routes(routes!(calls::join_channel_call))
        .routes(routes!(calls::lock_call, calls::unlock_call))
        .routes(routes!(calls::request_entry))
        .routes(routes!(calls::answer_entry_request))
        .routes(routes!(calls::cancel_entry_request))
        .routes(routes!(calls::invite_to_call))
        .routes(routes!(calls::accept_call_invitation))
        .routes(routes!(calls::refuse_call_invitation))
        .routes(routes!(calls::cancel_call_invitation))
        .routes(routes!(calls::move_participant))
        .routes(routes!(calls::join_move))
        .routes(routes!(calls::fail_move))
        .routes(routes!(calls::start_meeting, calls::end_meeting))
        .routes(routes!(calls::block_microphone, calls::unblock_microphone))
        .routes(routes!(calls::block_sound, calls::unblock_sound))
        .routes(routes!(calls::lower_participant_hand))
        .routes(routes!(calls::disconnect_participant))
        .routes(routes!(calls::join_direct_call))
        .routes(routes!(calls::refuse_direct_call))
        .routes(routes!(
            calls::mute_direct_conversation,
            calls::unmute_direct_conversation
        ))
        .routes(routes!(
            recordings::start_recording,
            recordings::stop_recording
        ))
        .routes(routes!(recordings::list_recordings))
        .routes(routes!(
            recordings::get_recording,
            recordings::remove_recording
        ))
        .routes(routes!(recordings::get_segment_file))
        .routes(routes!(
            transcripts::get_transcript,
            transcripts::request_transcript
        ))
        .routes(routes!(transcripts::list_transcript_lines))
        .routes(routes!(transcripts::correct_transcript_line))
        .routes(routes!(transcripts::rename_transcript_speaker))
        .routes(routes!(transcripts::get_transcript_text))
        .routes(routes!(glossary::get_glossary, glossary::set_glossary))
        .routes(routes!(audit_log::get_server_audit_log))
        .routes(routes!(audit_log::get_workspace_audit_log))
        .routes(routes!(snapshot::get_snapshot))
        .routes(routes!(stream::open_event_stream))
}

/// The OpenAPI document of every route that [`router`] serves.
pub fn openapi() -> utoipa::openapi::OpenApi {
    documented_routes().into_openapi()
}

pub fn router(state: AppState) -> Router {
    let (router, _) = documented_routes().split_for_parts();
    router
        .route(
            internal::MEDIA_NOTIFICATIONS,
            axum::routing::post(internal::media_notification),
        )
        .route(layout::INDEX, axum::routing::get(layout::index))
        .route(layout::FILE, axum::routing::get(layout::file))
        .fallback(unknown_path)
        .method_not_allowed_fallback(wrong_method)
        .layer(middleware::from_fn_with_state(state.clone(), whole_body))
        .layer(middleware::from_fn_with_state(state.clone(), time_limit))
        .layer(middleware::from_fn_with_state(state.clone(), call_changes))
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(request_span)
                .on_response(DefaultOnResponse::new().level(Level::INFO)),
        )
        .with_state(state)
}

/// The span of the log record of one request: the method and the path (ADR 0008). The query
/// is left out, because the recorder loads the layout page with its room token in the query
/// (ADR 0615).
fn request_span<B>(request: &axum::http::Request<B>) -> tracing::Span {
    tracing::span!(
        Level::INFO,
        "request",
        method = %request.method(),
        path = %request.uri().path(),
    )
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

/// Marks the rooms of the calls that a request changed, after the request has ended, and
/// wakes their check (ADR 0509).
async fn call_changes(State(app): State<AppState>, request: Request, next: Next) -> Response {
    crate::calls::following(&app, next.run(request)).await
}

async fn time_limit(State(app): State<AppState>, request: Request, next: Next) -> Response {
    let limit = app.config.limits.request_timeout.unsigned_abs();
    match tokio::time::timeout(limit, next.run(request)).await {
        Ok(response) => response,
        Err(_) => ApiError::timeout().into_response(),
    }
}

/// Reads the body of a request before its route runs (ADR 0013).
///
/// A body that is too large is read to its end and discarded, up to a second bound, and then
/// answered with the error body. Without that, the server would close a connection on which
/// the client still sends, and the client could lose the response.
///
/// The bytes that the server holds are bounded for the whole server and for one client
/// address. A request that would pass a bound is answered at once, while its body still
/// arrives, so that slow requests cannot fill the memory. A request whose method has no
/// body in this API is read in the same way, and none of its bytes are held.
async fn whole_body(State(app): State<AppState>, request: Request, next: Next) -> Response {
    let limits = &app.config.limits;
    let (kept_bytes, read_bytes) = (limits.request_body_bytes, limits.request_body_drained_bytes);
    let (parts, body) = request.into_parts();
    let holds = [Method::POST, Method::PUT, Method::PATCH].contains(&parts.method);
    let peer = parts
        .extensions
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ConnectInfo(address)| address.ip());
    let forwarded: Vec<&str> = parts
        .headers
        .get_all("x-forwarded-for")
        .iter()
        .filter_map(|value| value.to_str().ok())
        .collect();
    let address = peer.map(|peer| {
        limiter_key(client_address(
            peer,
            &forwarded,
            &app.config.trusted_proxies,
        ))
    });
    let mut reservation = address.map(|address| Reservation::new(app.bodies.clone(), address));

    let mut chunks = body.into_data_stream();
    let mut kept = Vec::new();
    let mut read = 0usize;
    while let Some(chunk) = chunks.next().await {
        let Ok(chunk) = chunk else {
            return ApiError::invalid("the request body could not be read").into_response();
        };
        read = read.saturating_add(chunk.len());
        if read > read_bytes {
            break;
        }
        if !holds || read > kept_bytes {
            kept = Vec::new();
            reservation = None;
            continue;
        }
        let counted = match &mut reservation {
            Some(reservation) => reservation.grow(chunk.len()),
            None => Ok(()),
        };
        match counted {
            Ok(()) => kept.extend_from_slice(&chunk),
            Err(Refused::Address) => {
                return ApiError::new(
                    ErrorCode::RateLimited,
                    "this address sends too many request bodies at one time",
                )
                .retry_after(time::Duration::seconds(1))
                .into_response();
            }
            Err(Refused::Server) => {
                return ApiError::new(
                    ErrorCode::Busy,
                    "the server holds too many request bodies; send the same request again",
                )
                .retry_after(time::Duration::seconds(1))
                .into_response();
            }
        }
    }
    if holds && read > kept_bytes {
        return ApiError::invalid(format!(
            "the request body is larger than {kept_bytes} bytes"
        ))
        .into_response();
    }
    let response = next.run(Request::from_parts(parts, Body::from(kept))).await;
    drop(reservation);
    response
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::sync::{Arc, Mutex};

    use super::*;

    #[derive(Clone, Default)]
    struct Captured(Arc<Mutex<Vec<u8>>>);

    impl Write for Captured {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0
                .lock()
                .map_err(|_| std::io::Error::other("poisoned"))?
                .extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn the_log_record_of_a_request_has_the_path_and_not_the_query() {
        let captured = Captured::default();
        let writer = captured.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_ansi(false)
            .with_writer(move || writer.clone())
            .finish();
        let request = axum::http::Request::builder()
            .method(Method::GET)
            .uri("/recording-layout/?url=ws%3A%2F%2Fmedia&token=secret-room-token&layout=")
            .body(())
            .unwrap_or_default();
        tracing::subscriber::with_default(subscriber, || {
            let span = request_span(&request);
            let _entered = span.enter();
            tracing::info!("finished processing request");
        });
        let logged = String::from_utf8_lossy(
            &captured
                .0
                .lock()
                .map(|bytes| bytes.clone())
                .unwrap_or_default(),
        )
        .into_owned();
        assert!(logged.contains("method=GET"), "{logged}");
        assert!(logged.contains("path=/recording-layout/"), "{logged}");
        assert!(!logged.contains("secret-room-token"), "{logged}");
        assert!(!logged.contains("token="), "{logged}");
    }
}
