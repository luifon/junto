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
use utoipa_axum::router::{OpenApiRouter, UtoipaMethodRouter};
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
pub mod moderation_notices;
pub mod notifications;
pub mod pins;
pub mod recordings;
pub mod retention;
pub mod search;
pub mod server_info;
pub mod server_logo;
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

/// Adds the routes that `made` makes to the documented routes.
///
/// Each call of `routes!` makes a large value, and a build without optimization gives
/// every such value of one function a place of its own on the stack. With every route in
/// one function, that function needed more than 1.5 MiB of stack in such a build, and a
/// thread has 2 MiB. Here each value is made in a function of its own, which returns
/// before the next one is called, so the stack that the routes need does not grow with
/// their number (ADR 0906).
fn add(router: &mut OpenApiRouter<AppState>, made: fn() -> UtoipaMethodRouter<AppState>) {
    *router = std::mem::take(router).routes(made());
}

fn documented_routes() -> OpenApiRouter<AppState> {
    let mut router = OpenApiRouter::with_openapi(ApiDocument::openapi());
    add(&mut router, || routes!(server_info::get_server_info));
    add(&mut router, || {
        routes!(
            server_logo::get_logo,
            server_logo::set_logo,
            server_logo::remove_logo
        )
    });
    add(&mut router, || routes!(health::get_health));
    add(&mut router, || routes!(threads::history, threads::reply));
    add(&mut router, || routes!(threads::fork));
    add(&mut router, || {
        routes!(threads::read_state, threads::mark_read)
    });
    add(&mut router, || routes!(threads::unread));
    add(&mut router, || {
        routes!(threads::set_follow, threads::remove_follow)
    });
    add(&mut router, || routes!(threads::followed));
    add(&mut router, || routes!(pins::list));
    add(&mut router, || routes!(pins::pin, pins::unpin));
    add(&mut router, || routes!(search::search));
    add(&mut router, || {
        routes!(notifications::get_settings, notifications::set_settings)
    });
    add(&mut router, || {
        routes!(
            notifications::get_default,
            notifications::set_default,
            notifications::remove_default
        )
    });
    add(&mut router, || routes!(notifications::list));
    add(&mut router, || routes!(notifications::mark_all_read));
    add(&mut router, || routes!(notifications::mark_read));
    add(&mut router, || {
        routes!(retention::get_workspace, retention::set_workspace)
    });
    add(&mut router, || {
        routes!(retention::get_direct, retention::set_direct)
    });
    add(&mut router, || {
        routes!(retention::get_recording, retention::set_recording)
    });
    add(&mut router, || routes!(attachments::start_upload));
    add(&mut router, || {
        routes!(attachments::get_limits, attachments::set_limits)
    });
    add(&mut router, || {
        routes!(attachments::get_upload, attachments::cancel_upload)
    });
    add(&mut router, || routes!(attachments::put_chunk));
    add(&mut router, || routes!(attachments::complete_upload));
    add(&mut router, || routes!(attachments::download));
    add(&mut router, || {
        routes!(messages::channel_history, messages::send_channel_message)
    });
    add(&mut router, || {
        routes!(
            messages::conversation_history,
            messages::send_conversation_message
        )
    });
    add(&mut router, || {
        routes!(
            messages::read_channel_messages,
            messages::channel_message_state
        )
    });
    add(&mut router, || {
        routes!(
            messages::read_conversation_messages,
            messages::conversation_message_state
        )
    });
    add(&mut router, || {
        routes!(
            messages::get_message,
            messages::edit_message,
            messages::delete_message
        )
    });
    add(&mut router, || routes!(messages::add_reaction));
    add(&mut router, || routes!(messages::remove_reaction));
    add(&mut router, || {
        routes!(messages::open_conversation, messages::list_conversations)
    });
    add(&mut router, || routes!(auth::create_challenge));
    add(&mut router, || routes!(auth::log_in));
    add(&mut router, || routes!(auth::log_out));
    add(&mut router, || routes!(auth::claim_owner));
    add(&mut router, || {
        routes!(account::get_own_account, account::update_own_account)
    });
    add(&mut router, || {
        routes!(account::set_avatar, account::remove_avatar)
    });
    add(&mut router, || routes!(account::get_profile));
    add(&mut router, || routes!(account::get_avatar));
    add(&mut router, || routes!(account::list_devices));
    add(&mut router, || {
        routes!(account::rename_device, account::remove_device)
    });
    add(&mut router, || routes!(account::list_recoveries));
    add(&mut router, || routes!(account::list_recovery_notices));
    add(&mut router, || {
        routes!(account::acknowledge_recovery_notice)
    });
    add(&mut router, || routes!(account::request_device_link));
    add(&mut router, || routes!(account::look_up_device_link));
    add(&mut router, || routes!(account::approve_device_link));
    add(&mut router, || {
        routes!(invites::create_invite, invites::list_invites)
    });
    add(&mut router, || routes!(invites::revoke_invite));
    add(&mut router, || routes!(invites::redeem_invite));
    add(&mut router, || routes!(invites::join));
    add(&mut router, || {
        routes!(invites::request_to_join, invites::list_join_requests)
    });
    add(&mut router, || routes!(invites::join_request_status));
    add(&mut router, || routes!(invites::approve_join_request));
    add(&mut router, || routes!(invites::deny_join_request));
    add(&mut router, || routes!(moderation::list_accounts));
    add(&mut router, || routes!(moderation::remove_account));
    add(&mut router, || {
        routes!(moderation::ban_account, moderation::unban_account)
    });
    add(&mut router, || routes!(moderation::recover_account));
    add(&mut router, || routes!(server_roles::reorder_server_roles));
    add(&mut router, || {
        routes!(workspace_roles::reorder_workspace_roles)
    });
    add(&mut router, || {
        routes!(
            server_roles::list_server_roles,
            server_roles::create_server_role
        )
    });
    add(&mut router, || {
        routes!(
            server_roles::update_server_role,
            server_roles::delete_server_role
        )
    });
    add(&mut router, || {
        routes!(
            server_roles::assign_server_role,
            server_roles::unassign_server_role
        )
    });
    add(&mut router, || {
        routes!(
            access_tokens::create_access_token,
            access_tokens::list_access_tokens
        )
    });
    add(&mut router, || routes!(access_tokens::revoke_access_token));
    add(&mut router, || routes!(structure::get_structure));
    add(&mut router, || routes!(structure::apply_structure_changes));
    add(&mut router, || {
        routes!(workspaces::list_workspaces, workspaces::create_workspace)
    });
    add(&mut router, || {
        routes!(workspaces::update_workspace, workspaces::delete_workspace)
    });
    add(&mut router, || routes!(workspaces::list_members));
    add(&mut router, || routes!(workspaces::add_member));
    add(&mut router, || routes!(workspaces::remove_member));
    add(&mut router, || {
        routes!(
            workspace_roles::list_workspace_roles,
            workspace_roles::create_workspace_role
        )
    });
    add(&mut router, || {
        routes!(
            workspace_roles::update_workspace_role,
            workspace_roles::delete_workspace_role
        )
    });
    add(&mut router, || {
        routes!(
            workspace_roles::assign_workspace_role,
            workspace_roles::unassign_workspace_role
        )
    });
    add(&mut router, || routes!(channels::create_category));
    add(&mut router, || {
        routes!(channels::update_category, channels::delete_category)
    });
    add(&mut router, || {
        routes!(channels::list_channels, channels::create_channel)
    });
    add(&mut router, || {
        routes!(channels::update_channel, channels::delete_channel)
    });
    add(&mut router, || {
        routes!(archive::archive_channel, archive::restore_channel)
    });
    add(&mut router, || routes!(archive::list_archived_channels));
    add(&mut router, || {
        routes!(archive::get_archive_period, archive::set_archive_period)
    });
    add(&mut router, || routes!(exports::export_channel));
    add(&mut router, || routes!(exports::export_conversation));
    add(&mut router, || routes!(exports::list_exports));
    add(&mut router, || {
        routes!(exports::get_export, exports::remove_export)
    });
    add(&mut router, || routes!(exports::get_export_file));
    add(&mut router, || routes!(channels::get_channel_permissions));
    add(&mut router, || routes!(channels::list_channel_overrides));
    add(&mut router, || routes!(channels::set_channel_override));
    add(&mut router, || routes!(channels::change_channel_override));
    add(&mut router, || {
        routes!(calls::get_own_call, calls::leave_call)
    });
    add(&mut router, || routes!(calls::report_call_state));
    add(&mut router, || {
        routes!(calls::report_screen_share, calls::end_screen_share)
    });
    add(&mut router, || {
        routes!(calls::raise_hand, calls::lower_hand)
    });
    add(&mut router, || routes!(calls::report_speaking));
    add(&mut router, || routes!(calls::get_channel_call));
    add(&mut router, || routes!(calls::update_call_settings));
    add(&mut router, || routes!(calls::join_channel_call));
    add(&mut router, || {
        routes!(calls::lock_call, calls::unlock_call)
    });
    add(&mut router, || routes!(calls::request_entry));
    add(&mut router, || routes!(calls::answer_entry_request));
    add(&mut router, || routes!(calls::cancel_entry_request));
    add(&mut router, || routes!(calls::invite_to_call));
    add(&mut router, || routes!(calls::accept_call_invitation));
    add(&mut router, || routes!(calls::refuse_call_invitation));
    add(&mut router, || routes!(calls::cancel_call_invitation));
    add(&mut router, || routes!(calls::move_participant));
    add(&mut router, || routes!(calls::join_move));
    add(&mut router, || routes!(calls::fail_move));
    add(&mut router, || {
        routes!(calls::start_meeting, calls::end_meeting)
    });
    add(&mut router, || {
        routes!(calls::block_microphone, calls::unblock_microphone)
    });
    add(&mut router, || {
        routes!(calls::block_sound, calls::unblock_sound)
    });
    add(&mut router, || routes!(calls::lower_participant_hand));
    add(&mut router, || routes!(calls::disconnect_participant));
    add(&mut router, || routes!(calls::join_direct_call));
    add(&mut router, || routes!(calls::refuse_direct_call));
    add(&mut router, || {
        routes!(
            calls::mute_direct_conversation,
            calls::unmute_direct_conversation
        )
    });
    add(&mut router, || {
        routes!(recordings::start_recording, recordings::stop_recording)
    });
    add(&mut router, || routes!(recordings::list_recordings));
    add(&mut router, || {
        routes!(recordings::get_recording, recordings::remove_recording)
    });
    add(&mut router, || routes!(recordings::get_segment_file));
    add(&mut router, || {
        routes!(transcripts::get_transcript, transcripts::request_transcript)
    });
    add(&mut router, || routes!(transcripts::list_transcript_lines));
    add(&mut router, || {
        routes!(transcripts::correct_transcript_line)
    });
    add(&mut router, || {
        routes!(transcripts::rename_transcript_speaker)
    });
    add(&mut router, || routes!(transcripts::get_transcript_text));
    add(&mut router, || {
        routes!(glossary::get_glossary, glossary::set_glossary)
    });
    add(&mut router, || routes!(audit_log::get_server_audit_log));
    add(&mut router, || routes!(audit_log::get_workspace_audit_log));
    add(&mut router, || routes!(moderation_notices::list));
    add(&mut router, || routes!(moderation_notices::prove));
    add(&mut router, || routes!(server_roles::change_permission));
    add(&mut router, || routes!(workspace_roles::change_permission));
    add(&mut router, || routes!(snapshot::get_snapshot));
    add(&mut router, || routes!(stream::open_event_stream));
    router
}

