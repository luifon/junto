//! Letting an account into a call and taking it out (ADR 0510).

use junto_protocol::call::{
    Call, CallJoin, CallPlace, EntryOutcome, EntryRequest, InvitationOutcome, LeaveReason,
};
use junto_protocol::error::ErrorCode;
use junto_protocol::events::Event;
use junto_protocol::ids::{AccountId, CallId, CallInvitationId};
use sqlx::PgConnection;
use time::OffsetDateTime;

use crate::access::Access;
use crate::calls::store::{self, By, CallRow, Seat};
use crate::error::{ApiError, ApiResult};
use crate::events;
use crate::events::hub::CallKey;
use crate::media::{self, JoinGrant, MediaServer, Publishing, RoomName};
use crate::state::AppState;

/// The random values of one request to enter a call. They are made before the transaction,
/// which can run several times (ADR 0009).
pub struct Fresh {
    pub call_id: CallId,
    pub room: RoomName,
    seat: String,
    pub invitation_id: CallInvitationId,
}

impl Fresh {
    pub fn new(app: &AppState) -> ApiResult<Self> {
        Ok(Self {
            call_id: CallId::generate(),
            room: RoomName::generate(app.calls.namespace())?,
            seat: media::seat()?,
            invitation_id: CallInvitationId::generate(),
        })
    }

    /// The identity of `account_id` on the new seat.
    pub fn identity(&self, account_id: AccountId) -> String {
        media::identity(account_id, &self.seat)
    }
}

/// What lets an account pass the lock and the user limit of a voice channel. Nothing lets
/// it pass a permission (ADR 0504).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pass {
    /// Only an accepted request to enter.
    None,
    /// An invitation of a person inside.
    Invitation,
}

/// An account that the transaction of a request let into a call.
pub struct Entered {
    pub call: Call,
    pub key: CallKey,
    pub room: RoomName,
    pub identity: String,
    /// The display name of the account at the time of the entry.
    pub name: String,
    pub publishing: Publishing,
    pub entry_requests: Vec<EntryRequest>,
}

impl Entered {
    /// Signs the join token and tells the event stream and the admission check. It runs
    /// after the commit of the transaction that made `self`.
    pub fn join(
        self,
        app: &AppState,
        media: &dyn MediaServer,
        now: OffsetDateTime,
    ) -> ApiResult<CallJoin> {
        let grant = JoinGrant {
            room: &self.room,
            identity: &self.identity,
            name: &self.name,
            publishing: &self.publishing,
            now,
            lifetime: app.config.limits.calls.token_lifetime,
        };
        let signed = media
            .join_token(&grant)
            .map_err(|error| ApiError::internal(&error))?;
        Ok(CallJoin {
            call: self.call,
            media_url: media.client_url().to_owned(),
            token: signed.token,
            token_expires_at: signed.expires_at,
            identity: self.identity,
            entry_requests: self.entry_requests,
        })
    }
}

pub fn locked() -> ApiError {
    ApiError::new(
        ErrorCode::CallLocked,
        "the voice channel is locked; a person inside can let you in",
    )
}

pub fn full() -> ApiError {
    ApiError::new(
        ErrorCode::CallFull,
        "the voice channel holds as many people as its limit allows",
    )
}

pub fn not_in_call() -> ApiError {
    ApiError::new(ErrorCode::NotInCall, "the caller is not in this call")
}

