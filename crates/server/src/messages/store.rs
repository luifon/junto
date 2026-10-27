//! Message content, page loading, unread summaries and system lines.
use super::{Place, Purpose, uploads};
use crate::{
    access::Access,
    error::{ApiError, ApiResult},
    events,
};
use junto_protocol::{
    events::Event,
    ids::{ChannelId, MessageId},
    message::{Document, Message, MessagePlace, MessageState, Reaction, SystemMessage},
};
use sqlx::{PgConnection, Row, types::Json};
use time::OffsetDateTime;
use uuid::Uuid;

pub async fn changed(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    place: MessagePlace,
    message_id: MessageId,
    position: u64,
    deleted: bool,
) -> ApiResult<()> {
    let event = Event::MessageChanged {
        place,
        message_id,
        position,
        deleted,
    };
    let audience: Option<(
        Option<junto_protocol::ids::CallId>,
        Option<Vec<junto_protocol::ids::AccountId>>,
        OffsetDateTime,
    )> = sqlx::query_as("select call_id,readers,created_at from messages where id=$1")
        .bind(message_id)
        .fetch_optional(&mut *connection)
        .await?;
    let audience = if let MessagePlace::Channel {
        workspace_id,
        channel_id,
    } = place
    {
        let (call_id, readers, created_at) = audience.ok_or_else(|| {
            ApiError::internal(&"a channel message event needs its creation time")
        })?;
        match call_id {
            Some(call_id) => events::audience::Audience::SystemMessage {
                workspace_id,
                channel_id,
                call_id,
                readers,
                created_at: Some(created_at),
            },
            None => events::audience::Audience::ChannelMessage {
                workspace_id,
                channel_id,
                created_at,
            },
        }
    } else {
        events::audience::Audience::fixed_by(&event)
            .ok_or_else(|| ApiError::internal(&"a message event needs its audience"))?
    };
    events::log::record(connection, now, &audience, &event).await?;
    Ok(())
}

pub const VISIBLE: &str = "(m.readers is null or $2=any(m.readers))";

