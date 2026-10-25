//! The rows of calls and what the routes and the admission check do with them. Every function
//! works inside the transaction of its caller.

use junto_protocol::call::{
    BlockReason, Call, CallInvitation, CallLock, CallMove, CallParticipant, CallPlace, ChannelCall,
    DirectMute, EntryOutcome, EntryRequest, InvitationOutcome, LeaveReason, Meeting, MeetingEnd,
    MicrophoneBlock, MoveOutcome, OwnCall, ScreenShare, ShareAudio, SoundBlock,
};
use junto_protocol::error::ErrorCode;
use junto_protocol::events::Event;
use junto_protocol::ids::{
    AccountId, CallId, CallInvitationId, CallMoveId, ChannelId, DeviceId, EntryRequestId,
    WorkspaceId,
};
use junto_protocol::structure::Channel;
use sqlx::{AssertSqlSafe, PgConnection};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::calls::Calls;
use crate::db::StoredText;
use crate::error::{ApiError, ApiResult};
use crate::events;
use crate::events::hub::CallKey;
use crate::media::{Publishing, RoomName};
use crate::messages::direct_calls::{self, Line};
use crate::store::Sessions;

impl StoredText for ShareAudio {
    fn stored(self) -> &'static str {
        match self {
            Self::Application => "application",
            Self::System => "system",
            Self::None => "none",
        }
    }

    fn from_stored(text: &str) -> Option<Self> {
        [Self::Application, Self::System, Self::None]
            .into_iter()
            .find(|audio| audio.stored() == text)
    }
}

impl StoredText for BlockReason {
    fn stored(self) -> &'static str {
        match self {
            Self::Moderator => "moderator",
            Self::Meeting => "meeting",
        }
    }

    fn from_stored(text: &str) -> Option<Self> {
        [Self::Moderator, Self::Meeting]
            .into_iter()
            .find(|reason| reason.stored() == text)
    }
}

impl StoredText for LeaveReason {
    fn stored(self) -> &'static str {
        match self {
            Self::Left => "left",
            Self::Disconnected => "disconnected",
            Self::Moved => "moved",
            Self::Switched => "switched",
            Self::ConnectionLost => "connection_lost",
            Self::NeverConnected => "never_connected",
            Self::AccessEnded => "access_ended",
        }
    }

    fn from_stored(text: &str) -> Option<Self> {
        [
            Self::Left,
            Self::Disconnected,
            Self::Moved,
            Self::Switched,
            Self::ConnectionLost,
            Self::NeverConnected,
            Self::AccessEnded,
        ]
        .into_iter()
        .find(|reason| reason.stored() == text)
    }
}

/// What a note about the call of a voice channel states (ADR 0501).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Note {
    CallStarted,
    CallEnded,
    ScreenShareStarted,
    EntryRequested,
    JoinedByInvitation,
    MeetingStarted,
    MeetingEnded,
    PersonJoined,
    PersonLeft,
}

impl Note {
    const fn stored(self) -> &'static str {
        match self {
            Self::CallStarted => "call_started",
            Self::CallEnded => "call_ended",
            Self::ScreenShareStarted => "screen_share_started",
            Self::EntryRequested => "entry_requested",
            Self::JoinedByInvitation => "joined_by_invitation",
            Self::MeetingStarted => "meeting_started",
            Self::MeetingEnded => "meeting_ended",
            Self::PersonJoined => "person_joined",
            Self::PersonLeft => "person_left",
        }
    }
}

/// Notes that a person joined a running call, unless the latest note of the person in this
/// call already says that the person is inside: the start of the call, an entry by
/// invitation, or a join from another device or seat.
pub async fn note_joined(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    call: &CallRow,
    account_id: AccountId,
) -> ApiResult<()> {
    let latest: Option<String> = sqlx::query_scalar(
        "select kind from call_notes where call_id = $1 and account_id = $2
             and kind in ('call_started', 'joined_by_invitation', 'person_joined', 'person_left')
         order by occurred_at desc, id desc limit 1",
    )
    .bind(call.id)
    .bind(account_id)
    .fetch_optional(&mut *connection)
    .await?;
    if latest.is_some_and(|kind| kind != Note::PersonLeft.stored()) {
        return Ok(());
    }
    note(connection, now, call, Note::PersonJoined, Some(account_id)).await
}

/// Notes that a person left a call that others are still in. The last person to leave is
/// not noted: the line of the end of the call follows.
pub async fn note_left(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    call: &CallRow,
    account_id: AccountId,
) -> ApiResult<()> {
    let others: bool = sqlx::query_scalar(
        "select exists(select 1 from call_members where call_id = $1 and account_id <> $2
             and connected_at is not null)",
    )
    .bind(call.id)
    .bind(account_id)
    .fetch_one(&mut *connection)
    .await?;
    if !others {
        return Ok(());
    }
    note(connection, now, call, Note::PersonLeft, Some(account_id)).await
}

