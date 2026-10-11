//! The routes of calls (ADR 0501 to ADR 0514). Each changes the server's record of a call in
//! one transaction; the admission check carries the change to the media server.

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use junto_authz::{ChannelScope, Participation, Request, WorkspacePermission};
use junto_protocol::audit::AuditEvent;
use junto_protocol::call::{
    AnswerEntryRequest, CallInvitation, CallJoin, CallMove, CallPlace, ChannelCall, DirectMute,
    EntryOutcome, EntryRequest, InvitationOutcome, InviteToCall, LeaveReason, MeetingEnd,
    MoveOutcome, MoveParticipant, MuteDirectConversation, OwnCallState, OwnCallView,
    ReportCallState, ReportScreenShare, ReportSpeaking, UpdateCallSettings,
};
use junto_protocol::events::Event;
use junto_protocol::ids::{AccountId, CallInvitationId, CallMoveId, ChannelId, EntryRequestId};
use junto_protocol::structure::{Channel, ChannelKind};
use sqlx::{AssertSqlSafe, PgConnection};
use time::OffsetDateTime;

use crate::access::{Access, ChannelRecord};
use crate::audit::{self, About};
use crate::calls::admission::may_be_at;
use crate::calls::entry::{self, Fresh, Pass, not_in_call};
use crate::calls::speaking;
use crate::calls::store::{
    self, By, CallRow, INVITATION_COLUMNS, MOVE_COLUMNS, MemberRow, MoveRow,
};
use crate::db::StoredText;
use crate::error::{ApiError, ApiResult};
use crate::events;
use crate::extract::{ApiJson, ApiPath, Bearer};
use crate::proof;
use crate::state::AppState;
use crate::{read, transact};

/// A voice channel that the caller may see.
pub(super) struct Voice {
    pub(super) record: ChannelRecord,
    pub(super) scope: ChannelScope,
    pub(super) place: CallPlace,
}

impl Voice {
    pub(super) fn about(&self) -> About {
        About::Channel(self.record.channel.workspace_id, self.record.channel.id)
    }
}

/// Access to a voice channel is the permission to see it: a member who sees a voice channel
/// can enter it, unless its lock or its user limit stops the entry (ADR 0504).
pub(super) async fn voice(
    connection: &mut PgConnection,
    access: &Access,
    channel_id: ChannelId,
) -> ApiResult<Voice> {
    let (record, scope) = access.channel(connection, channel_id).await?;
    access.require(Request::Channel {
        scope: &scope,
        permission: WorkspacePermission::ViewChannel,
    })?;
    if record.channel.kind != ChannelKind::Voice {
        return Err(ApiError::conflict("calls take place in voice channels"));
    }
    let place = CallPlace::Channel {
        workspace_id: record.channel.workspace_id,
        channel_id,
    };
    Ok(Voice {
        record,
        scope,
        place,
    })
}

/// The call of a voice channel, locked for a change, and the caller's membership in it.
pub(super) async fn inside(
    connection: &mut PgConnection,
    access: &Access,
    voice: &Voice,
) -> ApiResult<(CallRow, MemberRow)> {
    let call = store::call(&mut *connection, By::Place(voice.place), true)
        .await?
        .ok_or_else(not_in_call)?;
    let member = store::member_of(connection, access.account_id, true)
        .await?
        .filter(|member| member.call_id == call.id)
        .ok_or_else(not_in_call)?;
    Ok((call, member))
}

/// The call of a voice channel, locked for a change, and one member of it, for a moderator.
///
/// In a voice channel that hides who is inside, a moderator who is outside gets `not_in_call`
/// for every account, so that the answer does not tell who is inside (ADR 0504).
async fn member_in(
    connection: &mut PgConnection,
    access: &Access,
    voice: &Voice,
    account_id: AccountId,
) -> ApiResult<(CallRow, MemberRow)> {
    let call = store::call(&mut *connection, By::Place(voice.place), true).await?;
    if voice.record.channel.hides_occupants {
        let caller = store::member_of(&mut *connection, access.account_id, false).await?;
        let is_inside = match (&call, caller) {
            (Some(call), Some(caller)) => caller.call_id == call.id,
            _ => false,
        };
        if !is_inside {
            return Err(not_in_call());
        }
    }
    let call = call.ok_or_else(ApiError::not_found)?;
    let member = store::member_of(connection, account_id, true)
        .await?
        .filter(|member| member.call_id == call.id)
        .ok_or_else(ApiError::not_found)?;
    Ok((call, member))
}

/// The call that the caller's account is in, when the calling device is the one in it and
/// the account may still be at the place of the call. A member row alone is not enough: it
/// can outlive the access until the next admission check.
async fn own(connection: &mut PgConnection, access: &Access) -> ApiResult<(CallRow, MemberRow)> {
    let member = store::member_of(&mut *connection, access.account_id, true)
        .await?
        .filter(|member| {
            member.device_id == access.device_id && member.session_id == access.session_id
        })
        .ok_or_else(not_in_call)?;
    let call = store::call(&mut *connection, By::Id(member.call_id), true)
        .await?
        .ok_or_else(not_in_call)?;
    if !may_be_at(connection, access, &call.place).await? {
        return Err(not_in_call());
    }
    Ok((call, member))
}

/// The call that the caller's account is in. An account that may no longer be at the place
/// of its call gets no call.
#[utoipa::path(
    get,
    path = "/api/v1/call",
    tag = "calls",
    security(("session" = [])),
    responses((status = 200, body = OwnCallView), ApiError)
)]
pub async fn get_own_call(
    State(app): State<AppState>,
    bearer: Bearer,
) -> ApiResult<Json<OwnCallView>> {
    let now = app.now();
    read!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        access.require(Request::OwnAccount)?;
        let Some(member) = store::member_of(connection, access.account_id, false).await? else {
            return Ok(OwnCallView { call: None });
        };
        let Some(call) = store::call(connection, By::Id(member.call_id), false).await? else {
            return Ok(OwnCallView { call: None });
        };
        if !may_be_at(connection, &access, &call.place).await? {
            return Ok(OwnCallView { call: None });
        }
        Ok(OwnCallView {
            call: Some(OwnCallState {
                own: member.own_call(&call),
                entry_requests: store::entry_requests(connection, now, &call).await?,
                call: store::view(connection, &call).await?,
            }),
        })
    })
    .map(Json)
}

/// Leaves the call that the caller's account is in, from whichever device.
#[utoipa::path(
    delete,
    path = "/api/v1/call",
    tag = "calls",
    security(("session" = [])),
    responses((status = 204), ApiError)
)]
pub async fn leave_call(State(app): State<AppState>, bearer: Bearer) -> ApiResult<StatusCode> {
    let now = app.now();
    proof::limit(&app.calls.actions, bearer.0, now)?;
    let left = transact!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        access.require(Request::OwnAccount)?;
        let call = entry::leave(connection, now, access.account_id, LeaveReason::Left).await?;
        Ok(call.map(|call| (access.account_id, call)))
    })?;
    if let Some((account_id, call)) = left {
        app.streams.calls().leave(account_id);
        app.calls.mark(call.room.as_str());
    }
    Ok(StatusCode::NO_CONTENT)
}

/// Reports the person's own microphone and sound settings, for the people who see the call.
#[utoipa::path(
    put,
    path = "/api/v1/call/state",
    tag = "calls",
    request_body = ReportCallState,
    security(("session" = [])),
    responses((status = 204), ApiError)
)]
pub async fn report_call_state(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiJson(body): ApiJson<ReportCallState>,
) -> ApiResult<StatusCode> {
    let now = app.now();
    proof::limit(&app.calls.state_reports, bearer.0, now)?;
    transact!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        access.require(Request::OwnAccount)?;
        let (call, member) = own(connection, &access).await?;
        if (member.self_muted, member.self_deafened) != (body.muted, body.deafened) {
            sqlx::query(
                "update call_members set self_muted = $3, self_deafened = $4
                 where call_id = $1 and account_id = $2",
            )
            .bind(call.id)
            .bind(access.account_id)
            .bind(body.muted)
            .bind(body.deafened)
            .execute(&mut *connection)
            .await?;
            store::announce_participant(connection, now, &call, access.account_id).await?;
        }
        Ok(())
    })?;
    Ok(StatusCode::NO_CONTENT)
}