pub async fn get(
    connection: &mut PgConnection,
    access: &Access,
    id: MessageId,
    purpose: Purpose,
) -> ApiResult<(Place, Message)> {
    let row = sqlx::query(sqlx::AssertSqlSafe(format!("select m.*, a.name as display_name from messages m left join accounts a on a.id=m.author_id where m.id=$1
             and {VISIBLE}")))
        .bind(id)
        .bind(access.account_id)
        .fetch_optional(&mut *connection)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let place = Place::load(connection, access, row.try_get("place_id")?, purpose).await?;
    if let Purpose::Live(since) = purpose {
        let created_at: OffsetDateTime = row.try_get("created_at")?;
        if !since.is_some_and(|since| created_at > since) {
            place.require(access, junto_authz::WorkspacePermission::ReadMessageHistory)?;
        }
    }
    let mut messages = load_messages(connection, access, &place, &[id]).await?;
    Ok((place, messages.pop().ok_or_else(ApiError::not_found)?))
}

/// The first authors of the replies of the thread of the message `m`: at most three, in the
/// order of their first reply (ADR 0413).
pub const REPLY_AUTHORS: &str = "select ra.account_id from thread_reply_authors ra
     where ra.root_id=m.id order by ra.first_position,ra.account_id limit 3";

/// Loads a history page in three statements, independent of its number of messages.
pub async fn load_messages(
    connection: &mut PgConnection,
    access: &Access,
    place: &Place,
    ids: &[MessageId],
) -> ApiResult<Vec<Message>> {
    let rows = sqlx::query(sqlx::AssertSqlSafe(format!("select m.*,a.name as display_name, exists(select 1 from message_pins pin where pin.message_id=m.id) as
             pinned, case when exists(select 1 from messages target where target.id=m.reply_to and (target.readers is
             null or $2=any(target.readers))) then m.reply_to end as visible_reply_to,
             array({REPLY_AUTHORS}) as reply_authors from messages m left join
             accounts a on a.id=m.author_id where m.id=any($1) order by m.position,m.id")))
        .bind(ids)
        .bind(access.account_id)
        .fetch_all(&mut *connection)
        .await?;
    let reactions: Vec<(MessageId, String, i64, bool)> = sqlx::query_as("select message_id,emoji,count(*),bool_or(account_id=$2) from message_reactions where message_id=any($1)
             group by message_id,emoji order by emoji")
        .bind(ids)
        .bind(access.account_id)
        .fetch_all(&mut *connection)
        .await?;
    let mut attachments = uploads::of_messages(connection, ids).await?;
    rows.into_iter()
        .map(|row| {
            let id: MessageId = row.try_get("id")?;
            let mut system = row
                .try_get::<Option<Json<SystemMessage>>, _>("system")?
                .map(|value| value.0);
            if let Some(system) = &mut system {
                system.recording_id = row.try_get("recording_id")?;
            }
            Ok(Message {
                id,
                place: place.value,
                position: row.try_get::<i64, _>("position")? as u64,
                author_id: row.try_get("author_id")?,
                author_name: row.try_get("display_name")?,
                created_at: row.try_get("created_at")?,
                edited_at: row.try_get("edited_at")?,
                document: row
                    .try_get::<Option<Json<Document>>, _>("document")?
                    .map(|value| value.0),
                system,
                reply_to: row.try_get("visible_reply_to")?,
                permanent: row.try_get("permanent")?,
                deleted: row.try_get("deleted")?,
                attachments: attachments.remove(&id).unwrap_or_default(),
                thread_root: row.try_get("thread_root")?,
                reply_count: row.try_get::<i64, _>("reply_count")? as u64,
                last_reply: row.try_get("last_reply")?,
                pinned: row.try_get("pinned")?,
                files_removed_by_retention: row.try_get("files_removed_by_retention")?,
                reply_authors: row.try_get("reply_authors")?,
                reactions: reactions
                    .iter()
                    .filter(|(message, _, _, _)| *message == id)
                    .map(|(_, emoji, count, own)| Reaction {
                        emoji: emoji.clone(),
                        count: *count as u32,
                        own: *own,
                    })
                    .collect(),
            })
        })
        .collect()
}

pub async fn state(
    connection: &mut PgConnection,
    access: &Access,
    place: &Place,
    now: OffsetDateTime,
) -> ApiResult<MessageState> {
    let states = states(connection, access, &[place.value], now).await?;
    states.into_iter().next().ok_or_else(ApiError::not_found)
}

/// Aggregates every visible place together, reusing snapshot authorization.
async fn states(
    connection: &mut PgConnection,
    access: &Access,
    places: &[MessagePlace],
    now: OffsetDateTime,
) -> ApiResult<Vec<MessageState>> {
    let ids: Vec<Uuid> = places
        .iter()
        .map(|place| match place {
            MessagePlace::Channel { channel_id, .. } => channel_id.as_uuid(),
            MessagePlace::Direct {
                conversation_id, ..
            } => conversation_id.as_uuid(),
        })
        .collect();
    let query = state_query();
    let rows: Vec<(Uuid, i64, i64, i64, i64)> = sqlx::query_as(sqlx::AssertSqlSafe(query))
        .bind(&ids)
        .bind(access.account_id)
        .fetch_all(&mut *connection)
        .await?;
    let selected = mention_rows(true);
    let policy_query = format!("select p.id,policy.level,policy.muted,
        (select count(*) from (select * from ({selected}) candidates limit 100) bounded)
        from unnest($1::uuid[]) p(id) left join message_reads r on r.place_id=p.id and r.account_id=$2
        left join message_places floor on floor.id=p.id cross join lateral notification_policy($2,p.id,$3) policy");
    let policies: Vec<(Uuid, String, bool, i64)> =
        sqlx::query_as(sqlx::AssertSqlSafe(policy_query))
            .bind(&ids)
            .bind(access.account_id)
            .bind(now)
            .fetch_all(connection)
            .await?;
    let mut values: std::collections::HashMap<_, _> = rows
        .into_iter()
        .map(|(id, last, latest, unread, mentions)| (id, (last, latest, unread, mentions)))
        .collect();
    Ok(places
        .iter()
        .zip(ids)
        .filter_map(|(place, id)| {
            values
                .remove(&id)
                .map(|(last, latest, unread, mentions)| MessageState {
                    place: *place,
                    last_read: last as u64,
                    latest: latest as u64,
                    unread: unread as u64,
                    mentions: mentions as u64,
                    muted: policies
                        .iter()
                        .find(|(subject, ..)| *subject == id)
                        .is_some_and(|(_, _, muted, _)| *muted),
                    unread_sign: policies
                        .iter()
                        .find(|(subject, ..)| *subject == id)
                        .is_some_and(|(_, level, muted, mentions)| {
                            !muted
                                && ((level == "all" && unread > 0)
                                    || (level == "mentions" && *mentions > 0))
                        }),
                    notification_mentions: policies
                        .iter()
                        .find(|(subject, ..)| *subject == id)
                        .map_or(0, |(_, level, muted, mentions)| {
                            if *muted || level == "nothing" {
                                0
                            } else {
                                *mentions as u64
                            }
                        }),
                })
        })
        .collect())
}

pub async fn snapshot(
    connection: &mut PgConnection,
    access: &Access,
    workspaces: &[junto_protocol::snapshot::WorkspaceState],
    now: OffsetDateTime,
) -> ApiResult<Vec<MessageState>> {
    let mut places: Vec<MessagePlace> = workspaces
        .iter()
        .flat_map(|workspace| {
            workspace
                .channels
                .iter()
                .filter(|channel| {
                    workspace
                        .channel_permissions
                        .iter()
                        .find(|entry| entry.channel_id == channel.id)
                        .map_or(workspace.permissions.as_slice(), |entry| {
                            entry.permissions.as_slice()
                        })
                        .contains(&junto_authz::WorkspacePermission::ReadMessageHistory)
                })
                .map(|channel| MessagePlace::Channel {
                    workspace_id: workspace.workspace.id,
                    channel_id: channel.id,
                })
        })
        .collect();
    for conversation in Place::conversations(connection, access).await? {
        places.push(MessagePlace::Direct {
            conversation_id: conversation.id,
            low_account_id: conversation.low_account_id,
            high_account_id: conversation.high_account_id,
        });
    }
    states(connection, access, &places, now).await
}

/// The actions whose line states only that a person changed settings of the channel. These
/// lines are public. The line is not written again while the latest public line of the
/// channel is the same action by the same person, written less than [`JOINED_LINES_WITHIN`]
/// earlier (ADR 0408).
const JOINED_ACTIONS: [&str; 3] = [
    "channel_updated",
    "channel_override_set",
    "channel_call_settings_updated",
];
pub const JOINED_LINES_WITHIN: time::Duration = time::Duration::minutes(5);

/// Whether this line repeats the latest line of its place, under the lock of the place, so
/// that two changes at the same time do not both find no repeat.
async fn repeats_latest(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    place: &Place,
    system: &SystemMessage,
) -> ApiResult<bool> {
    let Some(actor_id) = system.actor_id else {
        return Ok(false);
    };
    if !JOINED_ACTIONS.contains(&system.action.as_str()) {
        return Ok(false);
    }
    // A place without its row has no line to repeat. The row is not created here: an insert
    // would wait for a concurrent writer of the place before the lock below does.
    let locked: Option<i32> =
        sqlx::query_scalar("select 1 from message_places where id=$1 for update")
            .bind(place.id)
            .fetch_optional(&mut *connection)
            .await?;
    if locked.is_none() {
        return Ok(false);
    }
    Ok(sqlx::query_scalar(
        "select coalesce((select system->>'action'=$2 and (system->>'actor_id')::uuid=$3
             and created_at>$4
             from messages where place_id=$1 and thread_root is null and readers is null
             order by position desc,created_at desc,id desc limit 1),false)",
    )
    .bind(place.id)
    .bind(&system.action)
    .bind(actor_id)
    .bind(now - JOINED_LINES_WITHIN)
    .fetch_one(connection)
    .await?)
}

pub async fn system(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    channel: ChannelId,
    id: MessageId,
    system: &SystemMessage,
    call_id: Option<junto_protocol::ids::CallId>,
) -> ApiResult<()> {
    let workspace =
        sqlx::query_scalar("select workspace_id from channels where id=$1 and deleted_at is null")
            .bind(channel)
            .fetch_optional(&mut *connection)
            .await?;
    let Some(workspace_id) = workspace else {
        return Ok(());
    };
    let place = Place {
        id: channel.as_uuid(),
        value: MessagePlace::Channel {
            workspace_id,
            channel_id: channel,
        },
        scope: None,
        archived: false,
    };
    if call_id.is_none() && repeats_latest(connection, now, &place, system).await? {
        return Ok(());
    }
    let readers: Option<Vec<junto_protocol::ids::AccountId>> = if let Some(call_id) = call_id {
        let active: bool = sqlx::query_scalar("select exists(select 1 from calls where id=$1)")
            .bind(call_id)
            .fetch_one(&mut *connection)
            .await?;
        if active {
            sqlx::query("select refresh_call_message_privacy($1)")
                .bind(call_id)
                .execute(&mut *connection)
                .await?;
        }
        sqlx::query_scalar("select case when hidden then case when cardinality(readers)>0 then readers else array(select account_id
             from call_message_members where call_id=$1 order by account_id) end end from call_message_privacy where
             call_id=$1")
            .bind(call_id)
            .fetch_optional(&mut *connection)
            .await?
            .flatten()
    } else {
        None
    };
    let position = if readers.is_some() {
        place.ensure(connection).await?;
        sqlx::query_scalar("select head from message_places where id=$1 for update")
            .bind(place.id)
            .fetch_one(&mut *connection)
            .await?
    } else {
        place.next(connection).await?
    };
    sqlx::query(
        "insert into messages(id,place_id,position,created_at,system,call_id,recording_id,readers)
             values($1,$2,$3,$4,$5,$6,$7,$8)",
    )
    .bind(id)
    .bind(place.id)
    .bind(position)
    .bind(now)
    .bind(Json(system))
    .bind(call_id)
    .bind(system.recording_id)
    .bind(&readers)
    .execute(&mut *connection)
    .await?;
    if let Some(call_id) = call_id {
        events::log::record(
            connection,
            now,
            &events::audience::Audience::SystemMessage {
                workspace_id,
                channel_id: channel,
                call_id,
                readers,
                created_at: Some(now),
            },
            &Event::MessageChanged {
                place: place.value,
                message_id: id,
                position: position as u64,
                deleted: false,
            },
        )
        .await
    } else {
        changed(connection, now, place.value, id, position as u64, false).await
    }
}

pub async fn remove(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    place: &Place,
    message: &Message,
) -> ApiResult<()> {
    changed(
        connection,
        now,
        place.value,
        message.id,
        message.position,
        true,
    )
    .await?;
    erase(connection, now, message.id).await?;
    if let Some(root) = message.thread_root {
        let position: i64 = sqlx::query_scalar("select position from messages where id=$1")
            .bind(root)
            .fetch_one(&mut *connection)
            .await?;
        changed(connection, now, place.value, root, position as u64, false).await?;
    }
    if message.permanent {
        sqlx::query(
            "update channels set permanent_message_count=permanent_message_count-1
             where id=$1",
        )
        .bind(place.id)
        .execute(&mut *connection)
        .await?;
    }
    Ok(())
}

/// Erases content while retaining the marker required by replies and future threads.
pub(crate) async fn erase(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    id: MessageId,
) -> ApiResult<()> {
    crate::exports::message_deleted(connection, now, id).await?;
    uploads::remove_message(connection, now, id).await?;
    sqlx::query("delete from message_pins where message_id=$1")
        .bind(id)
        .execute(&mut *connection)
        .await?;
    sqlx::query("delete from notifications where message_id=$1")
        .bind(id)
        .execute(&mut *connection)
        .await?;
    sqlx::query("delete from message_mentions where message_id=$1")
        .bind(id)
        .execute(&mut *connection)
        .await?;
    sqlx::query("delete from message_mention_accounts where message_id=$1")
        .bind(id)
        .execute(&mut *connection)
        .await?;
    sqlx::query("delete from message_reactions where message_id=$1")
        .bind(id)
        .execute(&mut *connection)
        .await?;
    let has_replies: bool = sqlx::query_scalar(
        "select exists(select 1 from messages where reply_to=$1 or thread_root=$1)",
    )
    .bind(id)
    .fetch_one(&mut *connection)
    .await?;
    if has_replies {
        sqlx::query("update messages set
             document=null,system=null,author_id=null,edited_at=null,permanent=false,deleted=true,recording_id=null,notifies=null
             where id=$1")
            .bind(id)
            .execute(&mut *connection)
            .await?;
    } else {
        sqlx::query("delete from messages where id=$1")
            .bind(id)
            .execute(&mut *connection)
            .await?;
    }
    Ok(())
}

fn mention_rows(policy: bool) -> String {
    let eligible = if policy {
        "and exists(select 1 from message_mentions mm left join workspace_roles wr on wr.id=mm.role_id
        where mm.message_id=ma.message_id and $2=any(mm.recipients) and (mm.account_id=$2
        or (policy.everyone and (mm.everyone or wr.is_everyone))
        or (policy.roles and mm.role_id is not null and not coalesce(wr.is_everyone,false))))"
    } else {
        ""
    };
    format!("(select ma.message_id from message_mention_accounts ma
        where ma.place_id=p.id and ma.account_id=$2 and ma.thread_root is null
        and ma.position>greatest(coalesce(r.position,0),coalesce(floor.read_floor,0)) {eligible}
        order by ma.position limit 100)
        union all (select ma.message_id from message_mention_accounts ma
        where ma.place_id=p.id and ma.account_id=$2 and ma.thread_root is not null and ma.unread {eligible}
        order by ma.position limit 100)")
}

pub fn state_query() -> String {
    let selected = mention_rows(false);
    format!("select p.id,greatest(coalesce(r.position,0),coalesce(floor.read_floor,0)),coalesce(latest.position,0),
        unread.count,mentioned.count from unnest($1::uuid[]) p(id)
        left join message_reads r on r.place_id=p.id and r.account_id=$2
        left join message_places floor on floor.id=p.id
        left join lateral (select position from messages m where m.place_id=p.id and m.thread_root is null
            and m.readers is null order by position desc limit 1) latest on true
        cross join lateral (select count(*) from ((select 1 from messages m where m.place_id=p.id
            and m.thread_root is null and m.position>greatest(coalesce(r.position,0),coalesce(floor.read_floor,0))
            and m.author_id is not null and m.author_id<>$2 and not m.deleted
            order by m.position limit 100)
            union all (select 1 from messages m where m.place_id=p.id
            and m.thread_root is null and m.position>greatest(coalesce(r.position,0),coalesce(floor.read_floor,0))
            and m.notifies=$2 and not m.deleted
            order by m.position limit 100) limit 100) bounded) unread
        cross join lateral (select count(*) from (select * from ({selected}) candidates limit 100) bounded) mentioned")
}