/// Lets the caller into the call at `place`. The caller must already have passed the
/// permission check for the place.
///
/// An account is in one call at a time, from one device. A membership that the account has
/// elsewhere ends here; within the same call the account keeps its place and only the seat
/// changes, so the earlier device is no longer allowed in the room. A seat belongs to one
/// session: only the session that got it gets a token for it again, and another session of
/// the same device gets a new seat.
#[allow(clippy::too_many_arguments)]
pub async fn enter(
    connection: &mut PgConnection,
    app: &AppState,
    now: OffsetDateTime,
    access: &Access,
    place: CallPlace,
    user_limit: Option<u32>,
    fresh: &Fresh,
    pass: Pass,
) -> ApiResult<Entered> {
    let limits = &app.config.limits.calls;
    let usable_until = now + limits.token_lifetime + limits.token_tolerance;
    let call =
        store::call_for_entry(&mut *connection, now, place, fresh.call_id, &fresh.room).await?;
    app.calls.admitting_into(&call.room)?;
    app.calls.mark(call.room.as_str());
    let existing = store::member_of(&mut *connection, access.account_id, true).await?;

    let mut identity = fresh.identity(access.account_id);
    let mut left = None;
    let mut keeps_place = false;
    let mut seated = false;
    match existing {
        Some(member)
            if member.call_id == call.id
                && member.device_id == access.device_id
                && member.session_id == access.session_id
                && (member.connected_at.is_some() || now < member.usable_until) =>
        {
            identity = member.identity;
            seated = true;
        }
        Some(member)
            if member.call_id == call.id
                && member.device_id == access.device_id
                && member.session_id == access.session_id =>
        {
            // The seat never connected and its time has ended. The request is a new entry:
            // a repeated request does not keep a seat that nobody sees (ADR 0517).
            store::abandon_move_of(&mut *connection, now, access.account_id).await?;
            let ended = LeaveReason::NeverConnected;
            store::take_out(&mut *connection, now, &call, &member, ended).await?;
            left = Some(ended);
        }
        Some(member) if member.call_id == call.id => {
            store::abandon_move_of(&mut *connection, now, access.account_id).await?;
            store::take_out(&mut *connection, now, &call, &member, LeaveReason::Switched).await?;
            left = Some(LeaveReason::Switched);
            keeps_place = true;
        }
        Some(member) => {
            store::abandon_move_of(&mut *connection, now, access.account_id).await?;
            if let Some(other) = store::call(&mut *connection, By::Id(member.call_id), true).await?
            {
                let switched = LeaveReason::Switched;
                app.calls.mark(other.room.as_str());
                store::take_out(&mut *connection, now, &other, &member, switched).await?;
                store::end_if_empty(&mut *connection, now, other.id).await?;
            }
            left = Some(LeaveReason::Switched);
        }
        None => {}
    }

    if !seated {
        if !keeps_place && matches!(place, CallPlace::Channel { .. }) {
            let occupied = store::occupancy(&mut *connection, call.id).await?;
            let is_full = user_limit.is_some_and(|limit| occupied >= i64::from(limit));
            if (call.lock.is_some() || is_full) && pass == Pass::None {
                let accepted = sqlx::query(
                    "delete from call_entry_requests
                     where call_id = $1 and account_id = $2
                       and accepted_by is not null and admits_until > $3",
                )
                .bind(call.id)
                .bind(access.account_id)
                .bind(now)
                .execute(&mut *connection)
                .await?;
                if accepted.rows_affected() == 0 {
                    return Err(if call.lock.is_some() {
                        locked()
                    } else {
                        full()
                    });
                }
            }
        }
        let seat = Seat {
            account_id: access.account_id,
            device_id: access.device_id,
            session_id: access.session_id,
            identity: &identity,
        };
        store::admit(&mut *connection, now, &call, seat, usable_until, false).await?;
    }
    settle_what_waited(&mut *connection, now, &call, access.account_id).await?;
    if !seated
        && let CallPlace::Direct {
            low_account_id,
            high_account_id,
        } = place
    {
        let other = if low_account_id == access.account_id {
            high_account_id
        } else {
            low_account_id
        };
        ring(
            &mut *connection,
            app,
            now,
            &call,
            access.account_id,
            other,
            fresh,
        )
        .await?;
    }

    if !seated {
        let own = Event::OwnCallChanged {
            call: Some(junto_protocol::call::OwnCall {
                hides_occupants: Some(call.hides_occupants),
                call_id: call.id,
                place: call.place,
                device_id: access.device_id,
                identity: identity.clone(),
            }),
            left,
        };
        events::tell_account(&mut *connection, now, access.account_id, &own).await?;
    }

    Ok(Entered {
        publishing: store::publishing_at_join(&mut *connection, &call, access.account_id).await?,
        entry_requests: store::entry_requests(&mut *connection, now, &call, access.account_id)
            .await?,
        call: store::view_for(&mut *connection, &call, access.account_id).await?,
        key: call.key(),
        name: store::display_name(&mut *connection, access.account_id).await?,
        room: call.room,
        identity,
    })
}