/// Reports that the person shares a screen, and which sound the share sends. The server
/// shows the share for as long as the media server lists a screen track of the person.
#[utoipa::path(
    put,
    path = "/api/v1/call/screen-share",
    tag = "calls",
    request_body = ReportScreenShare,
    security(("session" = [])),
    responses((status = 204), ApiError)
)]
pub async fn report_screen_share(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiJson(body): ApiJson<ReportScreenShare>,
) -> ApiResult<StatusCode> {
    let now = app.now();
    proof::limit(&app.calls.state_reports, bearer.0, now)?;
    transact!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        access.require(Request::OwnAccount)?;
        let (call, member) = own(connection, &access).await?;
        if member.share_audio.as_deref() != Some(body.audio.stored()) {
            sqlx::query(
                "update call_members
                 set share_audio = $3, share_started_at = coalesce(share_started_at, $4)
                 where call_id = $1 and account_id = $2",
            )
            .bind(call.id)
            .bind(access.account_id)
            .bind(body.audio.stored())
            .bind(now)
            .execute(&mut *connection)
            .await?;
            if member.share_audio.is_none() {
                let started = store::Note::ScreenShareStarted;
                store::note(connection, now, &call, started, Some(access.account_id)).await?;
            }
            store::announce_participant(connection, now, &call, access.account_id).await?;
            app.calls.mark(call.room.as_str());
        }
        Ok(())
    })?;
    Ok(StatusCode::NO_CONTENT)
}

/// Reports that the person no longer shares a screen. Meeting mode that the person runs ends.
#[utoipa::path(
    delete,
    path = "/api/v1/call/screen-share",
    tag = "calls",
    security(("session" = [])),
    responses((status = 204), ApiError)
)]
pub async fn end_screen_share(
    State(app): State<AppState>,
    bearer: Bearer,
) -> ApiResult<StatusCode> {
    let now = app.now();
    proof::limit(&app.calls.state_reports, bearer.0, now)?;
    transact!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        access.require(Request::OwnAccount)?;
        let (call, member) = own(connection, &access).await?;
        if member.share_audio.is_some() {
            app.calls.mark(call.room.as_str());
            sqlx::query(
                "update call_members
                 set share_audio = null, share_started_at = null, share_seen_at = null,
                     share_missing_since = null
                 where call_id = $1 and account_id = $2",
            )
            .bind(call.id)
            .bind(access.account_id)
            .execute(&mut *connection)
            .await?;
            store::announce_participant(connection, now, &call, access.account_id).await?;
            let presents = call
                .meeting
                .as_ref()
                .is_some_and(|meeting| meeting.presenter == access.account_id);
            if presents {
                store::end_meeting(connection, now, &call, MeetingEnd::ShareStopped).await?;
            }
        }
        Ok(())
    })?;
    Ok(StatusCode::NO_CONTENT)
}

async fn set_hand(app: &AppState, bearer: &Bearer, raised: bool) -> ApiResult<StatusCode> {
    let now = app.now();
    proof::limit(&app.calls.state_reports, bearer.0, now)?;
    transact!(app, |connection| {
        let access = Access::open(connection, bearer, now).await?;
        access.require(Request::OwnAccount)?;
        let (call, member) = own(connection, &access).await?;
        if member.hand_raised_at.is_some() != raised {
            sqlx::query(
                "update call_members set hand_raised_at = $3
                 where call_id = $1 and account_id = $2",
            )
            .bind(call.id)
            .bind(access.account_id)
            .bind(raised.then_some(now))
            .execute(&mut *connection)
            .await?;
            store::announce_participant(connection, now, &call, access.account_id).await?;
        }
        Ok(())
    })?;
    Ok(StatusCode::NO_CONTENT)
}

/// Raises the person's hand in the call.
#[utoipa::path(
    put,
    path = "/api/v1/call/hand",
    tag = "calls",
    security(("session" = [])),
    responses((status = 204), ApiError)
)]
pub async fn raise_hand(State(app): State<AppState>, bearer: Bearer) -> ApiResult<StatusCode> {
    set_hand(&app, &bearer, true).await
}

/// Lowers the person's hand.
#[utoipa::path(
    delete,
    path = "/api/v1/call/hand",
    tag = "calls",
    security(("session" = [])),
    responses((status = 204), ApiError)
)]
pub async fn lower_hand(State(app): State<AppState>, bearer: Bearer) -> ApiResult<StatusCode> {
    set_hand(&app, &bearer, false).await
}

/// Reports whether the person speaks. The server relays a mark, not a level, to the people
/// who may see who is in the voice channel, at most once per second for one call.
#[utoipa::path(
    put,
    path = "/api/v1/call/speaking",
    tag = "calls",
    request_body = ReportSpeaking,
    security(("session" = [])),
    responses((status = 204), ApiError)
)]
pub async fn report_speaking(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiJson(body): ApiJson<ReportSpeaking>,
) -> ApiResult<StatusCode> {
    let now = app.now();
    proof::limit(&app.calls.speaking_reports, bearer.0, now)?;
    let relayed = read!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        access.require(Request::OwnAccount)?;
        let member = store::member_of(connection, access.account_id, false)
            .await?
            .filter(|member| {
                member.device_id == access.device_id && member.session_id == access.session_id
            })
            .ok_or_else(not_in_call)?;
        let call = store::call(connection, By::Id(member.call_id), false)
            .await?
            .ok_or_else(not_in_call)?;
        if !may_be_at(connection, &access, &call.place).await? {
            return Err(not_in_call());
        }
        let in_a_channel = matches!(call.place, CallPlace::Channel { .. });
        Ok((in_a_channel && member.connected_at.is_some()).then_some((call.id, access.account_id)))
    })?;
    if let Some((call_id, account_id)) = relayed {
        speaking::report(&app, call_id, account_id, body.speaking);
    }
    Ok(StatusCode::NO_CONTENT)
}

/// What the caller may see of the call in a voice channel: whether the channel is in use,
/// and who is inside unless the channel hides that from the people outside.
#[utoipa::path(
    get,
    path = "/api/v1/channels/{channel_id}/call",
    tag = "calls",
    params(("channel_id" = ChannelId, Path)),
    security(("session" = [])),
    responses((status = 200, body = ChannelCall), ApiError)
)]
pub async fn get_channel_call(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(channel_id): ApiPath<ChannelId>,
) -> ApiResult<Json<ChannelCall>> {
    let now = app.now();
    read!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        let voice = voice(connection, &access, channel_id).await?;
        visible_call(connection, &access, &voice.record.channel).await
    })
    .map(Json)
}

/// The call of a voice channel as the caller, who may see the channel, may see it.
pub async fn visible_call(
    connection: &mut PgConnection,
    access: &Access,
    channel: &Channel,
) -> ApiResult<ChannelCall> {
    let is_inside: bool = sqlx::query_scalar(
        "select exists (
             select 1 from call_members m
             join calls c on c.id = m.call_id
             where m.account_id = $1 and c.channel_id = $2)",
    )
    .bind(access.account_id)
    .bind(channel.id)
    .fetch_one(&mut *connection)
    .await?;
    store::channel_call(connection, channel, is_inside).await
}