/// Keeps what happened in the call of a voice channel, for the system lines that the text
/// chat of the channel shows. A direct-message call keeps no notes.
pub async fn note(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    call: &CallRow,
    kind: Note,
    account_id: Option<AccountId>,
) -> ApiResult<()> {
    let CallPlace::Channel {
        workspace_id,
        channel_id,
    } = call.place
    else {
        return Ok(());
    };
    let message_id = junto_protocol::ids::MessageId::generate();
    sqlx::query(
        "insert into call_notes
             (id, occurred_at, workspace_id, channel_id, call_id, kind, account_id)
         values ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(message_id)
    .bind(now)
    .bind(workspace_id)
    .bind(channel_id)
    .bind(call.id)
    .bind(kind.stored())
    .bind(account_id)
    .execute(&mut *connection)
    .await?;
    crate::messages::system(
        connection,
        now,
        channel_id,
        message_id,
        &junto_protocol::message::SystemMessage {
            action: kind.stored().to_owned(),
            actor_id: account_id,
            recording_id: None,
            duration_seconds: None,
            subject_id: None,
        },
        Some(call.id),
    )
    .await?;
    Ok(())
}

#[derive(Debug, Clone)]
pub struct CallRow {
    pub id: CallId,
    pub room: RoomName,
    pub place: CallPlace,
    pub started_at: Option<OffsetDateTime>,
    pub lock: Option<CallLock>,
    pub meeting: Option<Meeting>,
    pub presenter_gone_since: Option<OffsetDateTime>,
    /// The call shows who is inside only to the people inside. The value is the setting of
    /// the voice channel at the creation of the record of the call (ADR 0517).
    pub hides_occupants: bool,
    /// The account whose entry made the first ring of a direct-message call (ADR 0518).
    pub direct_caller: Option<AccountId>,
    /// The time from which both accounts were inside a direct-message call.
    pub direct_answered_at: Option<OffsetDateTime>,
    /// The time from which one account is alone in a call that both were inside.
    pub direct_alone_since: Option<OffsetDateTime>,
    /// The call owes no missed call at its end: a ring of it ended with a line, or the
    /// other account was inside.
    pub direct_ring_settled: bool,
}

impl CallRow {
    pub fn key(&self) -> CallKey {
        key_of(&self.place)
    }
}

pub fn key_of(place: &CallPlace) -> CallKey {
    match *place {
        CallPlace::Channel { channel_id, .. } => CallKey::Channel(channel_id),
        CallPlace::Direct {
            low_account_id,
            high_account_id,
        } => CallKey::Direct(low_account_id, high_account_id),
    }
}

#[derive(sqlx::FromRow)]
struct CallRecord {
    id: CallId,
    room_name: String,
    workspace_id: Option<WorkspaceId>,
    channel_id: Option<ChannelId>,
    direct_low: Option<AccountId>,
    direct_high: Option<AccountId>,
    started_at: Option<OffsetDateTime>,
    locked_by: Option<AccountId>,
    locked_at: Option<OffsetDateTime>,
    meeting_presenter: Option<AccountId>,
    meeting_started_at: Option<OffsetDateTime>,
    meeting_presenter_gone_since: Option<OffsetDateTime>,
    hides_occupants: bool,
    direct_caller: Option<AccountId>,
    direct_answered_at: Option<OffsetDateTime>,
    direct_alone_since: Option<OffsetDateTime>,
    direct_ring_settled: bool,
}

const CALL_COLUMNS: &str = "id, room_name, workspace_id, channel_id, direct_low, direct_high, \
     started_at, locked_by, locked_at, meeting_presenter, meeting_started_at, \
     meeting_presenter_gone_since, hides_occupants, direct_caller, direct_answered_at, \
     direct_alone_since, direct_ring_settled";

fn call_row(record: CallRecord) -> ApiResult<CallRow> {
    let place = match (
        record.workspace_id,
        record.channel_id,
        record.direct_low,
        record.direct_high,
    ) {
        (Some(workspace_id), Some(channel_id), None, None) => CallPlace::Channel {
            workspace_id,
            channel_id,
        },
        (None, None, Some(low_account_id), Some(high_account_id)) => CallPlace::Direct {
            low_account_id,
            high_account_id,
        },
        _ => return Err(ApiError::internal(&"a stored call has no place")),
    };
    Ok(CallRow {
        id: record.id,
        room: RoomName::from_stored(record.room_name),
        place,
        started_at: record.started_at,
        lock: record
            .locked_by
            .zip(record.locked_at)
            .map(|(by, at)| CallLock { by, at }),
        meeting: record.meeting_presenter.zip(record.meeting_started_at).map(
            |(presenter, started_at)| Meeting {
                presenter,
                started_at,
            },
        ),
        presenter_gone_since: record.meeting_presenter_gone_since,
        hides_occupants: record.hides_occupants,
        direct_caller: record.direct_caller,
        direct_answered_at: record.direct_answered_at,
        direct_alone_since: record.direct_alone_since,
        direct_ring_settled: record.direct_ring_settled,
    })
}

#[derive(Debug, Clone, Copy)]
pub enum By {
    Id(CallId),
    Place(CallPlace),
}

/// The display name of an account, for its join token (ADR 0615).
pub async fn display_name(
    connection: &mut PgConnection,
    account_id: AccountId,
) -> ApiResult<String> {
    Ok(
        sqlx::query_scalar("select name from accounts where id = $1")
            .bind(account_id)
            .fetch_one(connection)
            .await?,
    )
}

/// Reads one call. With `for_change`, the row is locked and its revision grows, so that two
/// transactions that change the same call never both commit from the same state (ADR 0510).
pub async fn call(
    connection: &mut PgConnection,
    by: By,
    for_change: bool,
) -> ApiResult<Option<CallRow>> {
    let (filter, first, second) = match by {
        By::Id(id) => ("id = $1 and $2::uuid is null", id.as_uuid(), None),
        By::Place(CallPlace::Channel { channel_id, .. }) => (
            "channel_id = $1 and $2::uuid is null",
            channel_id.as_uuid(),
            None,
        ),
        By::Place(CallPlace::Direct {
            low_account_id,
            high_account_id,
        }) => (
            "direct_low = $1 and direct_high = $2",
            low_account_id.as_uuid(),
            Some(high_account_id.as_uuid()),
        ),
    };
    let statement = if for_change {
        format!("update calls set revision = revision + 1 where {filter} returning {CALL_COLUMNS}")
    } else {
        format!("select {CALL_COLUMNS} from calls where {filter}")
    };
    let record: Option<CallRecord> = sqlx::query_as(AssertSqlSafe(statement))
        .bind(first)
        .bind(second)
        .fetch_optional(connection)
        .await?;
    record.map(call_row).transpose()
}

/// The call at `place`, created when there is none. The row is locked for a change.
pub async fn call_for_entry(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    place: CallPlace,
    new_id: CallId,
    new_room: &RoomName,
) -> ApiResult<CallRow> {
    if let Some(existing) = call(&mut *connection, By::Place(place), true).await? {
        return Ok(existing);
    }
    let (workspace_id, channel_id, low, high) = match place {
        CallPlace::Channel {
            workspace_id,
            channel_id,
        } => (Some(workspace_id), Some(channel_id), None, None),
        CallPlace::Direct {
            low_account_id,
            high_account_id,
        } => (None, None, Some(low_account_id), Some(high_account_id)),
    };
    let hides_occupants: bool = sqlx::query_scalar(
        "insert into calls
             (id, room_name, workspace_id, channel_id, direct_low, direct_high, created_at)
         values ($1, $2, $3, $4, $5, $6, $7)
         returning hides_occupants",
    )
    .bind(new_id)
    .bind(new_room.as_str())
    .bind(workspace_id)
    .bind(channel_id)
    .bind(low)
    .bind(high)
    .bind(now)
    .fetch_one(&mut *connection)
    .await
    .map_err(ApiError::concurrent)?;
    Ok(CallRow {
        id: new_id,
        room: new_room.clone(),
        place,
        started_at: None,
        lock: None,
        meeting: None,
        presenter_gone_since: None,
        hides_occupants,
        direct_caller: None,
        direct_answered_at: None,
        direct_alone_since: None,
        direct_ring_settled: false,
    })
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct MemberRow {
    pub call_id: CallId,
    pub account_id: AccountId,
    pub device_id: DeviceId,
    pub session_id: Uuid,
    pub identity: String,
    pub usable_until: OffsetDateTime,
    pub connected_at: Option<OffsetDateTime>,
    pub absent_since: Option<OffsetDateTime>,
    pub self_muted: bool,
    pub self_deafened: bool,
    pub hand_raised_at: Option<OffsetDateTime>,
    pub share_audio: Option<String>,
    pub share_seen_at: Option<OffsetDateTime>,
    pub share_missing_since: Option<OffsetDateTime>,
    /// The identity of the seat in the source call that a completed move replaced.
    pub moved_from: Option<String>,
}

const MEMBER_COLUMNS: &str = "call_id, account_id, device_id, session_id, identity, \
     usable_until, connected_at, absent_since, self_muted, self_deafened, hand_raised_at, \
     share_audio, share_seen_at, share_missing_since, moved_from";

impl MemberRow {
    pub fn own_call(&self, call: &CallRow) -> OwnCall {
        OwnCall {
            hides_occupants: Some(call.hides_occupants),
            call_id: call.id,
            place: call.place,
            device_id: self.device_id,
            identity: self.identity.clone(),
        }
    }
}

/// The membership of an account, in whatever call it is. With `for_change` the row is locked.
pub async fn member_of(
    connection: &mut PgConnection,
    account_id: AccountId,
    for_change: bool,
) -> ApiResult<Option<MemberRow>> {
    let lock = if for_change { "for update" } else { "" };
    Ok(sqlx::query_as(AssertSqlSafe(format!(
        "select {MEMBER_COLUMNS} from call_members where account_id = $1 {lock}"
    )))
    .bind(account_id)
    .fetch_optional(connection)
    .await?)
}

pub async fn members(connection: &mut PgConnection, call_id: CallId) -> ApiResult<Vec<MemberRow>> {
    Ok(sqlx::query_as(AssertSqlSafe(format!(
        "select {MEMBER_COLUMNS} from call_members where call_id = $1 order by admitted_at, account_id"
    )))
    .bind(call_id)
    .fetch_all(connection)
    .await?)
}

/// The people who take a place in a call: its members and the people on their way into it by
/// a move.
pub async fn occupancy(connection: &mut PgConnection, call_id: CallId) -> ApiResult<i64> {
    Ok(sqlx::query_scalar(
        "select (select count(*) from call_members where call_id = $1)
              + (select count(*) from call_moves where to_call_id = $1)",
    )
    .bind(call_id)
    .fetch_one(connection)
    .await?)
}

#[derive(sqlx::FromRow)]
struct ParticipantRecord {
    avatar: Option<sqlx::types::Json<junto_protocol::account::Avatar>>,
    account_id: AccountId,
    identity: String,
    connected_at: OffsetDateTime,
    self_muted: bool,
    self_deafened: bool,
    hand_raised: bool,
    share_audio: Option<String>,
    share_started_at: Option<OffsetDateTime>,
    microphone_blocked_by: Option<AccountId>,
    microphone_block_reason: Option<String>,
    sound_blocked_by: Option<AccountId>,
}

fn participant(record: ParticipantRecord) -> ApiResult<CallParticipant> {
    let microphone_block = match (record.microphone_blocked_by, record.microphone_block_reason) {
        (Some(by), Some(reason)) => Some(MicrophoneBlock {
            by,
            reason: BlockReason::read(&reason)?,
        }),
        _ => None,
    };
    let screen_share = match (record.share_audio, record.share_started_at) {
        (Some(audio), Some(started_at)) => Some(ScreenShare {
            audio: ShareAudio::read(&audio)?,
            started_at,
        }),
        _ => None,
    };
    Ok(CallParticipant {
        avatar: record.avatar.map(|value| value.0),
        account_id: record.account_id,
        identity: record.identity,
        joined_at: record.connected_at,
        muted: record.self_muted,
        deafened: record.self_deafened,
        microphone_block,
        sound_block: record.sound_blocked_by.map(|by| SoundBlock { by }),
        hand_raised: record.hand_raised,
        screen_share,
    })
}

/// The members of a call (`$1`) whom the media server lists, or the one of `$2` when it is
/// not null, in the order in which they connected. The avatar of each member is read by its
/// account, once for each member: a join could read the avatars of the whole server
/// (ADR 0119).
pub const PARTICIPANTS: &str =
    "select m.account_id, m.identity, m.connected_at, m.self_muted, m.self_deafened,
                m.hand_raised_at is not null as hand_raised, m.share_audio, m.share_started_at,
                mb.blocked_by as microphone_blocked_by, mb.reason as microphone_block_reason,
                sb.blocked_by as sound_blocked_by,
                (select jsonb_build_object(
                    'hash', encode(av.hash, 'hex'), 'media_type', av.media_type,
                    'bytes', octet_length(av.content), 'width', av.width, 'height', av.height)
                 from account_avatars av where av.account_id = m.account_id) as avatar
         from call_members m
         left join call_microphone_blocks mb
             on mb.call_id = m.call_id and mb.account_id = m.account_id
         left join call_sound_blocks sb
             on sb.call_id = m.call_id and sb.account_id = m.account_id
         where m.call_id = $1 and m.connected_at is not null
           and ($2::uuid is null or m.account_id = $2)
         order by m.connected_at, m.account_id";

/// The members of a call whom the media server lists, or the one of `only`.
pub async fn participants(
    connection: &mut PgConnection,
    call_id: CallId,
    only: Option<AccountId>,
) -> ApiResult<Vec<CallParticipant>> {
    let records: Vec<ParticipantRecord> = sqlx::query_as(PARTICIPANTS)
        .bind(call_id)
        .bind(only)
        .fetch_all(connection)
        .await?;
    records.into_iter().map(participant).collect()
}

/// Whether an account is inside a call: the admission check has recorded that the media
/// server lists its seat, and the member row exists (ADR 0517). The database function
/// decides it, for this code and for every statement and trigger.
pub async fn is_inside(
    connection: &mut PgConnection,
    call_id: CallId,
    account_id: AccountId,
) -> ApiResult<bool> {
    Ok(sqlx::query_scalar("select is_inside_call($1, $2)")
        .bind(call_id)
        .bind(account_id)
        .fetch_one(connection)
        .await?)
}

/// How the seat `identity` of an account ended, when it is the last seat of the account that
/// ended (ADR 0520).
pub async fn seat_end(
    connection: &mut PgConnection,
    account_id: AccountId,
    identity: &str,
) -> ApiResult<Option<LeaveReason>> {
    let reason: Option<String> = sqlx::query_scalar(
        "select reason from call_seat_ends where account_id = $1 and identity = $2",
    )
    .bind(account_id)
    .bind(identity)
    .fetch_optional(connection)
    .await?;
    reason.as_deref().map(LeaveReason::read).transpose()
}

/// Whether the last end of a microphone block of a member was a grant of meeting mode that
/// the device has not answered with an open microphone (ADR 0521); false for an account
/// that has no seat in the call yet, as on its way into it by a move. Nothing for an
/// account that does not [`reads`] the call: a grant tells that meeting mode was on.
pub async fn meeting_grant(
    connection: &mut PgConnection,
    call: &CallRow,
    account_id: AccountId,
) -> ApiResult<Option<bool>> {
    if !reads(&mut *connection, call, account_id).await? {
        return Ok(None);
    }
    let granted: Option<bool> = sqlx::query_scalar(
        "select meeting_grant from call_members where call_id = $1 and account_id = $2",
    )
    .bind(call.id)
    .bind(account_id)
    .fetch_optional(connection)
    .await?;
    Ok(Some(granted.unwrap_or(false)))
}

/// Whether an account may read what tells who is inside a call: the call does not hide who
/// is inside, or the account is inside.
pub async fn reads(
    connection: &mut PgConnection,
    call: &CallRow,
    account_id: AccountId,
) -> ApiResult<bool> {
    Ok(!call.hides_occupants || is_inside(connection, call.id, account_id).await?)
}

/// The whole call, for an account that [`reads`] it.
pub async fn view(connection: &mut PgConnection, call: &CallRow) -> ApiResult<Call> {
    Ok(Call {
        id: call.id,
        place: call.place,
        started_at: call.started_at,
        lock: call.lock.clone(),
        meeting: call.meeting.clone(),
        hides_occupants: call.hides_occupants,
        participants: participants(&mut *connection, call.id, None).await?,
        recording: crate::recordings::store::active_of_call(connection, call.id).await?,
    })
}

/// The call as an account with a seat in it, or on its way into it, may read it now. An
/// account that is not inside a call that hides who is inside gets no lock, no meeting, no
/// participant and no recording; it reads them from `GET /api/v1/call` when the admission
/// check has seen it (ADR 0517).
pub async fn view_for(
    connection: &mut PgConnection,
    call: &CallRow,
    account_id: AccountId,
) -> ApiResult<Call> {
    if reads(&mut *connection, call, account_id).await? {
        return view(connection, call).await;
    }
    Ok(Call {
        id: call.id,
        place: call.place,
        started_at: call.started_at,
        lock: None,
        meeting: None,
        hides_occupants: true,
        participants: Vec::new(),
        recording: None,
    })
}

/// What `viewer`, who may see the voice channel, learns of its call.
pub async fn channel_call(
    connection: &mut PgConnection,
    channel: &Channel,
    viewer: AccountId,
) -> ApiResult<ChannelCall> {
    let place = CallPlace::Channel {
        workspace_id: channel.workspace_id,
        channel_id: channel.id,
    };
    let row = call(&mut *connection, By::Place(place), false).await?;
    let in_use = row.as_ref().is_some_and(|call| call.started_at.is_some());
    let shown = match &row {
        Some(call) if in_use && reads(&mut *connection, call, viewer).await? => {
            Some(view(connection, call).await?)
        }
        _ => None,
    };
    Ok(ChannelCall {
        workspace_id: channel.workspace_id,
        channel_id: channel.id,
        in_use,
        call: shown,
    })
}

/// Tells the accounts that may see who is inside that a participant changed. A member whom
/// the media server does not list yet is not shown, so nothing is told.
pub async fn announce_participant(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    call: &CallRow,
    account_id: AccountId,
) -> ApiResult<()> {
    let shown = participants(&mut *connection, call.id, Some(account_id)).await?;
    for participant in shown {
        let updated = Event::CallParticipantUpdated {
            call_id: call.id,
            place: call.place,
            participant,
        };
        events::publish(&mut *connection, now, &updated).await?;
    }
    Ok(())
}

/// What an account may publish in a call: pictures only while its microphone is blocked.
/// An account that is not a member yet, on its way into a call with meeting mode, gets the
/// block of the meeting on its entry, so it is treated as blocked already.
pub async fn publishing_of(
    connection: &mut PgConnection,
    call: &CallRow,
    account_id: AccountId,
) -> ApiResult<Publishing> {
    let (blocked, is_member): (bool, bool) = sqlx::query_as(
        "select exists (
                    select 1 from call_microphone_blocks where call_id = $1 and account_id = $2),
                exists (select 1 from call_members where call_id = $1 and account_id = $2)",
    )
    .bind(call.id)
    .bind(account_id)
    .fetch_one(connection)
    .await?;
    let by_meeting = !is_member
        && call
            .meeting
            .as_ref()
            .is_some_and(|meeting| meeting.presenter != account_id);
    Ok(if blocked || by_meeting {
        Publishing::pictures_only()
    } else {
        Publishing::open()
    })
}

/// What the join token of an account states that the account may publish. An account that
/// is not inside a call that hides who is inside gets pictures only, whatever the state of
/// the call: the grant of [`publishing_of`] depends on meeting mode, which such an account
/// must not learn (ADR 0517). The admission check sets what the account may publish when it
/// has seen the account in the room.
pub async fn publishing_at_join(
    connection: &mut PgConnection,
    call: &CallRow,
    account_id: AccountId,
) -> ApiResult<Publishing> {
    if !reads(&mut *connection, call, account_id).await? {
        return Ok(Publishing::pictures_only());
    }
    publishing_of(connection, call, account_id).await
}

/// The blocks of an account that the answer to its request for a place states. An account
/// that is not a member yet, on its way into a call with meeting mode, gets the block of the
/// meeting on its entry, so the answer states it already. An account that is not inside a
/// call that hides who is inside gets none: a block names the person who set it, and the
/// block of a meeting tells that meeting mode is on (ADR 0517). Such an account reads its
/// blocks with the call when the admission check has seen it.
pub async fn blocks_at_join(
    connection: &mut PgConnection,
    call: &CallRow,
    account_id: AccountId,
) -> ApiResult<(Option<MicrophoneBlock>, Option<SoundBlock>)> {
    if !reads(&mut *connection, call, account_id).await? {
        return Ok((None, None));
    }
    let (microphone_by, reason, sound_by, is_member): (
        Option<AccountId>,
        Option<String>,
        Option<AccountId>,
        bool,
    ) = sqlx::query_as(
        "select mb.blocked_by, mb.reason, sb.blocked_by,
                exists (select 1 from call_members where call_id = $1 and account_id = $2)
         from (select 1) one
         left join call_microphone_blocks mb on mb.call_id = $1 and mb.account_id = $2
         left join call_sound_blocks sb on sb.call_id = $1 and sb.account_id = $2",
    )
    .bind(call.id)
    .bind(account_id)
    .fetch_one(connection)
    .await?;
    let microphone = match (microphone_by, reason) {
        (Some(by), Some(reason)) => Some(MicrophoneBlock {
            by,
            reason: BlockReason::read(&reason)?,
        }),
        _ => call
            .meeting
            .as_ref()
            .filter(|meeting| !is_member && meeting.presenter != account_id)
            .map(|meeting| MicrophoneBlock {
                by: meeting.presenter,
                reason: BlockReason::Meeting,
            }),
    };
    let sound = sound_by;
    Ok((microphone, sound.map(|by| SoundBlock { by })))
}

/// One place in a call: the account, the device and the session that asked for it, and the
/// one identity that is allowed in the room for it.
#[derive(Debug, Clone, Copy)]
pub struct Seat<'a> {
    pub account_id: AccountId,
    pub device_id: DeviceId,
    pub session_id: Uuid,
    pub identity: &'a str,
}

