//! What one connection may receive next. Each decision reads the log, the session and the
//! roles in one read-only transaction that starts immediately before the messages are written
//! to the socket (ADR 0011).

use std::collections::{HashMap, HashSet};

use junto_protocol::account::{AccountStatus, RecoveryNotice};
use junto_protocol::error::ErrorCode;
use junto_protocol::events::{Cursor, EndReason, Event, Level, ResyncReason, ServerMessage};
use junto_protocol::ids::{
    AccountId, CallId, ChannelId, DeviceId, RecoveryId, StreamEpoch, WorkspaceId, WorkspaceRoleId,
};
use sqlx::PgConnection;
use sqlx::types::Json;
use time::OffsetDateTime;
use tokio::time::Instant;
use uuid::Uuid;

use crate::access::Access;
use crate::db::StoredText;
use crate::error::{ApiError, ApiResult};
use crate::events::audience::{
    Audience, CallStanding, ChannelStanding, Class, Recipient, WorkspaceStanding, receives,
};
use crate::events::hub::CallKey;
use crate::events::log::{self, STORED_AUDIENCE_BYTES, STORED_PAYLOAD_BYTES};
use crate::extract::Bearer;
use crate::read;
use crate::state::AppState;

const COUNTED_PER_READ: i64 = 500;

/// The lists of receivers, of the ones in `$1`, that hold the account `$2`.
pub const RECEIVER_LISTS: &str =
    "select event_id from event_receivers where event_id = any($1) and account_id = $2";

/// The device behind a connection, as the upgrade request proved it.
#[derive(Debug, Clone, Copy)]
pub struct Opened {
    pub bearer: Bearer,
    pub account_id: AccountId,
    pub device_id: DeviceId,
    pub session_id: Uuid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Subscription {
    pub level: Level,
    /// The connection has been served every event up to this position.
    pub position: i64,
    /// The epoch of the cursor that the subscription started from (ADR 0010).
    pub epoch: StreamEpoch,
}

/// The session of a connection no longer counts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ended {
    pub reason: EndReason,
    pub recovery: Option<RecoveryNotice>,
}

impl Ended {
    pub fn message(self) -> ServerMessage {
        ServerMessage::Ended {
            reason: self.reason,
            recovery: self.recovery,
        }
    }
}

#[derive(Debug)]
pub struct Delivery {
    pub position: i64,
    pub message: ServerMessage,
}

#[derive(Debug)]
pub enum Batch {
    Events {
        deliveries: Vec<Delivery>,
        /// The position that the connection has been served through once the deliveries are
        /// written.
        position: i64,
        /// True when the log can hold further events after `position`.
        more: bool,
        cursor: Cursor,
        /// Taken before the transaction of the batch read anything. Every decision of the
        /// batch is at most as old as this.
        read_at: Instant,
        /// The event at `position` is one that the account may receive and that the server
        /// cannot send. The client is told to resynchronise.
        skipped: bool,
    },
    Resync(ResyncReason),
    Ended(Ended),
}

#[derive(Debug)]
pub enum Start {
    Subscribed(Cursor),
    Resync(ResyncReason),
    Ended(Ended),
}

/// One event of the log that is addressed to the connection by account and class. A part
/// that is absent is too large or not readable by this version of the server.
struct Candidate {
    position: i64,
    recorded_at: OffsetDateTime,
    class: Option<Class>,
    audience: Option<Audience>,
    event: Option<Event>,
}

enum Decision {
    Deliver(Box<Delivery>),
    Hidden,
    /// The account may receive the event, or the server cannot tell, and the event cannot be
    /// sent.
    Unsendable,
}

/// Opens the caller of a connection as every request does. `Err(Ended)` when the session does
/// not count any more.
async fn open(
    connection: &mut PgConnection,
    who: &Opened,
    now: OffsetDateTime,
) -> ApiResult<Result<Access, Ended>> {
    match Access::open(connection, &who.bearer, now).await {
        Ok(access) => Ok(Ok(access)),
        Err(error) if error.code() == ErrorCode::Unauthenticated => {
            Ok(Err(why_ended(connection, who).await?))
        }
        Err(error) => Err(error),
    }
}