/// Changes the user limit of a voice channel and whether it hides who is inside.
#[utoipa::path(
    patch,
    path = "/api/v1/channels/{channel_id}/call/settings",
    tag = "calls",
    params(("channel_id" = ChannelId, Path)),
    request_body = UpdateCallSettings,
    security(("session" = [])),
    responses((status = 200, body = Channel), ApiError)
)]
pub async fn update_call_settings(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(channel_id): ApiPath<ChannelId>,
    ApiJson(body): ApiJson<UpdateCallSettings>,
) -> ApiResult<Json<Channel>> {
    let largest = app.config.limits.calls.largest_user_limit;
    if let Some(Some(limit)) = body.user_limit
        && !(1..=largest).contains(&limit)
    {
        return Err(ApiError::invalid(format!(
            "user_limit is from 1 to {largest}"
        )));
    }
    let now = app.now();
    proof::limit(&app.calls.actions, bearer.0, now)?;
    let channel = transact!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        let voice = voice(connection, &access, channel_id).await?;
        access.require(Request::Channel {
            scope: &voice.scope,
            permission: WorkspacePermission::ManageChannel,
        })?;
        let mut channel = voice.record.channel.clone();
        if let Some(user_limit) = body.user_limit {
            channel.user_limit = user_limit;
        }
        if let Some(hides_occupants) = body.hides_occupants {
            channel.hides_occupants = hides_occupants;
        }
        if channel == voice.record.channel {
            return Ok(channel);
        }
        let limit = channel
            .user_limit
            .map(i32::try_from)
            .transpose()
            .map_err(|error| ApiError::internal(&error))?;
        sqlx::query("update channels set user_limit = $2, hides_occupants = $3 where id = $1")
            .bind(channel_id)
            .bind(limit)
            .bind(channel.hides_occupants)
            .execute(&mut *connection)
            .await?;
        let changed = AuditEvent::ChannelCallSettingsUpdated {
            user_limit: channel.user_limit,
            hides_occupants: channel.hides_occupants,
        };
        let actor = Some(access.account_id);
        audit::record(connection, now, actor, voice.about(), &changed).await?;
        let updated = Event::ChannelUpdated {
            channel: channel.clone(),
        };
        events::publish(connection, now, &updated).await?;
        if voice.record.channel.hides_occupants && !channel.hides_occupants {
            crate::recordings::store::occupants_shown(connection, now, channel_id).await?;
        }
        Ok(channel)
    })?;
    Ok(Json(channel))
}

/// Asks for the place of the caller in the call of a voice channel. The answer carries the
/// address of the media server and a join token that is valid for about one minute. The
/// caller needs sight of the channel; a lock or a full channel stops a caller whom nobody
/// inside has accepted. A call of the account on another device, or in another place, ends.
#[utoipa::path(
    post,
    path = "/api/v1/channels/{channel_id}/call/join",
    tag = "calls",
    params(("channel_id" = ChannelId, Path)),
    security(("session" = [])),
    responses((status = 200, body = CallJoin), ApiError)
)]
pub async fn join_channel_call(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(channel_id): ApiPath<ChannelId>,
) -> ApiResult<Json<CallJoin>> {
    let now = app.now();
    proof::limit(&app.calls.actions, bearer.0, now)?;
    let fresh = Fresh::new(&app)?;
    let (entered, account_id) = transact!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        let voice = voice(connection, &access, channel_id).await?;
        app.calls.admitting()?;
        let limit = voice.record.channel.user_limit;
        let place = voice.place;
        let none = Pass::None;
        let entered =
            entry::enter(connection, &app, now, &access, place, limit, &fresh, none).await?;
        Ok((entered, access.account_id))
    })?;
    let media = app.calls.admitting()?.clone();
    app.streams.calls().enter(account_id, entered.key);
    entered.join(&app, media.as_ref(), now).map(Json)
}

/// Locks the voice channel that the caller is in: only people who are invited, moved in or
/// accepted by a person inside enter until the channel is empty or unlocked.
#[utoipa::path(
    put,
    path = "/api/v1/channels/{channel_id}/call/lock",
    tag = "calls",
    params(("channel_id" = ChannelId, Path)),
    security(("session" = [])),
    responses((status = 204), ApiError)
)]
pub async fn lock_call(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(channel_id): ApiPath<ChannelId>,
) -> ApiResult<StatusCode> {
    set_lock(&app, &bearer, channel_id, true).await
}

/// Unlocks the voice channel that the caller is in.
#[utoipa::path(
    delete,
    path = "/api/v1/channels/{channel_id}/call/lock",
    tag = "calls",
    params(("channel_id" = ChannelId, Path)),
    security(("session" = [])),
    responses((status = 204), ApiError)
)]
pub async fn unlock_call(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(channel_id): ApiPath<ChannelId>,
) -> ApiResult<StatusCode> {
    set_lock(&app, &bearer, channel_id, false).await
}

async fn set_lock(
    app: &AppState,
    bearer: &Bearer,
    channel_id: ChannelId,
    locked: bool,
) -> ApiResult<StatusCode> {
    let now = app.now();
    proof::limit(&app.calls.actions, bearer.0, now)?;
    transact!(app, |connection| {
        let access = Access::open(connection, bearer, now).await?;
        let voice = voice(connection, &access, channel_id).await?;
        access.require(Request::Channel {
            scope: &voice.scope,
            permission: WorkspacePermission::LockRoom,
        })?;
        let (call, _) = inside(connection, &access, &voice).await?;
        if call.lock.is_some() == locked {
            return Ok(());
        }
        let lock = locked.then_some(junto_protocol::call::CallLock {
            by: access.account_id,
            at: now,
        });
        sqlx::query("update calls set locked_by = $2, locked_at = $3 where id = $1")
            .bind(call.id)
            .bind(lock.as_ref().map(|lock| lock.by))
            .bind(lock.as_ref().map(|lock| lock.at))
            .execute(&mut *connection)
            .await?;
        let changed = Event::CallLockChanged {
            call_id: call.id,
            place: call.place,
            lock,
        };
        events::publish(connection, now, &changed).await
    })?;
    Ok(StatusCode::NO_CONTENT)
}

/// Asks the people inside a locked or full voice channel to let the caller in. The request
/// expires after 60 seconds.
#[utoipa::path(
    post,
    path = "/api/v1/channels/{channel_id}/call/entry-requests",
    tag = "calls",
    params(("channel_id" = ChannelId, Path)),
    security(("session" = [])),
    responses((status = 201, body = EntryRequest), ApiError)
)]
pub async fn request_entry(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(channel_id): ApiPath<ChannelId>,
) -> ApiResult<(StatusCode, Json<EntryRequest>)> {
    let now = app.now();
    proof::limit(&app.calls.actions, bearer.0, now)?;
    let limits = &app.config.limits.calls;
    let id = EntryRequestId::generate();
    let expires_at = now + limits.entry_request_lifetime;
    let request = transact!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        let voice = voice(connection, &access, channel_id).await?;
        let open = || ApiError::conflict("the voice channel is open; join it");
        let call = store::call(connection, By::Place(voice.place), true)
            .await?
            .ok_or_else(open)?;
        let occupied = store::occupancy(connection, call.id).await?;
        let limit = voice.record.channel.user_limit;
        let is_full = limit.is_some_and(|limit| occupied >= i64::from(limit));
        if call.lock.is_none() && !is_full {
            return Err(open());
        }
        let member = store::member_of(connection, access.account_id, false).await?;
        if member.is_some_and(|member| member.call_id == call.id) {
            return Err(ApiError::conflict("the caller is inside the voice channel"));
        }
        sqlx::query(
            "delete from call_entry_requests
             where call_id = $1 and account_id = $2
               and (expires_at <= $3 or admits_until <= $3)",
        )
        .bind(call.id)
        .bind(access.account_id)
        .bind(now)
        .execute(&mut *connection)
        .await?;
        let waiting: Vec<AccountId> =
            sqlx::query_scalar("select account_id from call_entry_requests where call_id = $1")
                .bind(call.id)
                .fetch_all(&mut *connection)
                .await?;
        if waiting.contains(&access.account_id) {
            return Err(ApiError::conflict("a request of the caller waits"));
        }
        if waiting.len() >= usize::try_from(limits.most_waiting_per_call).unwrap_or(usize::MAX) {
            return Err(ApiError::limit_reached(
                "too many requests to enter this voice channel wait",
            ));
        }
        sqlx::query(
            "insert into call_entry_requests (id, call_id, account_id, created_at, expires_at)
             values ($1, $2, $3, $4, $5)",
        )
        .bind(id)
        .bind(call.id)
        .bind(access.account_id)
        .bind(now)
        .bind(expires_at)
        .execute(&mut *connection)
        .await
        .map_err(ApiError::concurrent)?;
        let request = store::entry_request(&call, (id, access.account_id, now, expires_at));
        let asked = store::Note::EntryRequested;
        store::note(connection, now, &call, asked, Some(access.account_id)).await?;
        let requested = Event::CallEntryRequested {
            request: request.clone(),
        };
        let channel = &voice.record.channel;
        events::tell_inside(
            connection,
            now,
            channel.workspace_id,
            channel.id,
            &requested,
        )
        .await?;
        Ok(request)
    })?;
    Ok((StatusCode::CREATED, Json(request)))
}