/// The OpenAPI document of every route that [`router`] serves.
pub fn openapi() -> utoipa::openapi::OpenApi {
    let mut document = documented_routes().into_openapi();
    for item in document.paths.paths.values_mut() {
        for operation in [
            &mut item.get,
            &mut item.post,
            &mut item.put,
            &mut item.patch,
            &mut item.delete,
        ]
        .into_iter()
        .flatten()
        {
            for (status, response) in &mut operation.responses.responses {
                if status.starts_with('2')
                    && let utoipa::openapi::RefOr::T(response) = response
                {
                    let mut header = utoipa::openapi::header::Header::default();
                    header.description = Some("May be present for verified session credentials on successful mutations, and on reads that commit changes; absent for unauthenticated requests, access tokens, or a failed receipt lookup after commit. Absence states no position and does not mean the change failed: do not repeat the change for that reason; load a fresh snapshot if reconciliation needs a known boundary. JSON Cursor {epoch,position} of the last committed event that the caller may receive at the full level with its current access after the commit, or null when no event was written or none may be received by the caller. Wait for the stream or a snapshot to reach this cursor; the header does not advance the stream cursor.".into());
                    response.headers.insert(
                        junto_protocol::events::EVENT_CURSOR_HEADER.into(),
                        header.into(),
                    );
                }
            }
        }
    }
    document
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
        .layer(middleware::from_fn_with_state(
            state.clone(),
            crate::receipts::middleware,
        ))
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
    crate::error::track_changes(async {
        match tokio::time::timeout(limit, next.run(request)).await {
            Ok(response) => response,
            Err(_) => ApiError::timeout().into_response(),
        }
    })
    .await
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

    /// The stack that the documented routes and their schemas may need: one quarter of the
    /// 2 MiB that a thread of the runtime and of the test harness has (ADR 0906).
    const ROUTES_STACK: usize = 512 * 1024;

    /// A thread with [`ROUTES_STACK`] builds the routes with their schemas. When they need
    /// more, the thread overflows its stack and the test process ends with the name of
    /// the thread: a function that makes many large values in one expression came back,
    /// in this crate or in the schema functions of the protocol.
    #[test]
    fn the_documented_routes_are_built_within_a_quarter_of_a_thread_stack() {
        let built = std::thread::Builder::new()
            .name("the documented routes within a quarter of a thread stack".to_owned())
            .stack_size(ROUTES_STACK)
            .spawn(|| documented_routes().into_openapi().paths.paths.len())
            .map(std::thread::JoinHandle::join);
        assert!(
            matches!(built, Ok(Ok(paths)) if paths > 100),
            "the thread that builds the routes did not end with the routes"
        );
    }
}