/// The reason is the one that the change stored when it ended the session (ADR 0208). A
/// session without a stored reason has expired, or was ended by a change that stores none;
/// the state of its account and its device then gives the reason.
async fn why_ended(connection: &mut PgConnection, who: &Opened) -> ApiResult<Ended> {
    type Stored = (
        String,
        Option<RecoveryId>,
        Option<DeviceId>,
        Option<AccountId>,
        Option<OffsetDateTime>,
    );
    let stored: Option<Stored> = sqlx::query_as(
        "select e.reason, r.id, r.device_id, r.performed_by, r.performed_at
         from ended_sessions e
         left join recoveries r on r.id = e.recovery_id
         where e.session_id = $1",
    )
    .bind(who.session_id)
    .fetch_optional(&mut *connection)
    .await?;
    let because = |reason| Ended {
        reason,
        recovery: None,
    };
    if let Some((reason, recovery_id, device_id, performed_by, performed_at)) = stored {
        return match (reason.as_str(), recovery_id, performed_at) {
            ("recovered", Some(id), Some(performed_at)) => Ok(Ended {
                reason: EndReason::Recovered,
                recovery: Some(RecoveryNotice {
                    id,
                    device_id,
                    performed_by,
                    performed_at,
                }),
            }),
            ("banned", ..) => Ok(because(EndReason::Banned)),
            ("removed", ..) => Ok(because(EndReason::Removed)),
            ("device_removed", ..) => Ok(because(EndReason::DeviceRemoved)),
            ("logged_out", ..) => Ok(because(EndReason::SessionEnded)),
            (other, ..) => Err(ApiError::internal(&format_args!(
                "unknown stored reason {other:?} of an ended session"
            ))),
        };
    }
    let status: Option<String> = sqlx::query_scalar("select status from accounts where id = $1")
        .bind(who.account_id)
        .fetch_optional(&mut *connection)
        .await?;
    match status.as_deref().map(AccountStatus::read).transpose()? {
        Some(AccountStatus::Banned) => return Ok(because(EndReason::Banned)),
        Some(AccountStatus::Removed) | None => return Ok(because(EndReason::Removed)),
        Some(AccountStatus::Active) => {}
    }
    let device_exists: bool =
        sqlx::query_scalar("select exists (select 1 from devices where id = $1)")
            .bind(who.device_id)
            .fetch_one(connection)
            .await?;
    Ok(because(if device_exists {
        EndReason::SessionEnded
    } else {
        EndReason::DeviceRemoved
    }))
}

/// Checks the session of a connection that has nothing to deliver.
pub async fn validate(app: &AppState, who: &Opened) -> ApiResult<Option<Ended>> {
    read!(app.unattended(), |connection| {
        Ok(open(connection, who, app.now()).await?.err())
    })
}

/// The place of the call that the account of a connection has a seat in, read in the
/// transaction of the batch. A background connection needs it: it widens the classes that
/// the connection reads, and a value that is older than the batch would let the connection
/// pass events of its own call for good (ADR 0510). A full connection reads every class.
async fn seat_of(
    connection: &mut PgConnection,
    account_id: AccountId,
    level: Level,
) -> ApiResult<Option<CallKey>> {
    match level {
        Level::Full => Ok(None),
        Level::Background => crate::calls::store::seat_key(connection, account_id).await,
    }
}

/// The classes that the SQL statements select for a connection: every class for a full
/// connection and for an account in a call, because the events of the call's channel are of
/// any class.
fn selected_classes(level: Level, in_call: Option<CallKey>) -> Vec<&'static str> {
    let classes = match in_call {
        Some(_) => Class::received_at(Level::Full),
        None => Class::received_at(level),
    };
    classes.into_iter().map(StoredText::stored).collect()
}

/// Every query binds the recipient account at $2 and the selected classes at $3.
fn addressed_query(before: &'static str, after: &'static str) -> sqlx::AssertSqlSafe<String> {
    sqlx::AssertSqlSafe(format!(
        "{before}
         and audience->>'to' is distinct from 'suppressed'
         and (audience->>'to' is distinct from 'notification' or exists (
             select 1 from notification_receivers nr where nr.event_id=events.id and nr.account_id=$2))
         and (audience_account_id is null or audience_account_id=$2)
         and class=any($3)
         {after}"
    ))
}