type StoredEntry = (
    junto_protocol::ids::CallId,
    AccountId,
    OffsetDateTime,
    OffsetDateTime,
    Option<AccountId>,
);

/// Answers a request to enter the voice channel that the caller is in. The first answer
/// applies; a second one gets `conflict`.
#[utoipa::path(
    post,
    path = "/api/v1/call/entry-requests/{request_id}/answer",
    tag = "calls",
    params(("request_id" = EntryRequestId, Path)),
    request_body = AnswerEntryRequest,
    security(("session" = [])),
    responses((status = 204), ApiError)
)]
pub async fn answer_entry_request(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(request_id): ApiPath<EntryRequestId>,
    ApiJson(body): ApiJson<AnswerEntryRequest>,
) -> ApiResult<StatusCode> {
    let now = app.now();
    proof::limit(&app.calls.actions, bearer.0, now)?;
    let admits_until = now + app.config.limits.calls.entry_admission;
    transact!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        access.require(Request::OwnAccount)?;
        let member = store::member_of(connection, access.account_id, false)
            .await?
            .ok_or_else(ApiError::not_found)?;
        let stored: Option<StoredEntry> = sqlx::query_as(
            "select call_id, account_id, created_at, expires_at, accepted_by
             from call_entry_requests where id = $1 and call_id = $2 for update",
        )
        .bind(request_id)
        .bind(member.call_id)
        .fetch_optional(&mut *connection)
        .await?;
        let (call_id, account_id, created_at, expires_at, accepted_by) =
            stored.ok_or_else(ApiError::not_found)?;
        let call = store::call(connection, By::Id(call_id), true)
            .await?
            .ok_or_else(ApiError::not_found)?;
        if !may_be_at(connection, &access, &call.place).await? {
            return Err(ApiError::not_found());
        }
        if accepted_by.is_some() || expires_at <= now {
            return Err(ApiError::conflict(
                "the request has an answer already or has expired",
            ));
        }
        let request = store::entry_request(&call, (request_id, account_id, created_at, expires_at));
        let by = Some(access.account_id);
        if body.accept {
            sqlx::query(
                "update call_entry_requests set accepted_by = $2, admits_until = $3
                 where id = $1",
            )
            .bind(request_id)
            .bind(access.account_id)
            .bind(admits_until)
            .execute(&mut *connection)
            .await?;
            store::tell_entry(connection, now, &request, EntryOutcome::Accepted, by).await
        } else {
            sqlx::query("delete from call_entry_requests where id = $1")
                .bind(request_id)
                .execute(&mut *connection)
                .await?;
            store::tell_entry(connection, now, &request, EntryOutcome::Denied, by).await
        }
    })?;
    Ok(StatusCode::NO_CONTENT)
}

/// Withdraws the caller's own request to enter a voice channel.
#[utoipa::path(
    delete,
    path = "/api/v1/call/entry-requests/{request_id}",
    tag = "calls",
    params(("request_id" = EntryRequestId, Path)),
    security(("session" = [])),
    responses((status = 204), ApiError)
)]
pub async fn cancel_entry_request(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(request_id): ApiPath<EntryRequestId>,
) -> ApiResult<StatusCode> {
    let now = app.now();
    proof::limit(&app.calls.actions, bearer.0, now)?;
    transact!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        access.require(Request::OwnAccount)?;
        let call_id: Option<junto_protocol::ids::CallId> = sqlx::query_scalar(
            "select call_id from call_entry_requests where id = $1 and account_id = $2",
        )
        .bind(request_id)
        .bind(access.account_id)
        .fetch_optional(&mut *connection)
        .await?;
        let call_id = call_id.ok_or_else(ApiError::not_found)?;
        let call = store::call(connection, By::Id(call_id), true)
            .await?
            .ok_or_else(ApiError::not_found)?;
        let own = Some(access.account_id.as_uuid());
        let unanswered = "account_id = $2 and accepted_by is null and $3::timestamptz is null";
        let cancelled = EntryOutcome::Cancelled;
        store::resolve_entries(connection, now, &call, unanswered, own, None, cancelled).await?;
        sqlx::query("delete from call_entry_requests where id = $1")
            .bind(request_id)
            .execute(&mut *connection)
            .await?;
        Ok(())
    })?;
    Ok(StatusCode::NO_CONTENT)
}

/// Invites a member who has access to the voice channel that the caller is in. The
/// invitation passes the lock and the user limit of the channel, and no permission. It
/// expires after 60 seconds.
#[utoipa::path(
    post,
    path = "/api/v1/channels/{channel_id}/call/invitations",
    tag = "calls",
    params(("channel_id" = ChannelId, Path)),
    request_body = InviteToCall,
    security(("session" = [])),
    responses((status = 201, body = CallInvitation), ApiError)
)]
pub async fn invite_to_call(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(channel_id): ApiPath<ChannelId>,
    ApiJson(body): ApiJson<InviteToCall>,
) -> ApiResult<(StatusCode, Json<CallInvitation>)> {
    let now = app.now();
    proof::limit(&app.calls.actions, bearer.0, now)?;
    let limits = &app.config.limits.calls;
    let id = CallInvitationId::generate();
    let expires_at = now + limits.invitation_lifetime;
    let invitation = transact!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        let voice = voice(connection, &access, channel_id).await?;
        let (call, _) = inside(connection, &access, &voice).await?;
        if body.account_id == access.account_id {
            return Err(ApiError::invalid("the caller is inside already"));
        }
        let acquaintance = crate::store::acquaintance(connection, &access, body.account_id).await?;
        if !access.allows(Request::ViewAccount(acquaintance)) {
            return Err(ApiError::not_found());
        }
        let invited = Access::of_account(connection, body.account_id)
            .await?
            .ok_or_else(ApiError::not_found)?;
        if !may_be_at(connection, &invited, &call.place).await? {
            return Err(ApiError::conflict(
                "the account has no access to the voice channel",
            ));
        }
        let member = store::member_of(connection, body.account_id, false).await?;
        if member.is_some_and(|member| member.call_id == call.id) {
            return Err(ApiError::conflict("the account is inside already"));
        }
        let waiting: Vec<AccountId> =
            sqlx::query_scalar("select account_id from call_invitations where call_id = $1")
                .bind(call.id)
                .fetch_all(&mut *connection)
                .await?;
        let most = usize::try_from(limits.most_waiting_per_call).unwrap_or(usize::MAX);
        if !waiting.contains(&body.account_id) && waiting.len() >= most {
            return Err(ApiError::limit_reached(
                "too many invitations into this voice channel wait",
            ));
        }
        let invited = Some(body.account_id.as_uuid());
        let earlier = "account_id = $2 and $3::timestamptz is null";
        let replaced = InvitationOutcome::Cancelled;
        store::resolve_invitations(connection, now, &call, earlier, invited, None, replaced)
            .await?;
        sqlx::query(
            "insert into call_invitations
                 (id, call_id, account_id, invited_by, created_at, expires_at)
             values ($1, $2, $3, $4, $5, $6)",
        )
        .bind(id)
        .bind(call.id)
        .bind(body.account_id)
        .bind(access.account_id)
        .bind(now)
        .bind(expires_at)
        .execute(&mut *connection)
        .await
        .map_err(ApiError::concurrent)?;
        let row = (id, body.account_id, access.account_id, now, expires_at);
        let invitation = store::invitation(&call, row);
        let invited = Event::CallInvited {
            invitation: invitation.clone(),
        };
        for account_id in [body.account_id, access.account_id] {
            events::tell_account(connection, now, account_id, &invited).await?;
        }
        Ok(invitation)
    })?;
    Ok((StatusCode::CREATED, Json(invitation)))
}

