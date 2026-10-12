//! Call admission by repeated checking (ADR 0506, ADR 0509).
//!
//! A join token stays usable after its stated end, the media server gives a connected client
//! a longer one, and a removal ends no token. The server therefore compares the participants
//! of every room with its own record again and again, and removes whoever is not allowed.
//! Removal, ban, loss of a permission, the end of a session, the lock and the user limit all
//! take effect here: each of them changes the record, and the check makes the room agree.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};

use junto_authz::{Participation, Request, WorkspacePermission};
use junto_protocol::audit::AuditEvent;
use junto_protocol::call::{CallPlace, EntryOutcome, InvitationOutcome, LeaveReason, MeetingEnd};
use junto_protocol::call::{MoveOutcome, OwnCall};
use junto_protocol::error::ErrorCode;
use junto_protocol::events::Event;
use junto_protocol::ids::{AccountId, CallId, EntryRequestId};
use junto_protocol::structure::ChannelKind;
use sqlx::{AssertSqlSafe, PgConnection};
use time::OffsetDateTime;
use tokio::time::Instant;

use crate::access::Access;
use crate::audit::{self, About};
use crate::calls::Calls;
use crate::calls::store::{
    self, By, CallRow, ENTRY_COLUMNS, INVITATION_COLUMNS, MOVE_COLUMNS, MemberRow, MoveRow, Seat,
};
use crate::error::ApiResult;
use crate::events;
use crate::media::{
    MediaResult, MediaServer, Participant, ParticipantKind, Publishing, RoomName, TrackKind,
    TrackSource,
};
use crate::state::AppState;
use crate::{read, transact};
use futures_util::StreamExt;

/// What one check found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// The server has no media server.
    NoMediaServer,
    /// The media server did not answer. Nothing was compared and nothing changed.
    Unreachable,
    /// The rooms of this check were compared with the record.
    Checked { removed: usize, updated: usize },
}

/// What the media server has to be told after the record of a call was brought up to date.
#[derive(Debug, Default)]
struct Orders {
    removals: Vec<(String, String)>,
    updates: Vec<(String, String, Publishing)>,
    /// The jobs of the recorder in the room that no recording allows there (ADR 0604).
    recorder_stops: Vec<String>,
    /// The record changed in a way that the next check has to carry to the media server.
    again: bool,
}

/// The participants of the recorder in a room that has no recording which is starting or
/// runs: jobs whose start answer the server did not store, or that were declared lost while
/// their recorder lived. The recorder is asked to stop them.
async fn unallowed_recorder_jobs(
    app: &AppState,
    room: &str,
    participants: &[Participant],
) -> Vec<String> {
    let jobs: Vec<String> = participants
        .iter()
        .filter(|participant| participant.kind == ParticipantKind::Recorder)
        .map(|participant| participant.identity.clone())
        .collect();
    if jobs.is_empty() || app.recordings.backend().is_none() {
        return Vec::new();
    }
    let recorded: ApiResult<bool> = async {
        read!(app.background(), |connection| {
            let recorded: bool = sqlx::query_scalar(
                "select exists (
                     select 1 from recordings
                     where room_name = $1 and state in ('starting', 'recording'))",
            )
            .bind(room)
            .fetch_one(connection)
            .await?;
            Ok(recorded)
        })
    }
    .await;
    match recorded {
        Ok(false) => jobs,
        Ok(true) | Err(_) => Vec::new(),
    }
}

/// Whether the account of a member may still be in the call: its session counts, and the
/// permission function allows it to be at the place of the call now.
async fn still_allowed(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    place: &CallPlace,
    member: &MemberRow,
) -> ApiResult<bool> {
    let access = Access::of_session(
        &mut *connection,
        member.account_id,
        member.device_id,
        member.session_id,
        now,
    )
    .await?;
    let Some(access) = access else {
        return Ok(false);
    };
    may_be_at(connection, &access, place).await
}

/// The decision of the permission function for an account and the place of a call.
pub async fn may_be_at(
    connection: &mut PgConnection,
    access: &Access,
    place: &CallPlace,
) -> ApiResult<bool> {
    match *place {
        CallPlace::Channel { channel_id, .. } => {
            match access.channel(connection, channel_id).await {
                Ok((record, scope)) => Ok(record.channel.kind == ChannelKind::Voice
                    && record.archived_at.is_none()
                    && access.allows(Request::Channel {
                        scope: &scope,
                        permission: WorkspacePermission::ViewChannel,
                    })),
                Err(error) if error.code() == ErrorCode::NotFound => Ok(false),
                Err(error) => Err(error),
            }
        }
        CallPlace::Direct {
            low_account_id,
            high_account_id,
        } => {
            let other = match access.account_id {
                own if own == low_account_id => high_account_id,
                own if own == high_account_id => low_account_id,
                _ => return Ok(access.allows(Request::DirectMessage(Participation::Outsider))),
            };
            Ok(
                access.allows(Request::DirectMessage(Participation::Participant))
                    && crate::store::may_reach(connection, access, other).await?,
            )
        }
    }
}