fn decide(
    access: &Access,
    standing: &Standing,
    level: Level,
    in_call: Option<CallKey>,
    candidate: Candidate,
) -> ApiResult<Decision> {
    let Some(audience) = &candidate.audience else {
        return Ok(Decision::Unsendable);
    };
    if let Audience::Notification {
        message_id,
        notice_id,
    } = audience
        && (!standing.notifications.contains_key(message_id)
            || !standing.notification_notices.contains_key(notice_id)
            || (level == Level::Background && !standing.notification_notices[notice_id].0))
    {
        return Ok(Decision::Hidden);
    }
    let alert = match audience {
        Audience::Notification { notice_id, .. } => standing
            .notification_notices
            .get(notice_id)
            .is_some_and(|(_, alert)| *alert),
        Audience::Mention { .. } => true,
        _ => matches!(candidate.event, Some(Event::CallInvited { .. })),
    };
    if standing.quiet && alert {
        return Ok(Decision::Hidden);
    }
    let wanted = candidate.class.is_none_or(|class| class.reaches(level))
        || (in_call.is_some() && audience.call_key() == in_call);
    let recipient = Recipient {
        account_id: access.account_id,
        actor: access.actor(),
        server_role_ids: &access.server_role_ids,
        fellows: &standing.fellows,
        contacts: &standing.contacts,
        listed: &standing.listed,
        memberships: &standing.memberships,
        calls: &standing.calls,
    };
    let in_workspace = |workspace_id| standing.workspaces.get(&workspace_id);
    if let Audience::SystemMessage {
        readers: Some(readers),
        ..
    } = audience
        && !readers.contains(&access.account_id)
    {
        return Ok(Decision::Hidden);
    }
    if let Audience::Mention {
        message_id,
        account_id,
        notice_id,
        ..
    } = audience
        && (account_id.is_some_and(|id| id != access.account_id)
            || notice_id.is_some_and(|id| !standing.mention_notices.contains(&id))
            || !standing.mentioned.contains(message_id))
    {
        return Ok(Decision::Hidden);
    }
    if !(wanted && receives(recipient, audience, &in_workspace)) {
        return Ok(Decision::Hidden);
    }
    match (candidate.class, candidate.event) {
        (Some(_), Some(event)) => Ok(Decision::Deliver(Box::new(Delivery {
            position: candidate.position,
            message: ServerMessage::Event {
                id: u64::try_from(candidate.position)
                    .map_err(|error| ApiError::internal(&error))?,
                at: candidate.recorded_at,
                event,
            },
        }))),
        _ => Ok(Decision::Unsendable),
    }
}

fn parsed<T: serde::de::DeserializeOwned>(stored: Option<Json<serde_json::Value>>) -> Option<T> {
    serde_json::from_value(stored?.0).ok()
}

type CountedRow = (i64, String, Option<Json<serde_json::Value>>);