/// Allows a seat in the room of a call. During meeting mode the account gets the microphone
/// block of the meeting, unless it is the presenter.
pub async fn admit(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    call: &CallRow,
    seat: Seat<'_>,
    usable_until: OffsetDateTime,
    connected: bool,
) -> ApiResult<()> {
    sqlx::query(
        "insert into call_members
             (call_id, account_id, device_id, session_id, identity, admitted_at, usable_until,
              connected_at)
         values ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(call.id)
    .bind(seat.account_id)
    .bind(seat.device_id)
    .bind(seat.session_id)
    .bind(seat.identity)
    .bind(now)
    .bind(usable_until)
    .bind(connected.then_some(now))
    .execute(&mut *connection)
    .await
    .map_err(ApiError::concurrent)?;
    if let Some(meeting) = &call.meeting
        && meeting.presenter != seat.account_id
    {
        let only = Some(seat.account_id);
        block_for_meeting(connection, now, call.id, meeting.presenter, only).await?;
    }
    Ok(())
}

/// Gives the members of a call, or the one of `only`, the microphone block of meeting mode.
/// A block that exists stays as it is.
async fn block_for_meeting(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    call_id: CallId,
    presenter: AccountId,
    only: Option<AccountId>,
) -> ApiResult<()> {
    sqlx::query(
        "insert into call_microphone_blocks (call_id, account_id, blocked_by, reason, blocked_at)
         select m.call_id, m.account_id, $2, 'meeting', $3
         from call_members m
         where m.call_id = $1 and m.account_id <> $2 and ($4::uuid is null or m.account_id = $4)
         on conflict (call_id, account_id) do nothing",
    )
    .bind(call_id)
    .bind(presenter)
    .bind(now)
    .bind(only)
    .execute(connection)
    .await?;
    Ok(())
}

pub async fn start_meeting(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    call: &CallRow,
    presenter: AccountId,
) -> ApiResult<()> {
    sqlx::query(
        "update calls
         set meeting_presenter = $2, meeting_started_at = $3, meeting_presenter_gone_since = null
         where id = $1",
    )
    .bind(call.id)
    .bind(presenter)
    .bind(now)
    .execute(&mut *connection)
    .await?;
    block_for_meeting(&mut *connection, now, call.id, presenter, None).await?;
    note(
        &mut *connection,
        now,
        call,
        Note::MeetingStarted,
        Some(presenter),
    )
    .await?;
    let started = Event::CallMeetingStarted {
        call_id: call.id,
        place: call.place,
        meeting: Meeting {
            presenter,
            started_at: now,
        },
    };
    events::publish(connection, now, &started).await
}

/// Ends meeting mode and removes every microphone block of the call, the ones of moderators
/// included (ADR 0505).
pub async fn end_meeting(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    call: &CallRow,
    reason: MeetingEnd,
) -> ApiResult<()> {
    sqlx::query(
        "update calls
         set meeting_presenter = null, meeting_started_at = null,
             meeting_presenter_gone_since = null
         where id = $1",
    )
    .bind(call.id)
    .execute(&mut *connection)
    .await?;
    sqlx::query("delete from call_microphone_blocks where call_id = $1")
        .bind(call.id)
        .execute(&mut *connection)
        .await?;
    let presenter = call.meeting.as_ref().map(|meeting| meeting.presenter);
    note(&mut *connection, now, call, Note::MeetingEnded, presenter).await?;
    let ended = Event::CallMeetingEnded {
        call_id: call.id,
        place: call.place,
        reason,
    };
    events::publish(connection, now, &ended).await
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct MoveRow {
    pub id: CallMoveId,
    pub account_id: AccountId,
    pub from_call_id: CallId,
    pub to_call_id: CallId,
    pub moved_by: AccountId,
    pub identity: String,
    pub expires_at: OffsetDateTime,
}

pub const MOVE_COLUMNS: &str =
    "id, account_id, from_call_id, to_call_id, moved_by, identity, expires_at";

/// Ends a move that did not complete and tells the moved person and the person who moved:
/// nothing changed (ADR 0511). The identity of the move stops being allowed in the
/// destination.
pub async fn fail_move(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    pending: &MoveRow,
    outcome: MoveOutcome,
) -> ApiResult<()> {
    sqlx::query("delete from call_moves where id = $1")
        .bind(pending.id)
        .execute(&mut *connection)
        .await?;
    tell_move(&mut *connection, now, pending, outcome).await?;
    end_if_empty(connection, now, pending.to_call_id).await?;
    Ok(())
}

/// Tells both accounts of a move how it ended, and a person who moved themselves once. The
/// event names the move and neither channel. An account that can no longer see one of the two channels does not get it: it
/// gets `call_access_ended` from the check that notices the loss (ADR 0510).
pub async fn tell_move(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    pending: &MoveRow,
    outcome: MoveOutcome,
) -> ApiResult<()> {
    let resolved = Event::CallMoveResolved {
        move_id: pending.id,
        outcome,
    };
    let mut places = Vec::new();
    for call_id in [pending.from_call_id, pending.to_call_id] {
        if let Some(at) = call(&mut *connection, By::Id(call_id), false).await? {
            places.push(at.place);
        }
    }
    let mover = Some(pending.moved_by).filter(|mover| *mover != pending.account_id);
    for account_id in std::iter::once(pending.account_id).chain(mover) {
        events::tell_account_at(&mut *connection, now, account_id, &places, &resolved).await?;
    }
    Ok(())
}

/// Tells an account that what it waited for at places that it can no longer be at has ended.
/// The event names nothing.
pub async fn tell_access_ended(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    account_id: AccountId,
) -> ApiResult<()> {
    events::tell_account(connection, now, account_id, &Event::CallAccessEnded {}).await
}

pub fn call_move(pending: &MoveRow, from: CallPlace, to: CallPlace) -> CallMove {
    CallMove {
        id: pending.id,
        account_id: pending.account_id,
        from,
        to,
        moved_by: pending.moved_by,
        expires_at: pending.expires_at,
    }
}

/// Takes a member out of a call: its identity is no longer allowed in the room, and the
/// admission check removes it there. A move of the member that waits is abandoned.
pub async fn remove_member(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    call: &CallRow,
    member: &MemberRow,
    reason: LeaveReason,
) -> ApiResult<()> {
    abandon_move_of(&mut *connection, now, member.account_id).await?;
    take_out(&mut *connection, now, call, member, reason).await?;
    let changed = Event::OwnCallChanged {
        call: None,
        left: Some(reason),
    };
    events::tell_account(connection, now, member.account_id, &changed).await
}

/// Ends the move of an account that waits for the join of its destination.
pub async fn abandon_move_of(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    account_id: AccountId,
) -> ApiResult<()> {
    let pending: Option<MoveRow> = sqlx::query_as(AssertSqlSafe(format!(
        "select {MOVE_COLUMNS} from call_moves where account_id = $1"
    )))
    .bind(account_id)
    .fetch_optional(&mut *connection)
    .await?;
    if let Some(pending) = pending {
        fail_move(connection, now, &pending, MoveOutcome::Abandoned).await?;
    }
    Ok(())
}

/// Deletes the row of a member and tells the accounts that may see who is inside.
pub async fn take_out(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    call: &CallRow,
    member: &MemberRow,
    reason: LeaveReason,
) -> ApiResult<()> {
    sqlx::query("delete from call_members where call_id = $1 and account_id = $2")
        .bind(call.id)
        .bind(member.account_id)
        .execute(&mut *connection)
        .await?;
    sqlx::query(
        "insert into call_seat_ends (account_id, identity, reason, ended_at)
         values ($1, $2, $3, $4)
         on conflict (account_id) do update
             set identity = excluded.identity, reason = excluded.reason,
                 ended_at = excluded.ended_at",
    )
    .bind(member.account_id)
    .bind(&member.identity)
    .bind(reason.stored())
    .bind(now)
    .execute(&mut *connection)
    .await?;
    // A move and a removal by a moderator have the lines of their audit entries, and a
    // switch to another seat in the same call is no departure.
    if member.connected_at.is_some()
        && matches!(
            reason,
            LeaveReason::Left | LeaveReason::ConnectionLost | LeaveReason::AccessEnded
        )
    {
        note_left(&mut *connection, now, call, member.account_id).await?;
    }
    if member.connected_at.is_some() {
        let left = Event::CallParticipantLeft {
            call_id: call.id,
            place: call.place,
            account_id: member.account_id,
            reason,
        };
        events::publish(&mut *connection, now, &left).await?;
        if matches!(call.place, CallPlace::Direct { .. }) {
            // The time in which both accounts were in the room ends here, or when the
            // connection of the member was lost (ADR 0518).
            sqlx::query(
                "update calls set direct_alone_since = coalesce(direct_alone_since, $2)
                 where id = $1 and direct_answered_at is not null",
            )
            .bind(call.id)
            .bind(member.absent_since.unwrap_or(now))
            .execute(&mut *connection)
            .await?;
        }
    }
    let presents = call
        .meeting
        .as_ref()
        .is_some_and(|meeting| meeting.presenter == member.account_id);
    if presents {
        sqlx::query(
            "update calls
             set meeting_presenter_gone_since = coalesce(meeting_presenter_gone_since, $2)
             where id = $1",
        )
        .bind(call.id)
        .bind(now)
        .execute(connection)
        .await?;
    }
    Ok(())
}

/// Tells both sides of an invitation how it ended, each while it may be at the place of the
/// call. A declined ring of a direct-message call is recorded for the pause that follows
/// it, and an accepted one is no longer counted (ADR 0515). The invited account of a
/// silent invitation was not told of it and is not told how it ended.
pub async fn tell_invitation(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    waiting: &Waiting,
    outcome: InvitationOutcome,
) -> ApiResult<()> {
    let invitation = &waiting.invitation;
    match outcome {
        InvitationOutcome::Declined => {
            ring_declined(&mut *connection, now, invitation.id).await?;
        }
        InvitationOutcome::Accepted => {
            sqlx::query("delete from direct_call_rings where id = $1")
                .bind(invitation.id)
                .execute(&mut *connection)
                .await?;
        }
        InvitationOutcome::Expired | InvitationOutcome::Cancelled => {}
    }
    let resolved = Event::CallInvitationResolved {
        invitation_id: invitation.id,
        outcome,
    };
    let at = [invitation.place];
    for account_id in waiting.told() {
        events::tell_account_at(&mut *connection, now, account_id, &at, &resolved).await?;
    }
    // The ring of a direct-message call ends with a line in the conversation. The line is
    // the same for a silent ring, so the caller reads nothing of a mute (ADR 0518).
    if matches!(invitation.place, CallPlace::Direct { .. }) {
        let line = match outcome {
            InvitationOutcome::Declined => Some(Line::Declined),
            InvitationOutcome::Expired | InvitationOutcome::Cancelled => Some(Line::NotAnswered),
            InvitationOutcome::Accepted => None,
        };
        if let Some(line) = line {
            let (caller, callee) = (invitation.invited_by, invitation.account_id);
            direct_calls::line(&mut *connection, now, caller, callee, line).await?;
            sqlx::query("update calls set direct_ring_settled = true where id = $1")
                .bind(invitation.call_id)
                .execute(connection)
                .await?;
        }
    }
    Ok(())
}

pub type InvitationRow = (
    CallInvitationId,
    AccountId,
    AccountId,
    OffsetDateTime,
    OffsetDateTime,
    bool,
);

pub const INVITATION_COLUMNS: &str = "id, account_id, invited_by, created_at, expires_at, silent";

/// An invitation that waits. A silent one is the ring of a direct-message call for an
/// account that muted the conversation or is in its quiet time: the caller has it, with
/// the events and the end of every ring, and the invited account learns nothing of it
/// (ADR 0515).
#[derive(Debug, Clone)]
pub struct Waiting {
    pub invitation: CallInvitation,
    pub silent: bool,
}

impl Waiting {
    /// The accounts that are told of the invitation and of its end.
    pub fn told(&self) -> impl Iterator<Item = AccountId> {
        let invited = (!self.silent).then_some(self.invitation.account_id);
        invited
            .into_iter()
            .chain(std::iter::once(self.invitation.invited_by))
    }
}

pub fn invitation(call: &CallRow, row: InvitationRow) -> Waiting {
    let (id, account_id, invited_by, created_at, expires_at, silent) = row;
    Waiting {
        invitation: CallInvitation {
            id,
            call_id: call.id,
            place: call.place,
            account_id,
            invited_by,
            created_at,
            expires_at,
        },
        silent,
    }
}

/// Removes the invitations of a call that match `filter` over `$1` (the call) and `$2`, and
/// tells both sides the outcome.
pub async fn resolve_invitations(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    call: &CallRow,
    filter: &'static str,
    value: Option<Uuid>,
    due: Option<OffsetDateTime>,
    outcome: InvitationOutcome,
) -> ApiResult<usize> {
    let rows: Vec<InvitationRow> = sqlx::query_as(AssertSqlSafe(format!(
        "delete from call_invitations where call_id = $1 and {filter}
         returning {INVITATION_COLUMNS}"
    )))
    .bind(call.id)
    .bind(value)
    .bind(due)
    .fetch_all(&mut *connection)
    .await?;
    let resolved = rows.len();
    for row in rows {
        tell_invitation(&mut *connection, now, &invitation(call, row), outcome).await?;
    }
    Ok(resolved)
}

type EntryRow = (EntryRequestId, AccountId, OffsetDateTime, OffsetDateTime);

pub const ENTRY_COLUMNS: &str = "id, account_id, created_at, expires_at";

pub fn entry_request(call: &CallRow, row: EntryRow) -> EntryRequest {
    let (id, account_id, created_at, expires_at) = row;
    EntryRequest {
        id,
        call_id: call.id,
        place: call.place,
        account_id,
        created_at,
        expires_at,
    }
}

/// The requests to enter a call that wait for an answer, for an account inside the call
/// that answers them. Another account gets none: the requests are for the people inside
/// (ADR 0517), and a request to enter a full call for the holders of "admit to full call"
/// among them when one is inside (ADR 0514).
pub async fn entry_requests(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    call: &CallRow,
    reader: AccountId,
) -> ApiResult<Vec<EntryRequest>> {
    if !is_inside(&mut *connection, call.id, reader).await? {
        return Ok(Vec::new());
    }
    let rows: Vec<EntryRow> = sqlx::query_as(AssertSqlSafe(format!(
        "select {ENTRY_COLUMNS} from call_entry_requests
         where call_id = $1 and accepted_by is null and expires_at > $2
           and (answerers is null or $3 = any(answerers))
         order by created_at, id"
    )))
    .bind(call.id)
    .bind(now)
    .bind(reader)
    .fetch_all(connection)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| entry_request(call, row))
        .collect())
}