/// Whether accounts may be at the place of a call now, decided once per account in one check.
#[derive(Default)]
struct Sight {
    decided: HashMap<AccountId, bool>,
}

impl Sight {
    async fn of(
        &mut self,
        connection: &mut PgConnection,
        place: &CallPlace,
        account_id: AccountId,
    ) -> ApiResult<bool> {
        if let Some(known) = self.decided.get(&account_id) {
            return Ok(*known);
        }
        let sees = match Access::of_account(&mut *connection, account_id).await? {
            Some(access) => may_be_at(connection, &access, place).await?,
            None => false,
        };
        self.decided.insert(account_id, sees);
        Ok(sees)
    }
}

/// Ends what waits at a call for an account that may no longer be at its place: the
/// invitations that the account got or made and its request to enter. The other side of
/// each learns that it ended. The account itself is added to `lost`: it gets one event that
/// names nothing, and no outcome (ADR 0510).
async fn end_what_lost_its_place(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    call: &CallRow,
    sight: &mut Sight,
    lost: &mut Vec<AccountId>,
) -> ApiResult<()> {
    let mut lose = |account_id: AccountId| {
        if !lost.contains(&account_id) {
            lost.push(account_id);
        }
    };
    if let CallPlace::Direct {
        low_account_id,
        high_account_id,
    } = call.place
        && !sight
            .of(&mut *connection, &call.place, low_account_id)
            .await?
    {
        lose(low_account_id);
        lose(high_account_id);
    }

    let invitations: Vec<store::InvitationRow> = sqlx::query_as(AssertSqlSafe(format!(
        "select {INVITATION_COLUMNS} from call_invitations where call_id = $1 order by id"
    )))
    .bind(call.id)
    .fetch_all(&mut *connection)
    .await?;
    for row in invitations {
        let waiting = store::invitation(call, row);
        let invitation = &waiting.invitation;
        let mut sides = Vec::new();
        for account_id in [invitation.account_id, invitation.invited_by] {
            let sees = sight.of(&mut *connection, &call.place, account_id).await?;
            sides.push((account_id, sees));
        }
        if sides.iter().all(|(_, sees)| *sees) {
            continue;
        }
        sqlx::query("delete from call_invitations where id = $1")
            .bind(invitation.id)
            .execute(&mut *connection)
            .await?;
        let cancelled = Event::CallInvitationResolved {
            invitation_id: invitation.id,
            outcome: InvitationOutcome::Cancelled,
        };
        for (account_id, sees) in sides {
            // The invited account of a silent invitation never learned of it.
            if !waiting.told().any(|told| told == account_id) {
                continue;
            }
            if sees {
                let at = [call.place];
                events::tell_account_at(&mut *connection, now, account_id, &at, &cancelled).await?;
            } else {
                lose(account_id);
            }
        }
    }

    let requests: Vec<(EntryRequestId, AccountId, OffsetDateTime, OffsetDateTime)> =
        sqlx::query_as(AssertSqlSafe(format!(
            "select {ENTRY_COLUMNS} from call_entry_requests where call_id = $1 order by id"
        )))
        .bind(call.id)
        .fetch_all(&mut *connection)
        .await?;
    for row in requests {
        let request = store::entry_request(call, row);
        if sight
            .of(&mut *connection, &call.place, request.account_id)
            .await?
        {
            continue;
        }
        let waited: Option<bool> = sqlx::query_scalar(
            "delete from call_entry_requests where id = $1 returning accepted_by is null",
        )
        .bind(request.id)
        .fetch_optional(&mut *connection)
        .await?;
        if waited == Some(true) && matches!(request.place, CallPlace::Channel { .. }) {
            let cancelled = Event::CallEntryResolved {
                request_id: request.id,
                call_id: request.call_id,
                place: request.place,
                account_id: request.account_id,
                outcome: EntryOutcome::Cancelled,
                by: None,
            };
            events::tell_inside(&mut *connection, now, &cancelled).await?;
        }
        lose(request.account_id);
    }
    Ok(())
}