/// Decides where a subscription starts, or that the client must load a snapshot (ADR 0207).
/// The decision counts the events after the cursor that the account may receive at the
/// level, so it does not depend on events that the account may not receive.
pub async fn subscribe(
    app: &AppState,
    who: &Opened,
    level: Level,
    cursor: Option<Cursor>,
) -> ApiResult<Start> {
    let most = app.config.limits.stream.replay_most_events;
    read!(app.unattended(), |connection| {
        let access = match open(connection, who, app.now()).await? {
            Ok(access) => access,
            Err(ended) => return Ok(Start::Ended(ended)),
        };
        let in_call = seat_of(connection, access.account_id, level).await?;
        let classes = selected_classes(level, in_call);
        let log = log::state(connection).await?;
        let Some(cursor) = cursor else {
            return Ok(Start::Subscribed(log.cursor(log.head)?));
        };
        let mut after = match i64::try_from(cursor.position) {
            Ok(position) if cursor.epoch == log.epoch && position <= log.head => position,
            _ => return Ok(Start::Resync(ResyncReason::UnknownCursor)),
        };
        if after < log.trimmed_through {
            return Ok(Start::Resync(ResyncReason::TooFarBehind));
        }
        let mut waiting = 0;
        loop {
            let rows: Vec<CountedRow> = sqlx::query_as(addressed_query(
                "select position, class,
                        case when octet_length(audience::text) <= $5 then audience end
                 from events
                 where position > $1",
                "order by position
                 limit $4",
            ))
            .bind(after)
            .bind(access.account_id)
            .bind(&classes)
            .bind(COUNTED_PER_READ)
            .bind(STORED_AUDIENCE_BYTES)
            .fetch_all(&mut *connection)
            .await?;
            let Some(last) = rows.last() else {
                break;
            };
            after = last.0;
            let full = i64::try_from(rows.len()).unwrap_or(i64::MAX) >= COUNTED_PER_READ;
            let candidates: Vec<Candidate> = rows
                .into_iter()
                .map(|(position, class, audience)| Candidate {
                    position,
                    recorded_at: OffsetDateTime::UNIX_EPOCH,
                    class: Class::from_stored(&class),
                    audience: parsed(audience),
                    event: None,
                })
                .collect();
            let standing = Standing::load(connection, &access, &candidates, app.now()).await?;
            for candidate in candidates {
                let decision = decide(&access, &standing, level, in_call, candidate)?;
                if !matches!(decision, Decision::Hidden) {
                    waiting += 1;
                }
            }
            if waiting > most {
                return Ok(Start::Resync(ResyncReason::TooFarBehind));
            }
            if !full {
                break;
            }
        }
        Ok(Start::Subscribed(cursor))
    })
}

type EventRow = (
    i64,
    OffsetDateTime,
    String,
    Option<Json<serde_json::Value>>,
    Option<Json<serde_json::Value>>,
    i64,
);

/// The last event in this committed transaction that the account may receive at full level.
/// The caller supplies one read transaction so access, the epoch and all batches agree.
pub(crate) async fn last_receivable(
    connection: &mut PgConnection,
    account: AccountId,
    first: i64,
    last: i64,
    now: OffsetDateTime,
) -> ApiResult<Option<Cursor>> {
    let Some(access) = Access::of_account(connection, account).await? else {
        return Ok(None);
    };
    let log = log::state(connection).await?;
    if first <= log.trimmed_through {
        return Err(ApiError::internal(
            &"request events were trimmed before its receipt was read",
        ));
    }
    let classes = selected_classes(Level::Full, None);
    let mut through = last;
    loop {
        let rows: Vec<EventRow> = sqlx::query_as(addressed_query(
"select position, recorded_at, class,
                    case when octet_length(audience::text) <= $5 then audience end,
                    case when octet_length(audience::text) <= $5 and octet_length(payload::text) <= $6 then payload end,
                    0::bigint
             from events where position >= $1 and position <= $4",
"order by position desc limit 64",
)).bind(first).bind(account).bind(&classes).bind(through)
            .bind(STORED_AUDIENCE_BYTES).bind(STORED_PAYLOAD_BYTES)
            .fetch_all(&mut *connection).await?;
        let Some(oldest) = rows.last() else {
            return Ok(None);
        };
        through = oldest.0 - 1;
        let candidates: Vec<Candidate> = rows
            .into_iter()
            .map(
                |(position, recorded_at, class, audience, payload, _)| Candidate {
                    position,
                    recorded_at,
                    class: Class::from_stored(&class),
                    audience: parsed(audience),
                    event: parsed(payload),
                },
            )
            .collect();
        let standing = Standing::load(connection, &access, &candidates, now).await?;
        for candidate in candidates {
            match decide(&access, &standing, Level::Full, None, candidate)? {
                Decision::Deliver(delivery) => return Ok(Some(log.cursor(delivery.position)?)),
                Decision::Hidden => {}
                Decision::Unsendable => {
                    return Err(ApiError::internal(&"a request event cannot be delivered"));
                }
            }
        }
        if through < first {
            return Ok(None);
        }
    }
}