/// Tells the people inside and the person who asked how a request to enter ended. The
/// person outside gets an event that names the request and nothing of the call: not who is
/// inside, and not the channel. A person who can no longer see the channel does not get it.
pub async fn tell_entry(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    request: &EntryRequest,
    outcome: EntryOutcome,
    by: Option<AccountId>,
) -> ApiResult<()> {
    let resolved = Event::CallEntryResolved {
        request_id: request.id,
        call_id: request.call_id,
        place: request.place,
        account_id: request.account_id,
        outcome,
        by,
    };
    if matches!(request.place, CallPlace::Channel { .. }) {
        events::tell_inside(&mut *connection, now, &resolved).await?;
    }
    let answered = Event::CallEntryAnswered {
        request_id: request.id,
        outcome,
    };
    let at = [request.place];
    events::tell_account_at(connection, now, request.account_id, &at, &answered).await
}

/// Removes the requests to enter a call that match `filter` over `$1` (the call), `$2` and
/// `$3`, and tells how each ended.
pub async fn resolve_entries(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    call: &CallRow,
    filter: &'static str,
    value: Option<Uuid>,
    due: Option<OffsetDateTime>,
    outcome: EntryOutcome,
) -> ApiResult<usize> {
    let rows: Vec<EntryRow> = sqlx::query_as(AssertSqlSafe(format!(
        "delete from call_entry_requests where call_id = $1 and {filter}
         returning {ENTRY_COLUMNS}"
    )))
    .bind(call.id)
    .bind(value)
    .bind(due)
    .fetch_all(&mut *connection)
    .await?;
    let resolved = rows.len();
    for row in rows {
        let request = entry_request(call, row);
        tell_entry(&mut *connection, now, &request, outcome, None).await?;
    }
    Ok(resolved)
}