/// Whether the account that asked for a move may still move people between the two places.
async fn may_still_move(
    connection: &mut PgConnection,
    pending: &MoveRow,
    places: [&CallPlace; 2],
) -> ApiResult<bool> {
    let Some(mover) = Access::of_account(&mut *connection, pending.moved_by).await? else {
        return Ok(false);
    };
    for place in places {
        let CallPlace::Channel { channel_id, .. } = *place else {
            return Ok(false);
        };
        let allowed = match mover.channel(&mut *connection, channel_id).await {
            Ok((_, scope)) => mover.allows(Request::Channel {
                scope: &scope,
                permission: WorkspacePermission::MoveMembers,
            }),
            Err(error) if error.code() == ErrorCode::NotFound => false,
            Err(error) => return Err(error),
        };
        if !allowed {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Completes a move whose identity the media server lists in the room of the destination:
/// the account becomes a member there and stops being one of the source (ADR 0511). The
/// caller has checked that the move has not expired. Returns false, and ends the move
/// without effect, when the moved account or the account that moved may no longer do it.
async fn complete_move(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    destination: &CallRow,
    pending: &MoveRow,
    orders: &mut Orders,
) -> ApiResult<bool> {
    let member = store::member_of(&mut *connection, pending.account_id, true).await?;
    let source = store::call(&mut *connection, By::Id(pending.from_call_id), true).await?;
    let (Some(member), Some(source)) = (member, source) else {
        store::fail_move(connection, now, pending, MoveOutcome::Abandoned).await?;
        return Ok(false);
    };
    let places = [&source.place, &destination.place];
    if member.call_id != source.id
        || !still_allowed(&mut *connection, now, &destination.place, &member).await?
        || !may_still_move(&mut *connection, pending, places).await?
    {
        store::fail_move(connection, now, pending, MoveOutcome::Abandoned).await?;
        return Ok(false);
    }

    sqlx::query("delete from call_moves where id = $1")
        .bind(pending.id)
        .execute(&mut *connection)
        .await?;
    store::take_out(&mut *connection, now, &source, &member, LeaveReason::Moved).await?;
    let seat = Seat {
        account_id: member.account_id,
        device_id: member.device_id,
        session_id: member.session_id,
        identity: &pending.identity,
    };
    store::admit(&mut *connection, now, destination, seat, now, true).await?;
    sqlx::query(
        "update call_members set self_muted = $3, self_deafened = $4
         where call_id = $1 and account_id = $2",
    )
    .bind(destination.id)
    .bind(member.account_id)
    .bind(member.self_muted)
    .bind(member.self_deafened)
    .execute(&mut *connection)
    .await?;
    seen_in_room(&mut *connection, now, destination, member.account_id).await?;

    let own = Event::OwnCallChanged {
        call: Some(OwnCall {
            call_id: destination.id,
            place: destination.place,
            device_id: member.device_id,
            identity: pending.identity.clone(),
        }),
        left: Some(LeaveReason::Moved),
    };
    events::tell_account(&mut *connection, now, member.account_id, &own).await?;
    store::tell_move(&mut *connection, now, pending, MoveOutcome::Completed).await?;
    let (account_id, mover) = (member.account_id, Some(pending.moved_by));
    for (place, entry) in [
        (source.place, AuditEvent::CallMemberMovedOut { account_id }),
        (
            destination.place,
            AuditEvent::CallMemberMovedIn { account_id },
        ),
    ] {
        if let CallPlace::Channel {
            workspace_id,
            channel_id,
        } = place
        {
            let about = About::Channel(workspace_id, channel_id);
            audit::record(&mut *connection, now, mover, about, &entry).await?;
        }
    }
    store::end_if_empty(connection, now, source.id).await?;
    orders
        .removals
        .push((source.room.as_str().to_owned(), member.identity));
    Ok(true)
}

/// Records that the media server lists a member for the first time: the member is in the
/// call for everyone who may see who is inside, and the call has started when it is the
/// first one.
async fn seen_in_room(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    call: &CallRow,
    account_id: junto_protocol::ids::AccountId,
) -> ApiResult<()> {
    let started =
        sqlx::query("update calls set started_at = $2 where id = $1 and started_at is null")
            .bind(call.id)
            .bind(now)
            .execute(&mut *connection)
            .await?;
    if started.rows_affected() > 0 {
        let first = Some(account_id);
        store::note(&mut *connection, now, call, store::Note::CallStarted, first).await?;
        let started = Event::CallStarted {
            call_id: call.id,
            place: call.place,
            started_at: now,
        };
        events::publish(&mut *connection, now, &started).await?;
    }
    for participant in store::participants(&mut *connection, call.id, Some(account_id)).await? {
        let joined = Event::CallParticipantJoined {
            call_id: call.id,
            place: call.place,
            participant,
        };
        events::publish(&mut *connection, now, &joined).await?;
    }
    Ok(())
}

/// Starts the times of absence of one call again. Only time in which the server saw that a
/// person, a screen share or a presenter was absent counts: after a start of the server and
/// after a time in which the room could not be checked, nothing was seen (ADR 0509).
async fn restart_clocks(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    call_id: CallId,
) -> ApiResult<()> {
    sqlx::query(
        "update call_members set absent_since = null, share_missing_since = null
         where call_id = $1 and (absent_since is not null or share_missing_since is not null)",
    )
    .bind(call_id)
    .execute(&mut *connection)
    .await?;
    sqlx::query(
        "update calls set meeting_presenter_gone_since = $2
         where id = $1 and meeting_presenter_gone_since is not null",
    )
    .bind(call_id)
    .bind(now)
    .execute(connection)
    .await?;
    Ok(())
}

/// Brings the record of one call up to date with the participants that the media server
/// listed for its room, in one transaction, and returns what the media server has to be
/// told.
async fn apply(
    connection: &mut PgConnection,
    app: &AppState,
    now: OffsetDateTime,
    call_id: CallId,
    listed: &[Participant],
    after_a_gap: bool,
) -> ApiResult<Orders> {
    let limits = &app.config.limits.calls;
    let mut orders = Orders::default();
    if after_a_gap {
        restart_clocks(&mut *connection, now, call_id).await?;
    }
    let Some(mut call) = store::call(&mut *connection, By::Id(call_id), false).await? else {
        return Ok(orders);
    };
    let room = call.room.as_str().to_owned();
    let present: HashMap<&str, &Participant> = listed
        .iter()
        .filter(|participant| {
            participant.connected && participant.kind != ParticipantKind::Recorder
        })
        .map(|participant| (participant.identity.as_str(), participant))
        .collect();

    let mut allowed: HashSet<String> = HashSet::new();
    let mut presenter_shares = false;
    let mut sight = Sight::default();
    let mut lost: Vec<AccountId> = Vec::new();
    for member in store::members(&mut *connection, call_id).await? {
        if !still_allowed(&mut *connection, now, &call.place, &member).await? {
            let ended = LeaveReason::AccessEnded;
            store::remove_member(&mut *connection, now, &call, &member, ended).await?;
            continue;
        }
        sight.decided.insert(member.account_id, true);
        let Some(participant) = present.get(member.identity.as_str()) else {
            let gone = match (member.connected_at, member.absent_since) {
                (None, _) if now >= member.usable_until => Some(LeaveReason::NeverConnected),
                (None, _) => None,
                (Some(_), Some(since)) if now - since >= limits.absence_grace => {
                    Some(LeaveReason::ConnectionLost)
                }
                (Some(_), Some(_)) => None,
                (Some(_), None) => {
                    sqlx::query(
                        "update call_members set absent_since = $3
                         where call_id = $1 and account_id = $2",
                    )
                    .bind(call_id)
                    .bind(member.account_id)
                    .bind(now)
                    .execute(&mut *connection)
                    .await?;
                    None
                }
            };
            match gone {
                Some(reason) => {
                    store::remove_member(&mut *connection, now, &call, &member, reason).await?;
                }
                None => {
                    allowed.insert(member.identity.clone());
                }
            }
            continue;
        };
        allowed.insert(member.identity.clone());

        if member.connected_at.is_none() {
            sqlx::query(
                "update call_members set connected_at = $3
                 where call_id = $1 and account_id = $2",
            )
            .bind(call_id)
            .bind(member.account_id)
            .bind(now)
            .execute(&mut *connection)
            .await?;
            seen_in_room(&mut *connection, now, &call, member.account_id).await?;
            call.started_at = call.started_at.or(Some(now));
            if matches!(call.place, CallPlace::Direct { .. }) {
                store::direct_call_seen(&mut *connection, now, &call, member.account_id).await?;
            }
        } else if member.absent_since.is_some() {
            sqlx::query(
                "update call_members set absent_since = null
                 where call_id = $1 and account_id = $2",
            )
            .bind(call_id)
            .bind(member.account_id)
            .execute(&mut *connection)
            .await?;
            if matches!(call.place, CallPlace::Direct { .. }) {
                store::direct_call_seen(&mut *connection, now, &call, member.account_id).await?;
            }
        }

        let presents = call
            .meeting
            .as_ref()
            .is_some_and(|meeting| meeting.presenter == member.account_id);
        if member.share_audio.is_some() {
            if participant.publishes(TrackSource::ScreenShare) {
                presenter_shares |= presents;
                if member.share_seen_at.is_none() || member.share_missing_since.is_some() {
                    sqlx::query(
                        "update call_members
                         set share_seen_at = coalesce(share_seen_at, $3),
                             share_missing_since = null
                         where call_id = $1 and account_id = $2",
                    )
                    .bind(call_id)
                    .bind(member.account_id)
                    .bind(now)
                    .execute(&mut *connection)
                    .await?;
                }
            } else {
                let since = member.share_missing_since.unwrap_or(now);
                if now - since >= limits.share_grace && member.share_missing_since.is_some() {
                    sqlx::query(
                        "update call_members
                         set share_audio = null, share_started_at = null, share_seen_at = null,
                             share_missing_since = null
                         where call_id = $1 and account_id = $2",
                    )
                    .bind(call_id)
                    .bind(member.account_id)
                    .execute(&mut *connection)
                    .await?;
                    store::announce_participant(&mut *connection, now, &call, member.account_id)
                        .await?;
                    if presents {
                        let stopped = MeetingEnd::ShareStopped;
                        store::end_meeting(&mut *connection, now, &call, stopped).await?;
                        call.meeting = None;
                    }
                } else if member.share_missing_since.is_none() {
                    sqlx::query(
                        "update call_members set share_missing_since = $3
                         where call_id = $1 and account_id = $2",
                    )
                    .bind(call_id)
                    .bind(member.account_id)
                    .bind(now)
                    .execute(&mut *connection)
                    .await?;
                }
            }
        }

        let wanted = store::publishing_of(&mut *connection, &call, member.account_id).await?;
        if participant.publishing.as_ref() != Some(&wanted) {
            orders
                .updates
                .push((room.clone(), member.identity.clone(), wanted));
        } else if !wanted.allows(TrackSource::Microphone)
            && participant
                .tracks
                .iter()
                .any(|track| track.kind == TrackKind::Audio)
        {
            orders
                .removals
                .push((room.clone(), member.identity.clone()));
        }
    }

    let moves: Vec<MoveRow> = sqlx::query_as(AssertSqlSafe(format!(
        "select {MOVE_COLUMNS} from call_moves where to_call_id = $1 order by created_at, id"
    )))
    .bind(call_id)
    .fetch_all(&mut *connection)
    .await?;
    for pending in moves {
        let expired = now >= pending.expires_at;
        let mut blind = Vec::new();
        for account_id in [pending.account_id, pending.moved_by] {
            if !sight.of(&mut *connection, &call.place, account_id).await? {
                blind.push(account_id);
            }
        }
        if expired || !blind.is_empty() {
            let outcome = if expired {
                MoveOutcome::TimedOut
            } else {
                MoveOutcome::Abandoned
            };
            store::fail_move(&mut *connection, now, &pending, outcome).await?;
            for account_id in blind {
                if !lost.contains(&account_id) {
                    lost.push(account_id);
                }
            }
        } else if let Some(participant) = present.get(pending.identity.as_str())
            && complete_move(&mut *connection, now, &call, &pending, &mut orders).await?
        {
            allowed.insert(pending.identity.clone());
            call.started_at = call.started_at.or(Some(now));
            // The token of a move into a call that hides who is inside allows pictures
            // only (ADR 0517). The account is inside now and gets what it may publish.
            let wanted = store::publishing_of(&mut *connection, &call, pending.account_id).await?;
            if participant.publishing.as_ref() != Some(&wanted) {
                orders
                    .updates
                    .push((room.clone(), pending.identity.clone(), wanted));
            }
        }
    }

    for identity in present.keys() {
        if !allowed.contains(*identity) {
            orders.removals.push((room.clone(), (*identity).to_owned()));
        }
    }

    end_what_lost_its_place(&mut *connection, now, &call, &mut sight, &mut lost).await?;
    for account_id in lost {
        store::tell_access_ended(&mut *connection, now, account_id).await?;
    }

    let due = "expires_at <= $3 and $2::uuid is null";
    let expired = InvitationOutcome::Expired;
    store::resolve_invitations(&mut *connection, now, &call, due, None, Some(now), expired).await?;
    let unanswered = "accepted_by is null and expires_at <= $3 and $2::uuid is null";
    let expired = EntryOutcome::Expired;
    store::resolve_entries(
        &mut *connection,
        now,
        &call,
        unanswered,
        None,
        Some(now),
        expired,
    )
    .await?;
    sqlx::query("delete from call_entry_requests where call_id = $1 and admits_until <= $2")
        .bind(call_id)
        .bind(now)
        .execute(&mut *connection)
        .await?;

    if call.meeting.is_some() {
        let gone_since: Option<OffsetDateTime> =
            sqlx::query_scalar("select meeting_presenter_gone_since from calls where id = $1")
                .bind(call_id)
                .fetch_one(&mut *connection)
                .await?;
        match (presenter_shares, gone_since) {
            (true, Some(_)) => {
                sqlx::query("update calls set meeting_presenter_gone_since = null where id = $1")
                    .bind(call_id)
                    .execute(&mut *connection)
                    .await?;
            }
            (true, None) => {}
            (false, Some(since)) if now - since >= limits.presenter_grace => {
                let left = MeetingEnd::PresenterLeft;
                store::end_meeting(&mut *connection, now, &call, left).await?;
                orders.again = true;
            }
            (false, Some(_)) => {}
            (false, None) => {
                sqlx::query("update calls set meeting_presenter_gone_since = $2 where id = $1")
                    .bind(call_id)
                    .bind(now)
                    .execute(&mut *connection)
                    .await?;
            }
        }
    }

    store::end_if_empty(connection, now, call_id).await?;
    Ok(orders)
}

/// What the check of one room did.
#[derive(Debug, Default, Clone, Copy)]
struct Done {
    removed: usize,
    updated: usize,
}

/// What a room failed at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Failed {
    /// The room could not be observed: its participants or its record could not be read, or
    /// the record could not be brought up to date.
    Observation,
    /// The room was observed, and an action on the media server failed.
    Action,
}

/// Counts a failed check of a room, once per check, and logs the first failure and the one
/// after which the room is reported as failing.
struct Failure<'a> {
    app: &'a AppState,
    room: &'a str,
    counted: AtomicBool,
}

impl Failure<'_> {
    fn record(&self, failed: Failed, what: &str, error: &dyn std::fmt::Display) {
        let observed = failed == Failed::Action;
        if self.counted.swap(true, Ordering::Relaxed) {
            if !observed {
                self.app.calls.unobserved(self.room);
            }
            return;
        }
        let (room, calls) = (self.room, &self.app.calls);
        let times = calls.room_failed(room, observed);
        if times == 1 {
            tracing::warn!(room, %error, "{what}; the room is checked again at the next check");
        } else if times == self.app.config.limits.calls.failing_after {
            tracing::error!(room, times, %error, "{what}; the room keeps failing");
        }
    }

    /// The time limit of the check of the room passed.
    fn took_too_long(&self, failed: Failed) {
        let limit = self.app.config.limits.calls.room_check_time_limit;
        self.record(failed, "the check of a room took too long", &limit);
    }
}