type StoredInvitation = (
    junto_protocol::ids::CallId,
    CallInvitationId,
    AccountId,
    AccountId,
    OffsetDateTime,
    OffsetDateTime,
);

/// An invitation that names the caller as `side` states, with its call locked for a change.
async fn own_invitation(
    connection: &mut PgConnection,
    invitation_id: CallInvitationId,
    side: &'static str,
    account_id: AccountId,
) -> ApiResult<(CallRow, CallInvitation)> {
    let stored: Option<StoredInvitation> = sqlx::query_as(AssertSqlSafe(format!(
        "select call_id, {INVITATION_COLUMNS} from call_invitations
         where id = $1 and {side} = $2 for update"
    )))
    .bind(invitation_id)
    .bind(account_id)
    .fetch_optional(&mut *connection)
    .await?;
    let (call_id, id, invited, invited_by, created_at, expires_at) =
        stored.ok_or_else(ApiError::not_found)?;
    let call = store::call(connection, By::Id(call_id), true)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let invitation = store::invitation(&call, (id, invited, invited_by, created_at, expires_at));
    Ok((call, invitation))
}

/// Accepts an invitation into a call and asks for the place of the caller in it.
#[utoipa::path(
    post,
    path = "/api/v1/call/invitations/{invitation_id}/acceptance",
    tag = "calls",
    params(("invitation_id" = CallInvitationId, Path)),
    security(("session" = [])),
    responses((status = 200, body = CallJoin), ApiError)
)]
pub async fn accept_call_invitation(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(invitation_id): ApiPath<CallInvitationId>,
) -> ApiResult<Json<CallJoin>> {
    let now = app.now();
    proof::limit(&app.calls.actions, bearer.0, now)?;
    let fresh = Fresh::new(&app)?;
    let (entered, account_id) = transact!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        access.require(Request::OwnAccount)?;
        let (call, invitation) =
            own_invitation(connection, invitation_id, "account_id", access.account_id).await?;
        if invitation.expires_at <= now || !may_be_at(connection, &access, &call.place).await? {
            return Err(ApiError::not_found());
        }
        app.calls.admitting()?;
        let (place, invited) = (call.place, Pass::Invitation);
        let entered =
            entry::enter(connection, &app, now, &access, place, None, &fresh, invited).await?;
        Ok((entered, access.account_id))
    })?;
    let media = app.calls.admitting()?.clone();
    app.streams.calls().enter(account_id, entered.key);
    entered.join(&app, media.as_ref(), now).map(Json)
}

/// Declines an invitation into a call.
#[utoipa::path(
    post,
    path = "/api/v1/call/invitations/{invitation_id}/refusal",
    tag = "calls",
    params(("invitation_id" = CallInvitationId, Path)),
    security(("session" = [])),
    responses((status = 204), ApiError)
)]
pub async fn refuse_call_invitation(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(invitation_id): ApiPath<CallInvitationId>,
) -> ApiResult<StatusCode> {
    end_invitation(
        &app,
        &bearer,
        invitation_id,
        "account_id",
        InvitationOutcome::Declined,
    )
    .await
}

/// Withdraws an invitation that the caller made.
#[utoipa::path(
    delete,
    path = "/api/v1/call/invitations/{invitation_id}",
    tag = "calls",
    params(("invitation_id" = CallInvitationId, Path)),
    security(("session" = [])),
    responses((status = 204), ApiError)
)]
pub async fn cancel_call_invitation(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(invitation_id): ApiPath<CallInvitationId>,
) -> ApiResult<StatusCode> {
    end_invitation(
        &app,
        &bearer,
        invitation_id,
        "invited_by",
        InvitationOutcome::Cancelled,
    )
    .await
}

async fn end_invitation(
    app: &AppState,
    bearer: &Bearer,
    invitation_id: CallInvitationId,
    side: &'static str,
    outcome: InvitationOutcome,
) -> ApiResult<StatusCode> {
    let now = app.now();
    proof::limit(&app.calls.actions, bearer.0, now)?;
    transact!(app, |connection| {
        let access = Access::open(connection, bearer, now).await?;
        access.require(Request::OwnAccount)?;
        let (call, invitation) =
            own_invitation(connection, invitation_id, side, access.account_id).await?;
        if !may_be_at(connection, &access, &call.place).await? {
            return Err(ApiError::not_found());
        }
        sqlx::query("delete from call_invitations where id = $1")
            .bind(invitation_id)
            .execute(&mut *connection)
            .await?;
        store::tell_invitation(connection, now, &invitation, outcome).await
    })?;
    Ok(StatusCode::NO_CONTENT)
}

/// Moves a person from the call of one voice channel into another voice channel, without a
/// prompt. The device of the person joins the destination first; the person leaves the
/// source when the server has seen that join. A move that does not complete in 30 seconds
/// changes nothing.
#[utoipa::path(
    post,
    path = "/api/v1/channels/{channel_id}/call/participants/{account_id}/move",
    tag = "calls",
    params(("channel_id" = ChannelId, Path), ("account_id" = AccountId, Path)),
    request_body = MoveParticipant,
    security(("session" = [])),
    responses((status = 201, body = CallMove), ApiError)
)]
pub async fn move_participant(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath((channel_id, account_id)): ApiPath<(ChannelId, AccountId)>,
    ApiJson(body): ApiJson<MoveParticipant>,
) -> ApiResult<(StatusCode, Json<CallMove>)> {
    if body.to_channel_id == channel_id {
        return Err(ApiError::invalid("the destination is the source"));
    }
    let now = app.now();
    proof::limit(&app.calls.actions, bearer.0, now)?;
    let id = CallMoveId::generate();
    let expires_at = now + app.config.limits.calls.move_lifetime;
    let fresh = Fresh::new(&app)?;
    let identity = fresh.identity(account_id);
    let moved = transact!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        let source = voice(connection, &access, channel_id).await?;
        access.require(Request::Channel {
            scope: &source.scope,
            permission: WorkspacePermission::MoveMembers,
        })?;
        let destination = voice(connection, &access, body.to_channel_id).await?;
        access.require(Request::Channel {
            scope: &destination.scope,
            permission: WorkspacePermission::MoveMembers,
        })?;
        let (from, member) = member_in(connection, &access, &source, account_id).await?;
        let moved_account = Access::of_session(
            connection,
            member.account_id,
            member.device_id,
            member.session_id,
            now,
        )
        .await?
        .ok_or_else(ApiError::not_found)?;
        if !may_be_at(connection, &moved_account, &destination.place).await? {
            return Err(ApiError::conflict(
                "the person has no access to the destination",
            ));
        }
        app.calls.admitting()?;
        let place = destination.place;
        let to = store::call_for_entry(connection, now, place, fresh.call_id, &fresh.room).await?;
        app.calls.admitting_into(&to.room)?;
        let waiting: bool =
            sqlx::query_scalar("select exists (select 1 from call_moves where account_id = $1)")
                .bind(account_id)
                .fetch_one(&mut *connection)
                .await?;
        if waiting {
            return Err(ApiError::conflict("a move of the person waits"));
        }
        sqlx::query(
            "insert into call_moves
                 (id, account_id, from_call_id, to_call_id, moved_by, identity, created_at,
                  expires_at)
             values ($1, $2, $3, $4, $5, $6, $7, $8)",
        )
        .bind(id)
        .bind(account_id)
        .bind(from.id)
        .bind(to.id)
        .bind(access.account_id)
        .bind(&identity)
        .bind(now)
        .bind(expires_at)
        .execute(&mut *connection)
        .await
        .map_err(ApiError::concurrent)?;
        let pending = MoveRow {
            id,
            account_id,
            from_call_id: from.id,
            to_call_id: to.id,
            moved_by: access.account_id,
            identity: identity.clone(),
            expires_at,
        };
        let call_move = store::call_move(&pending, from.place, to.place);
        let requested = Event::CallMoveRequested {
            call_move: call_move.clone(),
        };
        events::tell_account(connection, now, account_id, &requested).await?;
        if access.account_id != account_id {
            events::tell_account(connection, now, access.account_id, &requested).await?;
        }
        Ok(call_move)
    })?;
    Ok((StatusCode::CREATED, Json(moved)))
}