/// The other account of a direct-message call. `None` for a voice channel and for an account
/// that is not one of the two.
pub fn other_of(place: &CallPlace, account_id: AccountId) -> Option<AccountId> {
    match *place {
        CallPlace::Direct {
            low_account_id,
            high_account_id,
        } if account_id == low_account_id => Some(high_account_id),
        CallPlace::Direct {
            low_account_id,
            high_account_id,
        } if account_id == high_account_id => Some(low_account_id),
        _ => None,
    }
}

/// Records the account whose entry made the ring of a direct-message call, and writes the
/// line of the start of the call into the conversation (ADR 0518).
pub async fn direct_call_rung(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    call: &CallRow,
    caller: AccountId,
    callee: AccountId,
) -> ApiResult<()> {
    sqlx::query("update calls set direct_caller = $2 where id = $1")
        .bind(call.id)
        .bind(caller)
        .execute(&mut *connection)
        .await?;
    direct_calls::line(connection, now, caller, callee, Line::Started).await
}

/// Records what the admission check saw of `account_id` in the room of a direct-message
/// call: the account is inside (ADR 0517). The other account of the ring that was inside
/// has no missed call. When both accounts are in the room, the time that the line of the
/// end states runs: from the first such moment to the moment from which one account was
/// alone for good (ADR 0518). A member whose connection is lost is not in the room.
pub async fn direct_call_seen(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    call: &CallRow,
    account_id: AccountId,
) -> ApiResult<()> {
    sqlx::query(
        "update calls c
         set direct_ring_settled = c.direct_ring_settled or c.direct_caller is distinct from $3,
             direct_answered_at = case when seen.both
                 then coalesce(c.direct_answered_at, $2) else c.direct_answered_at end,
             direct_alone_since = case when seen.both then null else c.direct_alone_since end
         from (select count(*) >= 2 as both from call_members
               where call_id = $1 and connected_at is not null and absent_since is null) seen
         where c.id = $1",
    )
    .bind(call.id)
    .bind(now)
    .bind(account_id)
    .execute(connection)
    .await?;
    Ok(())
}