/// Observes one room: asks the media server for its participants and brings the record of
/// its call up to date. Returns what the media server has to be told. `None` when the room
/// could not be observed, which is recorded in `failure`, or is not of this server.
///
/// The times of absence of the call start again when the room was not observed before in
/// `era`, or an observation of it failed since: only then did the server not see what
/// happened in between.
async fn observe(
    app: &AppState,
    media: &dyn MediaServer,
    room: &str,
    listed: Option<bool>,
    era: u64,
    failure: &Failure<'_>,
) -> Option<Orders> {
    let stored = async {
        read!(app.background(), |connection| {
            let call_id: Option<CallId> =
                sqlx::query_scalar("select id from calls where room_name = $1")
                    .bind(room)
                    .fetch_optional(connection)
                    .await?;
            Ok(call_id)
        })
    };
    let call_id: Option<CallId> = match stored.await {
        Ok(call_id) => call_id,
        Err(error) => {
            let code = format!("{:?}", error.code());
            let what = "the record of a room could not be read";
            failure.record(Failed::Observation, what, &code);
            return None;
        }
    };
    if call_id.is_none() && !RoomName::is_of(app.calls.namespace(), room) {
        return None;
    }
    let participants = if listed == Some(false) {
        Vec::new()
    } else {
        match media.list_participants(room).await {
            Ok(participants) => participants,
            Err(error) => {
                let what = "the participants of a room could not be listed";
                failure.record(Failed::Observation, what, &error);
                return None;
            }
        }
    };

    let recorder_stops = unallowed_recorder_jobs(app, room, &participants).await;
    let Some(call_id) = call_id else {
        return Some(Orders {
            removals: participants
                .into_iter()
                .filter(|participant| participant.kind != ParticipantKind::Recorder)
                .map(|participant| (room.to_owned(), participant.identity))
                .collect(),
            recorder_stops,
            ..Orders::default()
        });
    };
    let after_a_gap = !app.calls.continues(room, era);
    let now = app.now();
    let applied: ApiResult<Orders> = async {
        transact!(app.background(), |connection| {
            apply(connection, app, now, call_id, &participants, after_a_gap).await
        })
    }
    .await;
    match applied {
        Ok(orders) => {
            app.calls.observed(room, era);
            Some(Orders {
                recorder_stops,
                ..orders
            })
        }
        Err(error) => {
            let code = format!("{:?}", error.code());
            let what = "the record of a call could not be brought up to date";
            failure.record(Failed::Observation, what, &code);
            None
        }
    }
}