/// The account is in the call: an invitation of it into this call is accepted, and a
/// request of it to enter is no longer needed.
async fn settle_what_waited(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    call: &CallRow,
    account_id: AccountId,
) -> ApiResult<()> {
    let own = Some(account_id.as_uuid());
    let filter = "account_id = $2 and $3::timestamptz is null";
    let accepted = InvitationOutcome::Accepted;
    let invited =
        store::resolve_invitations(&mut *connection, now, call, filter, own, None, accepted)
            .await?;
    if invited > 0 {
        let by_invitation = store::Note::JoinedByInvitation;
        store::note(&mut *connection, now, call, by_invitation, Some(account_id)).await?;
    }
    let unanswered = "account_id = $2 and accepted_by is null and $3::timestamptz is null";
    let cancelled = EntryOutcome::Cancelled;
    store::resolve_entries(
        &mut *connection,
        now,
        call,
        unanswered,
        own,
        None,
        cancelled,
    )
    .await?;
    sqlx::query("delete from call_entry_requests where call_id = $1 and account_id = $2")
        .bind(call.id)
        .bind(account_id)
        .execute(connection)
        .await?;
    Ok(())
}

/// Invites the other participant of a direct message into the call, unless that account is
/// in the call or has an invitation that waits. For an account that has muted the
/// conversation or is in its quiet time the invitation is silent: that account is told
/// nothing, and the caller gets what every ring gives, so that the caller cannot tell the
/// two cases apart. The rings of the caller for that account are limited (ADR 0515).
async fn ring(
    connection: &mut PgConnection,
    app: &AppState,
    now: OffsetDateTime,
    call: &CallRow,
    caller: AccountId,
    other: AccountId,
    fresh: &Fresh,
) -> ApiResult<()> {
    let reached: bool = sqlx::query_scalar(
        "select exists (select 1 from call_members where call_id = $1 and account_id = $2)
             or exists (select 1 from call_invitations where call_id = $1 and account_id = $2)",
    )
    .bind(call.id)
    .bind(other)
    .fetch_one(&mut *connection)
    .await?;
    if reached {
        return Ok(());
    }
    let silent = store::has_muted(&mut *connection, now, other, caller).await?
        || crate::messages::notifications::quiet(&mut *connection, other, now).await?;
    let limits = &app.config.limits.calls;
    store::count_ring(
        &mut *connection,
        now,
        limits,
        fresh.invitation_id,
        caller,
        other,
    )
    .await?;
    let expires_at = now + limits.invitation_lifetime;
    sqlx::query(
        "insert into call_invitations
             (id, call_id, account_id, invited_by, created_at, expires_at, silent)
         values ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(fresh.invitation_id)
    .bind(call.id)
    .bind(other)
    .bind(caller)
    .bind(now)
    .bind(expires_at)
    .bind(silent)
    .execute(&mut *connection)
    .await
    .map_err(ApiError::concurrent)?;
    let row = (fresh.invitation_id, other, caller, now, expires_at, silent);
    let waiting = store::invitation(call, row);
    let invited = Event::CallInvited {
        invitation: waiting.invitation.clone(),
    };
    if waiting.silent {
        events::log::record(
            connection,
            now,
            &events::audience::Audience::Suppressed,
            &invited,
        )
        .await?;
    }
    for account_id in waiting.told() {
        events::tell_account(&mut *connection, now, account_id, &invited).await?;
    }
    // A later ring of the same call, after a new seat of an account that waits alone,
    // starts no second call.
    if call.direct_caller.is_none() {
        store::direct_call_rung(connection, now, call, caller, other).await?;
    }
    Ok(())
}

/// Takes an account out of the call that it is in. Returns the call that it left.
pub async fn leave(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    account_id: AccountId,
    reason: LeaveReason,
) -> ApiResult<Option<CallRow>> {
    let Some(member) = store::member_of(&mut *connection, account_id, true).await? else {
        return Ok(None);
    };
    let Some(call) = store::call(&mut *connection, By::Id(member.call_id), true).await? else {
        return Ok(None);
    };
    store::remove_member(&mut *connection, now, &call, &member, reason).await?;
    store::end_if_empty(connection, now, call.id).await?;
    Ok(Some(call))
}