/// Ends a call that has no member and no person on the way into it: its row is deleted, and
/// with it its lock, its meeting, its blocks, its invitations and its requests. Returns
/// true when the call ended.
pub async fn end_if_empty(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    call_id: CallId,
) -> ApiResult<bool> {
    if occupancy(&mut *connection, call_id).await? > 0 {
        return Ok(false);
    }
    let Some(ending) = call(&mut *connection, By::Id(call_id), true).await? else {
        return Ok(false);
    };
    if ending.meeting.is_some() {
        end_meeting(&mut *connection, now, &ending, MeetingEnd::PresenterLeft).await?;
    }
    let all = "($2::uuid is null and $3::timestamptz is null)";
    let cancelled = InvitationOutcome::Cancelled;
    let rings =
        resolve_invitations(&mut *connection, now, &ending, all, None, None, cancelled).await?;
    let cancelled = EntryOutcome::Cancelled;
    resolve_entries(&mut *connection, now, &ending, all, None, None, cancelled).await?;
    if ending.place.channel_id().is_some() {
        let hides = ending.hides_occupants;
        crate::recordings::store::call_ended(&mut *connection, now, call_id, hides).await?;
    }
    if let Some(caller) = ending.direct_caller
        && let Some(callee) = other_of(&ending.place, caller)
    {
        // A call that both accounts were in the room of ends with its duration. A call
        // that the other account was never inside, and whose ring has no line yet, was
        // not answered: the other account took a place and did not connect (ADR 0518).
        let line = match ending.direct_answered_at {
            Some(answered_at) => {
                let until = ending.direct_alone_since.unwrap_or(now);
                let seconds = u64::try_from((until - answered_at).whole_seconds());
                Some(Line::Ended {
                    seconds: seconds.unwrap_or_default(),
                })
            }
            None if !ending.direct_ring_settled && rings == 0 => Some(Line::NotAnswered),
            None => None,
        };
        if let Some(line) = line {
            direct_calls::line(&mut *connection, now, caller, callee, line).await?;
        }
    }
    if ending.started_at.is_some() {
        note(&mut *connection, now, &ending, Note::CallEnded, None).await?;
        let ended = Event::CallEnded {
            call_id,
            place: ending.place,
        };
        events::publish(&mut *connection, now, &ended).await?;
    }
    sqlx::query("delete from calls where id = $1")
        .bind(call_id)
        .execute(connection)
        .await?;
    Ok(true)
}