/// Sends the actions of one kind to the media server, one after the other, until
/// `deadline`. An action that fails does not stop the ones after it. Returns the number of
/// actions that succeeded.
async fn send_each<'a, A, F>(
    actions: &'a [A],
    send: impl Fn(&'a A) -> F,
    deadline: Instant,
    failure: &Failure<'_>,
    what: &str,
) -> usize
where
    F: Future<Output = MediaResult<()>>,
{
    let mut done = 0;
    for action in actions {
        match tokio::time::timeout_at(deadline, send(action)).await {
            Ok(Ok(())) => done += 1,
            Ok(Err(error)) => failure.record(Failed::Action, what, &error),
            Err(_) => {
                failure.took_too_long(Failed::Action);
                break;
            }
        }
    }
    done
}

/// Checks one room: observes it, and tells the media server whom to remove and what each
/// participant may publish. `listed` states whether the media server listed the room, when
/// the caller knows. `era` is the time of the media server in which the check started.
///
/// The observation and the actions together have the time limit of one room. The removals
/// and the changes of permissions are sent at the same time, each kind one after the other:
/// removals that are slow do not use the time of the changes of permissions, and the other
/// way round.
///
/// A failure stays in this room. An observation that fails ends the check of the room; an
/// action that fails does not stop the other actions. The next check computes every action
/// again from the record and the room, so one that failed or was not sent is tried again
/// then. A failed action does not change what was observed.
async fn check_room(
    app: &AppState,
    media: &dyn MediaServer,
    room: &str,
    listed: Option<bool>,
    era: u64,
) -> Done {
    let time_limit = app.config.limits.calls.room_check_time_limit;
    let deadline = Instant::now() + time_limit.unsigned_abs();
    let failure = Failure {
        app,
        room,
        counted: AtomicBool::new(false),
    };
    let observing = observe(app, media, room, listed, era, &failure);
    let orders = match tokio::time::timeout_at(deadline, observing).await {
        Ok(Some(orders)) => orders,
        Ok(None) => return Done::default(),
        Err(_) => {
            failure.took_too_long(Failed::Observation);
            return Done::default();
        }
    };

    let removing = send_each(
        &orders.removals,
        |(room, identity)| media.remove_participant(room, identity),
        deadline,
        &failure,
        "a participant could not be removed",
    );
    let updating = send_each(
        &orders.updates,
        |(room, identity, publishing)| media.update_publishing(room, identity, publishing),
        deadline,
        &failure,
        "the permission of a participant could not be set",
    );
    let stopping = async {
        let Some(backend) = app.recordings.backend() else {
            return;
        };
        let stops = orders
            .recorder_stops
            .iter()
            .map(|job_id| backend.recorder.stop(job_id));
        // A stop without an answer is no failure of the room: the job of a lost recorder
        // leaves by itself, and the next check asks again for one that is still there.
        let _ = tokio::time::timeout_at(deadline, futures_util::future::join_all(stops)).await;
    };
    let (removed, updated, ()) = tokio::join!(removing, updating, stopping);
    let every_action_done = removed == orders.removals.len() && updated == orders.updates.len();
    if every_action_done && app.calls.room_served(room) {
        tracing::info!(room, "the room is checked again");
    }
    if orders.again {
        app.calls.mark(room);
        app.calls.poke();
    }
    Done { removed, updated }
}