/// The move of the caller's account that waits, with its two calls. The destination is
/// locked for a change. Only the session that holds the place in the source acts on a move:
/// it gets the join of the destination, and it reports that the join failed.
async fn own_move(
    connection: &mut PgConnection,
    access: &Access,
    move_id: CallMoveId,
) -> ApiResult<(MoveRow, CallRow, CallRow)> {
    let pending: Option<MoveRow> = sqlx::query_as(AssertSqlSafe(format!(
        "select {MOVE_COLUMNS} from call_moves where id = $1 and account_id = $2 for update"
    )))
    .bind(move_id)
    .bind(access.account_id)
    .fetch_optional(&mut *connection)
    .await?;
    let pending = pending.ok_or_else(ApiError::not_found)?;
    let member = store::member_of(&mut *connection, access.account_id, false).await?;
    let in_the_source = member.is_some_and(|member| {
        member.call_id == pending.from_call_id
            && member.device_id == access.device_id
            && member.session_id == access.session_id
    });
    if !in_the_source {
        return Err(not_in_call());
    }
    let from = store::call(&mut *connection, By::Id(pending.from_call_id), false).await?;
    let to = store::call(connection, By::Id(pending.to_call_id), true).await?;
    match (from, to) {
        (Some(from), Some(to)) => Ok((pending, from, to)),
        _ => Err(ApiError::not_found()),
    }
}

/// Asks for the join of the destination of a move. Only the device that is in the call of
/// the source gets it.
#[utoipa::path(
    post,
    path = "/api/v1/call/moves/{move_id}/join",
    tag = "calls",
    params(("move_id" = CallMoveId, Path)),
    security(("session" = [])),
    responses((status = 200, body = CallJoin), ApiError)
)]
pub async fn join_move(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(move_id): ApiPath<CallMoveId>,
) -> ApiResult<Json<CallJoin>> {
    let now = app.now();
    proof::limit(&app.calls.actions, bearer.0, now)?;
    let entered = transact!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        access.require(Request::OwnAccount)?;
        let (pending, _, to) = own_move(connection, &access, move_id).await?;
        if pending.expires_at <= now || !may_be_at(connection, &access, &to.place).await? {
            return Err(ApiError::not_found());
        }
        app.calls.admitting()?;
        app.calls.admitting_into(&to.room)?;
        app.calls.mark(to.room.as_str());
        Ok(entry::Entered {
            publishing: store::publishing_of(connection, &to, access.account_id).await?,
            entry_requests: Vec::new(),
            call: store::view(connection, &to).await?,
            key: to.key(),
            room: to.room,
            identity: pending.identity,
        })
    })?;
    let media = app.calls.admitting()?.clone();
    entered.join(&app, media.as_ref(), now).map(Json)
}

/// Reports that the device could not join the destination of a move. Nothing changes: the
/// person stays in the source, and the person who moved is told. Another session of the
/// account gets `not_in_call`.
#[utoipa::path(
    post,
    path = "/api/v1/call/moves/{move_id}/failure",
    tag = "calls",
    params(("move_id" = CallMoveId, Path)),
    security(("session" = [])),
    responses((status = 204), ApiError)
)]
pub async fn fail_move(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(move_id): ApiPath<CallMoveId>,
) -> ApiResult<StatusCode> {
    let now = app.now();
    proof::limit(&app.calls.actions, bearer.0, now)?;
    transact!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        access.require(Request::OwnAccount)?;
        let (pending, _, to) = own_move(connection, &access, move_id).await?;
        app.calls.mark(to.room.as_str());
        store::fail_move(connection, now, &pending, MoveOutcome::Refused).await
    })?;
    Ok(StatusCode::NO_CONTENT)
}

/// Turns meeting mode on in the voice channel that the caller is in. The caller shares a
/// screen that the media server lists. Every other microphone is blocked.
#[utoipa::path(
    put,
    path = "/api/v1/channels/{channel_id}/call/meeting",
    tag = "calls",
    params(("channel_id" = ChannelId, Path)),
    security(("session" = [])),
    responses((status = 204), ApiError)
)]
pub async fn start_meeting(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(channel_id): ApiPath<ChannelId>,
) -> ApiResult<StatusCode> {
    let now = app.now();
    proof::limit(&app.calls.actions, bearer.0, now)?;
    transact!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        let voice = voice(connection, &access, channel_id).await?;
        access.require(Request::Channel {
            scope: &voice.scope,
            permission: WorkspacePermission::RunMeetingMode,
        })?;
        let (call, member) = inside(connection, &access, &voice).await?;
        match &call.meeting {
            Some(meeting) if meeting.presenter == access.account_id => return Ok(()),
            Some(_) => {
                return Err(ApiError::conflict(
                    "another person runs meeting mode in this voice channel",
                ));
            }
            None => {}
        }
        if member.share_seen_at.is_none() || member.share_missing_since.is_some() {
            return Err(ApiError::conflict(
                "meeting mode is turned on by a person whose screen share the media server \
                 lists",
            ));
        }
        let blocked: bool = sqlx::query_scalar(
            "select exists (
                 select 1 from call_microphone_blocks where call_id = $1 and account_id = $2)",
        )
        .bind(call.id)
        .bind(access.account_id)
        .fetch_one(&mut *connection)
        .await?;
        if blocked {
            return Err(ApiError::conflict(
                "a person whose microphone is blocked does not run meeting mode",
            ));
        }
        app.calls.mark(call.room.as_str());
        store::start_meeting(connection, now, &call, access.account_id).await
    })?;
    Ok(StatusCode::NO_CONTENT)
}

/// Turns meeting mode off. Only the person who runs it does this, from inside the call. Every
/// microphone block in the voice channel is removed.
#[utoipa::path(
    delete,
    path = "/api/v1/channels/{channel_id}/call/meeting",
    tag = "calls",
    params(("channel_id" = ChannelId, Path)),
    security(("session" = [])),
    responses((status = 204), ApiError)
)]
pub async fn end_meeting(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(channel_id): ApiPath<ChannelId>,
) -> ApiResult<StatusCode> {
    let now = app.now();
    proof::limit(&app.calls.actions, bearer.0, now)?;
    transact!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        let voice = voice(connection, &access, channel_id).await?;
        access.require(Request::Channel {
            scope: &voice.scope,
            permission: WorkspacePermission::RunMeetingMode,
        })?;
        let (call, _) = inside(connection, &access, &voice).await?;
        match &call.meeting {
            None => Ok(()),
            Some(meeting) if meeting.presenter == access.account_id => {
                app.calls.mark(call.room.as_str());
                store::end_meeting(connection, now, &call, MeetingEnd::TurnedOff).await
            }
            Some(_) => Err(ApiError::new(
                junto_protocol::error::ErrorCode::Forbidden,
                "only the person who runs meeting mode turns it off",
            )),
        }
    })?;
    Ok(StatusCode::NO_CONTENT)
}