/// Ends the places in calls that belong to sessions that end, with their moves, in the
/// transaction that ends the sessions (ADR 0510). The rooms are marked for the next check.
pub async fn end_places_of(
    connection: &mut PgConnection,
    calls: &Calls,
    now: OffsetDateTime,
    sessions: Sessions,
) -> ApiResult<()> {
    let (filter, id) = match sessions {
        Sessions::One(id) => ("session_id = $1", id),
        Sessions::OfDevice(id) => ("device_id = $1", id.as_uuid()),
        Sessions::OfAccount(id) => ("account_id = $1", id.as_uuid()),
    };
    let members: Vec<MemberRow> = sqlx::query_as(AssertSqlSafe(format!(
        "select {MEMBER_COLUMNS} from call_members where {filter} for update"
    )))
    .bind(id)
    .fetch_all(&mut *connection)
    .await?;
    end_places(connection, calls, now, members).await
}

/// Ends the places in calls whose session has expired or no longer exists. At most `most`
/// places end in one call of this function.
pub async fn end_places_without_a_session(
    connection: &mut PgConnection,
    calls: &Calls,
    now: OffsetDateTime,
    most: i64,
) -> ApiResult<usize> {
    let members: Vec<MemberRow> = sqlx::query_as(AssertSqlSafe(format!(
        "select {MEMBER_COLUMNS} from call_members m
         where not exists (
             select 1 from sessions s where s.id = m.session_id and s.expires_at > $1)
         limit $2
         for update"
    )))
    .bind(now)
    .bind(most)
    .fetch_all(&mut *connection)
    .await?;
    let ended = members.len();
    end_places(connection, calls, now, members).await?;
    Ok(ended)
}