/// Checks `rooms`, a bounded number of them at one time, each within the time limit of the
/// check of one room.
async fn check_rooms(
    app: &AppState,
    media: &dyn MediaServer,
    rooms: Vec<(String, Option<bool>)>,
) -> Done {
    let era = app.calls.era();
    let one = |(room, listed): (String, Option<bool>)| async move {
        check_room(app, media, &room, listed, era).await
    };
    futures_util::stream::iter(rooms)
        .map(one)
        .buffer_unordered(app.config.limits.calls.concurrent_room_checks.max(1))
        .fold(Done::default(), |total, room| async move {
            Done {
                removed: total.removed + room.removed,
                updated: total.updated + room.updated,
            }
        })
        .await
}

async fn replace_registry(app: &AppState, read_at: u64) {
    let members: ApiResult<Vec<_>> = async {
        read!(app.background(), |connection| {
            store::everyone_in_a_call(connection).await
        })
    }
    .await;
    match members {
        Ok(members) => {
            app.streams
                .replace_calls(read_at, members.into_iter().collect());
        }
        Err(error) => tracing::warn!(code = ?error.code(), "the accounts in calls were not read"),
    }
}

/// Runs one part of the full admission check. The full check takes the rooms in turn: each
/// part looks into the next rooms with a call and the next rooms of this server without one,
/// a bounded number of each, and the part after it continues where this one ended. In each
/// room, whoever is not allowed is removed, and what a participant may publish is set to
/// what the record states (ADR 0509).
///
/// The part asks the media server for its rooms first. While that request fails, the media
/// server is unreachable: the check changes nothing, the server lets nobody into a call, and
/// the next part tries again.
///
/// One part costs one request for the rooms, one statement for the names of the recorded
/// rooms, and for each room that it takes at most one request for the participants, one
/// transaction, and the actions of that room within the time limit of one room.
pub async fn check(app: &AppState) -> ApiResult<Outcome> {
    let Some(media) = app.calls.media().cloned() else {
        return Ok(Outcome::NoMediaServer);
    };
    let _round = app.calls.round_of_a_part();
    let listed: BTreeSet<String> = match media.list_rooms().await {
        Ok(rooms) => rooms.into_iter().map(|room| room.name).collect(),
        Err(error) => {
            if app.calls.set_reachable(false) {
                tracing::warn!(%error, "the media server does not answer; no call can be joined");
            }
            return Ok(Outcome::Unreachable);
        }
    };
    let returned = !app.calls.set_reachable(true);
    if returned {
        tracing::info!("the media server answers; call admission is checked");
    }
    let registry = app.streams.calls().generation();
    let recorded: BTreeSet<String> = read!(app.background(), |connection| {
        let rooms: Vec<String> = sqlx::query_scalar("select room_name from calls")
            .fetch_all(connection)
            .await?;
        Ok(rooms.into_iter().collect())
    })?;
    let ended: BTreeSet<String> = listed
        .iter()
        .filter(|room| !recorded.contains(*room))
        .filter(|room| RoomName::is_of(app.calls.namespace(), room))
        .cloned()
        .collect();

    let limits = &app.config.limits.calls;
    let mut names = app.calls.next_recorded(&recorded, limits.rooms_per_check);
    names.extend(
        app.calls
            .next_ended(&ended, limits.most_ended_rooms_per_check),
    );
    app.calls.unmark(&names);
    let rooms = names
        .into_iter()
        .map(|room| {
            let is_listed = listed.contains(&room);
            (room, Some(is_listed))
        })
        .collect();
    let done = check_rooms(app, media.as_ref(), rooms).await;

    app.calls
        .forget_rooms(|room| recorded.contains(room) || listed.contains(room));
    replace_registry(app, registry).await;
    app.calls.poke();
    Ok(Outcome::Checked {
        removed: done.removed,
        updated: done.updated,
    })
}