/// Reads the next events after the position of a subscription and keeps the ones that the
/// account may receive now. A batch has at most `most` rows and a budget of bytes, and a part
/// of a row above its largest size is not loaded.
pub async fn next_batch(
    app: &AppState,
    who: &Opened,
    subscription: Subscription,
    most: u32,
) -> ApiResult<Batch> {
    let limits = &app.config.limits.stream;
    let size = i64::from(most.clamp(1, limits.batch.max(1)));
    let budget = i64::from(limits.batch_bytes.max(1));
    read!(app.unattended(), |connection| {
        let read_at = Instant::now();
        let access = match open(connection, who, app.now()).await? {
            Ok(access) => access,
            Err(ended) => return Ok(Batch::Ended(ended)),
        };
        let in_call = seat_of(connection, access.account_id, subscription.level).await?;
        let classes = selected_classes(subscription.level, in_call);
        let log = log::state(connection).await?;
        if subscription.epoch != log.epoch {
            return Ok(Batch::Resync(ResyncReason::UnknownCursor));
        }
        if subscription.position < log.trimmed_through {
            return Ok(Batch::Resync(ResyncReason::TooFarBehind));
        }
        let rows: Vec<EventRow> = sqlx::query_as(addressed_query(
            "select position, recorded_at, class,
                    case when audience_bytes <= $6 then audience end,
                    case when audience_bytes <= $6 and payload_bytes <= $7 then payload end,
                    total
             from (
                 select *, count(*) over () as total,
                        sum(least(audience_bytes, $6) + least(payload_bytes, $7))
                            over (order by position) as bytes_through
                 from (
                     select position, recorded_at, class, audience, payload,
                            octet_length(audience::text) as audience_bytes,
                            octet_length(payload::text) as payload_bytes
                     from events
                     where position > $1",
            "order by position
                     limit $4
                 ) as addressed
             ) as sized
             where bytes_through - least(audience_bytes, $6) - least(payload_bytes, $7) < $5
             order by position",
        ))
        .bind(subscription.position)
        .bind(access.account_id)
        .bind(&classes)
        .bind(size)
        .bind(budget)
        .bind(STORED_AUDIENCE_BYTES)
        .bind(STORED_PAYLOAD_BYTES)
        .fetch_all(&mut *connection)
        .await?;
        let read = i64::try_from(rows.len()).unwrap_or(i64::MAX);
        let addressed = rows.first().map_or(0, |row| row.5);
        let mut more = addressed >= size || read < addressed;
        let mut position = match rows.last() {
            Some(last) if more => last.0,
            _ => log.head.max(subscription.position),
        };
        let candidates: Vec<Candidate> = rows
            .into_iter()
            .map(
                |(position, recorded_at, class, audience, payload, _)| Candidate {
                    position,
                    recorded_at,
                    class: Class::from_stored(&class),
                    audience: parsed(audience),
                    event: parsed(payload),
                },
            )
            .collect();
        let standing = Standing::load(connection, &access, &candidates, app.now()).await?;

        let mut deliveries = Vec::new();
        let mut skipped = false;
        for candidate in candidates {
            let at = candidate.position;
            match decide(&access, &standing, subscription.level, in_call, candidate)? {
                Decision::Deliver(delivery) => deliveries.push(*delivery),
                Decision::Hidden => {}
                Decision::Unsendable => {
                    tracing::error!(position = at, "an event of the log cannot be sent");
                    position = at;
                    more = false;
                    skipped = true;
                    break;
                }
            }
        }
        Ok(Batch::Events {
            deliveries,
            position,
            more,
            cursor: log.cursor(position)?,
            read_at,
            skipped,
        })
    })
}

/// The standing of one account in the workspaces and channels that a batch names, read in
/// the transaction of the batch.
struct Standing {
    quiet: bool,
    workspaces: HashMap<WorkspaceId, WorkspaceStanding>,
    fellows: HashSet<AccountId>,
    contacts: HashMap<AccountId, junto_authz::Contact>,
    listed: HashSet<Uuid>,
    /// The workspaces, of the ones that the invites of the batch name, that the account is
    /// a member of. Read only for an account that holds `create_invites`.
    memberships: HashSet<WorkspaceId>,
    calls: HashMap<CallId, CallStanding>,
    mentioned: HashSet<junto_protocol::ids::MessageId>,
    mention_notices: HashSet<Uuid>,
    notification_notices: HashMap<Uuid, (bool, bool)>,
    notifications:
        HashMap<junto_protocol::ids::MessageId, junto_protocol::discovery::NotificationKind>,
}