async fn end_places(
    connection: &mut PgConnection,
    calls: &Calls,
    now: OffsetDateTime,
    members: Vec<MemberRow>,
) -> ApiResult<()> {
    for member in members {
        let Some(place) = call(&mut *connection, By::Id(member.call_id), true).await? else {
            continue;
        };
        let ended = LeaveReason::AccessEnded;
        remove_member(&mut *connection, now, &place, &member, ended).await?;
        end_if_empty(&mut *connection, now, place.id).await?;
        calls.mark(place.room.as_str());
    }
    Ok(())
}

/// The direct-message conversations that `account_id` has muted now.
pub async fn direct_mutes(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    account_id: AccountId,
) -> ApiResult<Vec<DirectMute>> {
    let rows: Vec<(AccountId, Option<OffsetDateTime>)> = sqlx::query_as(
        "select other_account_id, muted_until from direct_mutes
         where account_id = $1 and (muted_until is null or muted_until > $2)
         order by other_account_id",
    )
    .bind(account_id)
    .bind(now)
    .fetch_all(connection)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(account_id, until)| DirectMute { account_id, until })
        .collect())
}

/// Whether `account_id` has muted its direct-message conversation with `other` now.
pub async fn has_muted(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    account_id: AccountId,
    other: AccountId,
) -> ApiResult<bool> {
    Ok(sqlx::query_scalar(
        "select exists (
             select 1 from direct_mutes
             where account_id = $1 and other_account_id = $2
               and (muted_until is null or muted_until > $3))",
    )
    .bind(account_id)
    .bind(other)
    .bind(now)
    .fetch_one(connection)
    .await?)
}

/// Counts a ring of `caller` for `callee`, or refuses it: after a declined ring the caller
/// waits, and the rings of one account for another in a time are limited. The rows are in
/// the database, so the rule holds for every session of the caller (ADR 0515).
pub async fn count_ring(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    limits: &crate::config::CallLimits,
    ring_id: CallInvitationId,
    caller: AccountId,
    callee: AccountId,
) -> ApiResult<()> {
    let refused = |wait: time::Duration| {
        ApiError::new(
            ErrorCode::RateLimited,
            "the account was called too often; wait before the next call",
        )
        .retry_after(wait.max(time::Duration::seconds(1)))
    };
    let declined_at: Option<OffsetDateTime> = sqlx::query_scalar(
        "select max(declined_at) from direct_call_rings where caller_id = $1 and callee_id = $2",
    )
    .bind(caller)
    .bind(callee)
    .fetch_one(&mut *connection)
    .await?;
    if let Some(declined_at) = declined_at
        && now < declined_at + limits.ring_pause_after_decline
    {
        return Err(refused(declined_at + limits.ring_pause_after_decline - now));
    }
    let recent: Vec<OffsetDateTime> = sqlx::query_scalar(
        "select rung_at from direct_call_rings
         where caller_id = $1 and callee_id = $2 and rung_at > $3
         order by rung_at",
    )
    .bind(caller)
    .bind(callee)
    .bind(now - limits.rings.window)
    .fetch_all(&mut *connection)
    .await?;
    let most = usize::try_from(limits.rings.attempts).unwrap_or(usize::MAX);
    if recent.len() >= most
        && let Some(oldest) = recent.first()
    {
        return Err(refused(*oldest + limits.rings.window - now));
    }
    sqlx::query(
        "insert into direct_call_rings (id, caller_id, callee_id, rung_at)
         values ($1, $2, $3, $4)",
    )
    .bind(ring_id)
    .bind(caller)
    .bind(callee)
    .bind(now)
    .execute(connection)
    .await?;
    Ok(())
}

/// Records that the ring behind an invitation was declined. An invitation into a voice
/// channel has no ring, and nothing changes.
pub async fn ring_declined(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    invitation_id: CallInvitationId,
) -> ApiResult<()> {
    sqlx::query("update direct_call_rings set declined_at = $2 where id = $1")
        .bind(invitation_id)
        .bind(now)
        .execute(connection)
        .await?;
    Ok(())
}

/// Marks the rooms whose people a change of permissions can concern, so that the check
/// looks at them at once: the calls of the workspace, or every call for a change that is
/// not of one workspace.
pub async fn mark_after_permission_change(
    connection: &mut PgConnection,
    workspace_id: Option<WorkspaceId>,
) -> ApiResult<()> {
    let rooms: Vec<String> = sqlx::query_scalar(
        "select room_name from calls where $1::uuid is null or workspace_id = $1",
    )
    .bind(workspace_id)
    .fetch_all(connection)
    .await?;
    for room in rooms {
        crate::calls::changed(&room);
    }
    Ok(())
}

/// Marks the room of the call of a voice channel for the next check, after a change that
/// can end the access of the people inside.
pub async fn mark_channel(
    connection: &mut PgConnection,
    calls: &Calls,
    channel_id: ChannelId,
) -> ApiResult<()> {
    let room: Option<String> =
        sqlx::query_scalar("select room_name from calls where channel_id = $1")
            .bind(channel_id)
            .fetch_optional(connection)
            .await?;
    if let Some(room) = room {
        calls.mark(&room);
    }
    Ok(())
}

/// The place of the call that an account has a seat in.
pub async fn seat_key(
    connection: &mut PgConnection,
    account_id: AccountId,
) -> ApiResult<Option<CallKey>> {
    let row: Option<(Option<ChannelId>, Option<AccountId>, Option<AccountId>)> = sqlx::query_as(
        "select c.channel_id, c.direct_low, c.direct_high
         from call_members m
         join calls c on c.id = m.call_id
         where m.account_id = $1",
    )
    .bind(account_id)
    .fetch_optional(connection)
    .await?;
    Ok(
        row.and_then(|(channel_id, low, high)| match (channel_id, low, high) {
            (Some(channel_id), ..) => Some(CallKey::Channel(channel_id)),
            (None, Some(low), Some(high)) => Some(CallKey::Direct(low, high)),
            _ => None,
        }),
    )
}

/// Every account that is a member of a call, with the place of its call.
pub async fn everyone_in_a_call(
    connection: &mut PgConnection,
) -> ApiResult<Vec<(AccountId, CallKey)>> {
    type Row = (
        AccountId,
        Option<ChannelId>,
        Option<AccountId>,
        Option<AccountId>,
    );
    let rows: Vec<Row> = sqlx::query_as(
        "select m.account_id, c.channel_id, c.direct_low, c.direct_high
         from call_members m
         join calls c on c.id = m.call_id",
    )
    .fetch_all(connection)
    .await?;
    Ok(rows
        .into_iter()
        .filter_map(|(account_id, channel_id, low, high)| {
            let key = match (channel_id, low, high) {
                (Some(channel_id), ..) => CallKey::Channel(channel_id),
                (None, Some(low), Some(high)) => CallKey::Direct(low, high),
                _ => return None,
            };
            Some((account_id, key))
        })
        .collect())
}