/// Checks rooms whose calls a request or a notification changed since their last check: a
/// bounded number of them, and the others at the next call. Without such a room it sends no
/// request to the media server. While the media server is unreachable the rooms stay
/// marked.
pub async fn check_marked(app: &AppState) -> ApiResult<Outcome> {
    let Some(media) = app.calls.media().cloned() else {
        return Ok(Outcome::NoMediaServer);
    };
    let _round = app.calls.round();
    if app.calls.reachable() != Some(true) {
        return Ok(Outcome::Unreachable);
    }
    let rooms = app
        .calls
        .take_marked(app.config.limits.calls.rooms_per_check);
    let mut done = Done::default();
    if !rooms.is_empty() {
        let registry = app.streams.calls().generation();
        let rooms = rooms.into_iter().map(|room| (room, None)).collect();
        done = check_rooms(app, media.as_ref(), rooms).await;
        replace_registry(app, registry).await;
    }
    app.calls.poke();
    Ok(Outcome::Checked {
        removed: done.removed,
        updated: done.updated,
    })
}

/// What the task of the admission check does next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Turn {
    /// A part of the full check.
    Part,
    /// A run of the targeted check.
    Marked,
}

/// Waits until a part of the full check is due or a check is asked for, and returns what
/// the task does then. When both hold at the same time, the kind that did not run last goes
/// first: a run of the targeted check waits for one part at most, and a part for one run at
/// most (ADR 0509).
async fn next_turn(ticks: &mut tokio::time::Interval, calls: &Calls, last: Turn) -> Turn {
    match last {
        Turn::Part => tokio::select! {
            biased;
            () = calls.poked() => calls.asked_for(last),
            _ = ticks.tick() => Turn::Part,
        },
        Turn::Marked => tokio::select! {
            biased;
            _ = ticks.tick() => Turn::Part,
            () = calls.poked() => calls.asked_for(last),
        },
    }
}