/// The block of a moderator in a call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Block {
    Microphone,
    Sound,
}

impl Block {
    const fn permission(self) -> WorkspacePermission {
        match self {
            Self::Microphone => WorkspacePermission::MuteMembers,
            Self::Sound => WorkspacePermission::DeafenMembers,
        }
    }
}

async fn set_block(
    app: &AppState,
    bearer: &Bearer,
    channel_id: ChannelId,
    account_id: AccountId,
    block: Block,
) -> ApiResult<StatusCode> {
    let now = app.now();
    proof::limit(&app.calls.actions, bearer.0, now)?;
    transact!(app, |connection| {
        let access = Access::open(connection, bearer, now).await?;
        let voice = voice(connection, &access, channel_id).await?;
        access.require(Request::Channel {
            scope: &voice.scope,
            permission: block.permission(),
        })?;
        let (call, _) = member_in(connection, &access, &voice, account_id).await?;
        let set =
            match block {
                Block::Microphone => {
                    sqlx::query(
                        "insert into call_microphone_blocks
                         (call_id, account_id, blocked_by, reason, blocked_at)
                     values ($1, $2, $3, 'moderator', $4)
                     on conflict (call_id, account_id) do update
                         set blocked_by = excluded.blocked_by, reason = excluded.reason,
                             blocked_at = excluded.blocked_at
                         where call_microphone_blocks.reason <> 'moderator'",
                    )
                    .bind(call.id)
                    .bind(account_id)
                    .bind(access.account_id)
                    .bind(now)
                    .execute(&mut *connection)
                    .await?
                }
                Block::Sound => sqlx::query(
                    "insert into call_sound_blocks (call_id, account_id, blocked_by, blocked_at)
                     values ($1, $2, $3, $4)
                     on conflict (call_id, account_id) do nothing",
                )
                .bind(call.id)
                .bind(account_id)
                .bind(access.account_id)
                .bind(now)
                .execute(&mut *connection)
                .await?,
            };
        if set.rows_affected() == 0 {
            return Ok(());
        }
        app.calls.mark(call.room.as_str());
        let blocked = match block {
            Block::Microphone => AuditEvent::CallMicrophoneBlocked { account_id },
            Block::Sound => AuditEvent::CallSoundBlocked { account_id },
        };
        let actor = Some(access.account_id);
        audit::record(connection, now, actor, voice.about(), &blocked).await?;
        store::announce_participant(connection, now, &call, account_id).await
    })?;
    Ok(StatusCode::NO_CONTENT)
}

async fn remove_block(
    app: &AppState,
    bearer: &Bearer,
    channel_id: ChannelId,
    account_id: AccountId,
    block: Block,
) -> ApiResult<StatusCode> {
    let now = app.now();
    proof::limit(&app.calls.actions, bearer.0, now)?;
    transact!(app, |connection| {
        let access = Access::open(connection, bearer, now).await?;
        let voice = voice(connection, &access, channel_id).await?;
        let call = store::call(connection, By::Place(voice.place), true).await?;
        if voice.record.channel.hides_occupants {
            let caller = store::member_of(connection, access.account_id, false).await?;
            let is_inside = match (&call, caller) {
                (Some(call), Some(caller)) => caller.call_id == call.id,
                _ => false,
            };
            if !is_inside {
                return Err(not_in_call());
            }
        }
        let presents = call.as_ref().is_some_and(|call| {
            block == Block::Microphone
                && call
                    .meeting
                    .as_ref()
                    .is_some_and(|meeting| meeting.presenter == access.account_id)
        });
        if !presents {
            access.require(Request::Channel {
                scope: &voice.scope,
                permission: block.permission(),
            })?;
        }
        let call = call.ok_or_else(ApiError::not_found)?;
        let table = match block {
            Block::Microphone => "call_microphone_blocks",
            Block::Sound => "call_sound_blocks",
        };
        let removed = sqlx::query(AssertSqlSafe(format!(
            "delete from {table} where call_id = $1 and account_id = $2"
        )))
        .bind(call.id)
        .bind(account_id)
        .execute(&mut *connection)
        .await?;
        if removed.rows_affected() == 0 {
            return Err(ApiError::not_found());
        }
        app.calls.mark(call.room.as_str());
        let unblocked = match block {
            Block::Microphone => AuditEvent::CallMicrophoneUnblocked { account_id },
            Block::Sound => AuditEvent::CallSoundUnblocked { account_id },
        };
        let actor = Some(access.account_id);
        audit::record(connection, now, actor, voice.about(), &unblocked).await?;
        store::announce_participant(connection, now, &call, account_id).await
    })?;
    Ok(StatusCode::NO_CONTENT)
}

/// Blocks the microphone of a person in the call of a voice channel. The media server stops
/// accepting sound of the person.
#[utoipa::path(
    put,
    path = "/api/v1/channels/{channel_id}/call/participants/{account_id}/microphone-block",
    tag = "calls",
    params(("channel_id" = ChannelId, Path), ("account_id" = AccountId, Path)),
    security(("session" = [])),
    responses((status = 204), ApiError)
)]
pub async fn block_microphone(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath((channel_id, account_id)): ApiPath<(ChannelId, AccountId)>,
) -> ApiResult<StatusCode> {
    set_block(&app, &bearer, channel_id, account_id, Block::Microphone).await
}

/// Removes the block of a person's microphone: a moderator, or the person who runs meeting
/// mode. The microphone returns to the person's own setting.
#[utoipa::path(
    delete,
    path = "/api/v1/channels/{channel_id}/call/participants/{account_id}/microphone-block",
    tag = "calls",
    params(("channel_id" = ChannelId, Path), ("account_id" = AccountId, Path)),
    security(("session" = [])),
    responses((status = 204), ApiError)
)]
pub async fn unblock_microphone(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath((channel_id, account_id)): ApiPath<(ChannelId, AccountId)>,
) -> ApiResult<StatusCode> {
    remove_block(&app, &bearer, channel_id, account_id, Block::Microphone).await
}

/// Blocks the sound of a person in the call of a voice channel. The client of the person
/// applies the block.
#[utoipa::path(
    put,
    path = "/api/v1/channels/{channel_id}/call/participants/{account_id}/sound-block",
    tag = "calls",
    params(("channel_id" = ChannelId, Path), ("account_id" = AccountId, Path)),
    security(("session" = [])),
    responses((status = 204), ApiError)
)]
pub async fn block_sound(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath((channel_id, account_id)): ApiPath<(ChannelId, AccountId)>,
) -> ApiResult<StatusCode> {
    set_block(&app, &bearer, channel_id, account_id, Block::Sound).await
}

/// Removes the block of a person's sound.
#[utoipa::path(
    delete,
    path = "/api/v1/channels/{channel_id}/call/participants/{account_id}/sound-block",
    tag = "calls",
    params(("channel_id" = ChannelId, Path), ("account_id" = AccountId, Path)),
    security(("session" = [])),
    responses((status = 204), ApiError)
)]
pub async fn unblock_sound(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath((channel_id, account_id)): ApiPath<(ChannelId, AccountId)>,
) -> ApiResult<StatusCode> {
    remove_block(&app, &bearer, channel_id, account_id, Block::Sound).await
}

