//! What one connection may receive next. Each decision reads the log, the session and the
//! roles in one read-only transaction that starts immediately before the messages are written
//! to the socket (ADR 0011).

use std::collections::{HashMap, HashSet};

use junto_protocol::account::{AccountStatus, RecoveryNotice};
use junto_protocol::error::ErrorCode;
use junto_protocol::events::{Cursor, EndReason, Event, Level, ResyncReason, ServerMessage};
use junto_protocol::ids::{
    AccountId, ChannelId, DeviceId, RecoveryId, StreamEpoch, WorkspaceId, WorkspaceRoleId,
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
    Audience, ChannelStanding, Class, Recipient, WorkspaceStanding, receives,
};
use crate::events::hub::CallKey;
use crate::events::log::{self, STORED_AUDIENCE_BYTES, STORED_PAYLOAD_BYTES};
use crate::extract::Bearer;
use crate::read;
use crate::state::AppState;

const COUNTED_PER_READ: i64 = 500;

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
    let wanted = candidate.class.is_none_or(|class| class.reaches(level))
        || (in_call.is_some() && audience.call_key() == in_call);
    let recipient = Recipient {
        account_id: access.account_id,
        actor: access.actor(),
        server_role_ids: &access.server_role_ids,
        fellows: &standing.fellows,
        contacts: &standing.contacts,
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
    let in_call = app.streams.calls().key_of(who.account_id);
    let classes = selected_classes(level, in_call);
    read!(app.unattended(), |connection| {
        let access = match open(connection, who, app.now()).await? {
            Ok(access) => access,
            Err(ended) => return Ok(Start::Ended(ended)),
        };
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
            let rows: Vec<CountedRow> = sqlx::query_as(
                "select position, class,
                        case when octet_length(audience::text) <= $5 then audience end
                 from events
                 where position > $1
                   and (audience_account_id is null or audience_account_id = $2)
                   and class = any($3)
                 order by position
                 limit $4",
            )
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
            let standing = Standing::load(connection, &access, &candidates).await?;
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
    let in_call = app.streams.calls().key_of(who.account_id);
    let classes = selected_classes(subscription.level, in_call);
    read!(app.unattended(), |connection| {
        let read_at = Instant::now();
        let access = match open(connection, who, app.now()).await? {
            Ok(access) => access,
            Err(ended) => return Ok(Batch::Ended(ended)),
        };
        let log = log::state(connection).await?;
        if subscription.epoch != log.epoch {
            return Ok(Batch::Resync(ResyncReason::UnknownCursor));
        }
        if subscription.position < log.trimmed_through {
            return Ok(Batch::Resync(ResyncReason::TooFarBehind));
        }
        let rows: Vec<EventRow> = sqlx::query_as(
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
                     where position > $1
                       and (audience_account_id is null or audience_account_id = $2)
                       and class = any($3)
                     order by position
                     limit $4
                 ) as addressed
             ) as sized
             where bytes_through - least(audience_bytes, $6) - least(payload_bytes, $7) < $5
             order by position",
        )
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
        let standing = Standing::load(connection, &access, &candidates).await?;

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
    workspaces: HashMap<WorkspaceId, WorkspaceStanding>,
    fellows: HashSet<AccountId>,
    contacts: HashMap<AccountId, junto_authz::Contact>,
    mentioned: HashSet<junto_protocol::ids::MessageId>,
    mention_notices: HashSet<Uuid>,
}

impl Standing {
    async fn load(
        connection: &mut PgConnection,
        access: &Access,
        candidates: &[Candidate],
    ) -> ApiResult<Self> {
        let notice_ids: Vec<_> = candidates
            .iter()
            .filter_map(|candidate| match &candidate.audience {
                Some(Audience::Mention { notice_id, .. }) => *notice_id,
                _ => None,
            })
            .collect();
        let mention_notices = sqlx::query_scalar("select event_id from message_mention_receivers where event_id=any($1) and account_id=$2")
            .bind(notice_ids).bind(access.account_id).fetch_all(&mut *connection).await?.into_iter().collect();
        let ids: Vec<junto_protocol::ids::MessageId> = candidates
            .iter()
            .filter_map(|c| match c.audience.as_ref() {
                Some(Audience::Mention { message_id, .. }) => Some(*message_id),
                _ => None,
            })
            .collect();
        let mut mentioned = HashSet::new();
        for id in ids {
            if crate::messages::is_mentioned(connection, access.account_id, id).await? {
                mentioned.insert(id);
            }
        }
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
                let inside: Vec<ChannelId> = sqlx::query_scalar(
                    "select c.channel_id
                     from call_members m
                     join calls c on c.id = m.call_id
                     where m.account_id = $1 and c.channel_id = any($2)",
                )
                .bind(access.account_id)
                .bind(&channel_ids)
                .fetch_all(&mut *connection)
                .await?;
                access
                    .named_channels(connection, workspace_id, &scope, &channel_ids)
                    .await?
                    .into_iter()
                    .map(|(record, scope)| {
                        let standing = ChannelStanding {
                            deleted: record.deleted_at.is_some(),
                            hides_occupants: record.channel.hides_occupants,
                            inside: inside.contains(&record.channel.id),
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
            workspaces,
            fellows,
            contacts,
            mentioned,
            mention_notices,
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