/// Runs a part of the full check at the start and at every interval, and the check of the
/// changed rooms whenever a request or a notification of the media server asks for it, one
/// after the other, until the task is dropped. A check that fails is logged and the next one
/// tries again.
pub async fn run(app: AppState) {
    let interval = app.config.limits.calls.check_interval.unsigned_abs();
    let mut ticks = tokio::time::interval(interval);
    ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut turn = Turn::Marked;
    loop {
        turn = next_turn(&mut ticks, &app.calls, turn).await;
        let checked = match turn {
            Turn::Part => check(&app).await,
            Turn::Marked => check_marked(&app).await,
        };
        if let Err(error) = checked {
            tracing::warn!(code = ?error.code(), "the admission check failed");
        }
    }
}

#[cfg(test)]
mod tests {
    use junto_protocol::ids::ServerId;

    use super::*;
    use crate::config::Limits;

    fn due_at_once() -> tokio::time::Interval {
        let mut ticks = tokio::time::interval(std::time::Duration::from_millis(1));
        ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        ticks
    }

    async fn due(ticks: &tokio::time::Interval) {
        tokio::time::sleep(ticks.period() * 2).await;
    }

    /// ADR 0509: while every part of the full check is due when the one before it ends, a
    /// run of the targeted check that is asked for waits for one part at most, and a part
    /// waits for one run at most.
    #[tokio::test]
    async fn a_part_that_is_due_and_a_run_that_is_asked_for_take_turns() {
        let calls = Calls::new(None, ServerId::generate(), &Limits::default());
        let mut ticks = due_at_once();
        for _ in 0..32 {
            for (last, next) in [(Turn::Part, Turn::Marked), (Turn::Marked, Turn::Part)] {
                calls.mark("room");
                calls.poke();
                due(&ticks).await;
                assert_eq!(next_turn(&mut ticks, &calls, last).await, next);
            }
        }
    }

    /// A notification that names no room asks for a part. Marked rooms that wait since the
    /// part before it are checked first, and the part that starts meets the request.
    #[tokio::test]
    async fn a_part_that_a_notification_asks_for_does_not_pass_the_marked_rooms() {
        let calls = Calls::new(None, ServerId::generate(), &Limits::default());
        let mut ticks = tokio::time::interval(std::time::Duration::from_secs(3600));
        ticks.tick().await;

        calls.mark_everything();
        calls.poke();
        assert_eq!(next_turn(&mut ticks, &calls, Turn::Part).await, Turn::Part);
        calls.mark("room");
        calls.poke();
        assert_eq!(
            next_turn(&mut ticks, &calls, Turn::Part).await,
            Turn::Marked
        );
        assert_eq!(calls.take_marked(1), ["room"]);
        assert!(!calls.settled());
        calls.poke();
        assert_eq!(
            next_turn(&mut ticks, &calls, Turn::Marked).await,
            Turn::Part
        );
        drop(calls.round_of_a_part());
        assert!(calls.settled());
    }
}