impl Standing {
    async fn load(
        connection: &mut PgConnection,
        access: &Access,
        candidates: &[Candidate],
        now: OffsetDateTime,
    ) -> ApiResult<Self> {
        let notification_events: Vec<_> = candidates
            .iter()
            .filter_map(|candidate| match &candidate.audience {
                Some(Audience::Notification { notice_id, .. }) => Some(*notice_id),
                _ => None,
            })
            .collect();
        let notification_notices: HashMap<Uuid, (bool, bool)> = if notification_events.is_empty() {
            HashMap::new()
        } else {
            let rows: Vec<(Uuid, bool, bool)> = sqlx::query_as("select event_id,background,alert from notification_receivers where event_id=any($1) and account_id=$2")
                .bind(notification_events)
                .bind(access.account_id)
                .fetch_all(&mut *connection)
                .await?;
            rows.into_iter()
                .map(|(id, background, alert)| (id, (background, alert)))
                .collect()
        };
        let notification_ids: Vec<_> = candidates
            .iter()
            .filter_map(|candidate| match &candidate.audience {
                Some(Audience::Notification {
                    message_id,
                    notice_id,
                }) if notification_notices.contains_key(notice_id) => Some(*message_id),
                _ => None,
            })
            .collect();
        let notifications = if notification_ids.is_empty() {
            HashMap::new()
        } else {
            crate::messages::notifications::visible(
                connection,
                access,
                now,
                Some(&notification_ids),
                None,
                notification_ids.len() as i64,
            )
            .await?
            .into_iter()
            .map(|notice| (notice.message_id, notice.kind))
            .collect()
        };
        let notice_ids: Vec<_> = candidates
            .iter()
            .filter_map(|candidate| match &candidate.audience {
                Some(Audience::Mention { notice_id, .. }) => *notice_id,
                _ => None,
            })
            .collect();
        let mention_notices: HashSet<Uuid> = sqlx::query_scalar("select event_id from message_mention_receivers where event_id=any($1) and account_id=$2")
            .bind(notice_ids)
            .bind(access.account_id)
            .fetch_all(&mut *connection)
            .await?
            .into_iter()
            .collect();
        // Only a mention that names the account or lists it as a receiver can be
        // delivered; the others are decided without the message. One statement reads the
        // rest, so the cost of a batch does not grow with the mentions of other accounts.
        let mut ids: Vec<junto_protocol::ids::MessageId> = candidates
            .iter()
            .filter_map(|c| match c.audience.as_ref() {
                Some(Audience::Mention {
                    message_id,
                    account_id,
                    notice_id,
                    ..
                }) if account_id.is_none_or(|id| id == access.account_id)
                    && notice_id.is_none_or(|id| mention_notices.contains(&id)) =>
                {
                    Some(*message_id)
                }
                _ => None,
            })
            .collect();
        ids.sort_unstable();
        ids.dedup();
        let mentioned: HashSet<junto_protocol::ids::MessageId> = if ids.is_empty() {
            HashSet::new()
        } else {
            crate::messages::mentioned_among(connection, access.account_id, &ids, now)
                .await?
                .into_iter()
                .collect()
        };
        let mut named: Vec<(WorkspaceId, Vec<ChannelId>)> = Vec::new();
        let places = candidates
            .iter()
            .filter_map(|event| event.audience.as_ref())
            .flat_map(Audience::places);
        for (workspace_id, channel_id) in places {
            let index = match named.iter().position(|(id, _)| *id == workspace_id) {
                Some(index) => index,
                None => {
                    named.push((workspace_id, Vec::new()));
                    named.len() - 1
                }
            };
            if let Some(channel_id) = channel_id
                && !named[index].1.contains(&channel_id)
            {
                named[index].1.push(channel_id);
            }
        }
        let viewed: Vec<AccountId> = candidates
            .iter()
            .filter_map(|event| event.audience.as_ref()?.viewed_account_id())
            .collect();
        let fellows = if viewed.is_empty() {
            HashSet::new()
        } else {
            crate::store::fellow_members(connection, access.account_id, &viewed).await?
        };
        let mut contacts = HashMap::new();
        let reached = candidates
            .iter()
            .filter_map(|event| event.audience.as_ref())
            .flat_map(|audience| audience.reached_account_ids(access.account_id));
        for other in reached {
            if !contacts.contains_key(&other)
                && let Some(contact) = crate::store::contact(connection, access, other).await?
            {
                contacts.insert(other, contact);
            }
        }
        let lists: Vec<Uuid> = candidates
            .iter()
            .filter_map(|event| event.audience.as_ref()?.receiver_list())
            .collect();
        let listed = if lists.is_empty() {
            HashSet::new()
        } else {
            sqlx::query_scalar(RECEIVER_LISTS)
                .bind(&lists)
                .bind(access.account_id)
                .fetch_all(&mut *connection)
                .await?
                .into_iter()
                .collect()
        };
        let mut invited: Vec<WorkspaceId> = candidates
            .iter()
            .filter_map(|event| event.audience.as_ref())
            .flat_map(|audience| audience.invite_workspace_ids().iter().copied())
            .collect();
        invited.sort_unstable();
        invited.dedup();
        let inviter = junto_authz::Request::Server(junto_authz::ServerPermission::CreateInvites);
        let memberships = if invited.is_empty() || !access.allows(inviter) {
            HashSet::new()
        } else {
            crate::access::memberships_among(connection, access.account_id, &invited).await?
        };
        let mut call_ids: Vec<CallId> = candidates
            .iter()
            .filter_map(|event| event.audience.as_ref()?.inside_of_call())
            .collect();
        call_ids.sort_unstable();
        call_ids.dedup();
        let calls = if call_ids.is_empty() {
            HashMap::new()
        } else {
            let rows: Vec<(CallId, bool, bool)> = sqlx::query_as(
                "select call_id, hidden, is_inside_call(call_id, $2)
                 from call_message_privacy where call_id = any($1)",
            )
            .bind(&call_ids)
            .bind(access.account_id)
            .fetch_all(&mut *connection)
            .await?;
            rows.into_iter()
                .map(|(call_id, hides_occupants, inside)| {
                    let standing = CallStanding {
                        hides_occupants,
                        inside,
                    };
                    (call_id, standing)
                })
                .collect()
        };
        let mut workspaces = HashMap::new();
        for (workspace_id, channel_ids) in named {
            let scope = match access.workspace(connection, workspace_id).await {
                Ok(scope) => scope,
                Err(error) if error.code() == ErrorCode::NotFound => continue,
                Err(error) => return Err(error),
            };
            let everyone_role_id: WorkspaceRoleId = sqlx::query_scalar(
                "select id from workspace_roles where workspace_id = $1 and is_everyone",
            )
            .bind(workspace_id)
            .fetch_one(&mut *connection)
            .await?;
            let channels = if channel_ids.is_empty() {
                HashMap::new()
            } else {
                access
                    .named_channels(connection, workspace_id, &scope, &channel_ids)
                    .await?
                    .into_iter()
                    .map(|(record, scope)| {
                        let standing = ChannelStanding {
                            deleted: record.deleted_at.is_some(),
                            scope,
                        };
                        (record.channel.id, standing)
                    })
                    .collect()
            };
            workspaces.insert(
                workspace_id,
                WorkspaceStanding {
                    scope,
                    everyone_role_id,
                    channels,
                },
            );
        }
        Ok(Self {
            quiet: candidates.iter().any(|candidate| {
                matches!(
                    candidate.audience,
                    Some(Audience::Mention { .. } | Audience::Notification { .. })
                ) || matches!(candidate.event, Some(Event::CallInvited { .. }))
            }) && crate::messages::notifications::quiet(connection, access.account_id, now)
                .await?,
            workspaces,
            fellows,
            contacts,
            listed,
            memberships,
            calls,
            mentioned,
            mention_notices,
            notification_notices,
            notifications,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_call_selects_every_class_and_a_level_selects_its_own() {
        let channel = Some(CallKey::Channel(ChannelId::generate()));
        assert_eq!(selected_classes(Level::Background, channel).len(), 5);
        assert_eq!(selected_classes(Level::Full, None).len(), 5);
        assert_eq!(
            selected_classes(Level::Background, None),
            ["control", "direct_message", "mention", "call_invite"]
        );
    }
}