/// Removes a person from the call of a voice channel.
#[utoipa::path(
    delete,
    path = "/api/v1/channels/{channel_id}/call/participants/{account_id}",
    tag = "calls",
    params(("channel_id" = ChannelId, Path), ("account_id" = AccountId, Path)),
    security(("session" = [])),
    responses((status = 204), ApiError)
)]
pub async fn disconnect_participant(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath((channel_id, account_id)): ApiPath<(ChannelId, AccountId)>,
) -> ApiResult<StatusCode> {
    let now = app.now();
    proof::limit(&app.calls.actions, bearer.0, now)?;
    transact!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        let voice = voice(connection, &access, channel_id).await?;
        access.require(Request::Channel {
            scope: &voice.scope,
            permission: WorkspacePermission::DisconnectMembers,
        })?;
        let (call, member) = member_in(connection, &access, &voice, account_id).await?;
        let disconnected = LeaveReason::Disconnected;
        app.calls.mark(call.room.as_str());
        store::remove_member(connection, now, &call, &member, disconnected).await?;
        let removed = AuditEvent::CallMemberDisconnected { account_id };
        let actor = Some(access.account_id);
        audit::record(connection, now, actor, voice.about(), &removed).await?;
        store::end_if_empty(connection, now, call.id).await?;
        Ok(())
    })?;
    app.streams.calls().leave(account_id);
    Ok(StatusCode::NO_CONTENT)
}

/// The other participant of the direct message of the caller with `account_id`: an active
/// account that the caller may reach (ADR 0307).
async fn direct_place(
    connection: &mut PgConnection,
    access: &Access,
    account_id: AccountId,
) -> ApiResult<CallPlace> {
    if account_id == access.account_id {
        return Err(ApiError::invalid("a direct message has two participants"));
    }
    if !crate::store::may_reach(connection, access, account_id).await? {
        return Err(ApiError::not_found());
    }
    access.require(Request::DirectMessage(Participation::Participant))?;
    Ok(CallPlace::direct(access.account_id, account_id))
}

/// Asks for the place of the caller in the direct-message call with another account. When
/// that account is not in the call, it is invited: the invitation reaches its devices on
/// every connection level. A direct-message call has no lock, no limit, no meeting mode and
/// no recording.
#[utoipa::path(
    post,
    path = "/api/v1/direct-calls/{account_id}/join",
    tag = "calls",
    params(("account_id" = AccountId, Path)),
    security(("session" = [])),
    responses((status = 200, body = CallJoin), ApiError)
)]
pub async fn join_direct_call(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(account_id): ApiPath<AccountId>,
) -> ApiResult<Json<CallJoin>> {
    let now = app.now();
    proof::limit(&app.calls.actions, bearer.0, now)?;
    let fresh = Fresh::new(&app)?;
    let (entered, caller) = transact!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        let place = direct_place(connection, &access, account_id).await?;
        app.calls.admitting()?;
        let none = Pass::None;
        let entered =
            entry::enter(connection, &app, now, &access, place, None, &fresh, none).await?;
        Ok((entered, access.account_id))
    })?;
    let media = app.calls.admitting()?.clone();
    app.streams.calls().enter(caller, entered.key);
    entered.join(&app, media.as_ref(), now).map(Json)
}

/// Declines the call of the other participant of a direct message.
#[utoipa::path(
    post,
    path = "/api/v1/direct-calls/{account_id}/refusal",
    tag = "calls",
    params(("account_id" = AccountId, Path)),
    security(("session" = [])),
    responses((status = 204), ApiError)
)]
pub async fn refuse_direct_call(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(account_id): ApiPath<AccountId>,
) -> ApiResult<StatusCode> {
    let now = app.now();
    proof::limit(&app.calls.actions, bearer.0, now)?;
    transact!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        let place = direct_place(connection, &access, account_id).await?;
        let call = store::call(connection, By::Place(place), true)
            .await?
            .ok_or_else(ApiError::not_found)?;
        let own = Some(access.account_id.as_uuid());
        let filter = "account_id = $2 and $3::timestamptz is null";
        let declined = InvitationOutcome::Declined;
        let resolved =
            store::resolve_invitations(connection, now, &call, filter, own, None, declined).await?;
        if resolved == 0 {
            return Err(ApiError::not_found());
        }
        Ok(())
    })?;
    Ok(StatusCode::NO_CONTENT)
}

/// The other account of a direct-message conversation of the caller: an account whose
/// profile the caller may see, or one that may call the caller (ADR 0307). Another account
/// gets the answer for an absent one.
async fn conversation_with(
    connection: &mut PgConnection,
    access: &Access,
    account_id: AccountId,
) -> ApiResult<()> {
    if account_id == access.account_id {
        return Err(ApiError::invalid("a direct message has two participants"));
    }
    let exists: bool = sqlx::query_scalar("select exists (select 1 from accounts where id = $1)")
        .bind(account_id)
        .fetch_one(&mut *connection)
        .await?;
    let acquaintance = crate::store::acquaintance(connection, access, account_id).await?;
    let seen = exists && access.allows(Request::ViewAccount(acquaintance));
    if !seen && !crate::store::may_reach(connection, access, account_id).await? {
        return Err(ApiError::not_found());
    }
    Ok(())
}

/// Mutes the direct-message conversation of the caller with another account, until a time
/// or until the mute is turned off. While it lasts, a call of that account invites the
/// caller's account no more; the other account is not told (ADR 0515).
#[utoipa::path(
    put,
    path = "/api/v1/direct-messages/{account_id}/mute",
    tag = "notifications",
    params(("account_id" = AccountId, Path)),
    request_body = MuteDirectConversation,
    security(("session" = [])),
    responses((status = 204), ApiError)
)]
pub async fn mute_direct_conversation(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(account_id): ApiPath<AccountId>,
    ApiJson(body): ApiJson<MuteDirectConversation>,
) -> ApiResult<StatusCode> {
    let now = app.now();
    proof::limit(&app.calls.actions, bearer.0, now)?;
    if body.until.is_some_and(|until| until <= now) {
        return Err(ApiError::invalid("until is not in the future"));
    }
    transact!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        access.require(Request::OwnAccount)?;
        conversation_with(connection, &access, account_id).await?;
        sqlx::query(
            "insert into direct_mutes (account_id, other_account_id, muted_at, muted_until)
             values ($1, $2, $3, $4)
             on conflict (account_id, other_account_id)
             do update set muted_at = excluded.muted_at, muted_until = excluded.muted_until",
        )
        .bind(access.account_id)
        .bind(account_id)
        .bind(now)
        .bind(body.until)
        .execute(&mut *connection)
        .await?;
        let set = Event::DirectMuteSet {
            mute: DirectMute {
                account_id,
                until: body.until,
            },
        };
        events::tell_account(connection, now, access.account_id, &set).await
    })?;
    Ok(StatusCode::NO_CONTENT)
}

/// Turns the mute of a direct-message conversation of the caller off.
#[utoipa::path(
    delete,
    path = "/api/v1/direct-messages/{account_id}/mute",
    tag = "notifications",
    params(("account_id" = AccountId, Path)),
    security(("session" = [])),
    responses((status = 204), ApiError)
)]
pub async fn unmute_direct_conversation(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(account_id): ApiPath<AccountId>,
) -> ApiResult<StatusCode> {
    let now = app.now();
    proof::limit(&app.calls.actions, bearer.0, now)?;
    transact!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        access.require(Request::OwnAccount)?;
        conversation_with(connection, &access, account_id).await?;
        let removed =
            sqlx::query("delete from direct_mutes where account_id = $1 and other_account_id = $2")
                .bind(access.account_id)
                .bind(account_id)
                .execute(&mut *connection)
                .await?;
        if removed.rows_affected() > 0 {
            let removed = Event::DirectMuteRemoved { account_id };
            events::tell_account(connection, now, access.account_id, &removed).await?;
        }
        Ok(())
    })?;
    Ok(StatusCode::NO_CONTENT)
}
