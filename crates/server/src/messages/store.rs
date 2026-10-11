//! Message content, page loading, unread summaries and system lines.
use super::{Place, uploads};
use crate::{
    access::Access,
    error::{ApiError, ApiResult},
    events,
};
use junto_protocol::{
    events::Event,
    ids::{ChannelId, MessageId},
    message::*,
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
    )> = sqlx::query_as("select call_id,readers from messages where id=$1")
        .bind(message_id)
        .fetch_optional(&mut *connection)
        .await?;
    let (call_id, readers) = audience.unwrap_or_default();
    if let (
        Some(call_id),
        MessagePlace::Channel {
            workspace_id,
            channel_id,
        },
    ) = (call_id, place)
    {
        events::log::record(
            connection,
            now,
            &events::audience::Audience::SystemMessage {
                workspace_id,
                channel_id,
                call_id,
                readers,
            },
            &event,
        )
        .await?;
    } else {
        events::publish(connection, now, &event).await?;
    }
    Ok(())
}

pub const VISIBLE: &str = "(m.readers is null or $2=any(m.readers))";

pub async fn get(
    connection: &mut PgConnection,
    access: &Access,
    id: MessageId,
) -> ApiResult<(Place, Message)> {
    let row = sqlx::query(sqlx::AssertSqlSafe(format!(
        "select m.*, a.name as display_name
             from messages m
             left join accounts a on a.id=m.author_id
             where m.id=$1
             and {VISIBLE}"
    )))
    .bind(id)
    .bind(access.account_id)
    .fetch_optional(&mut *connection)
    .await?
    .ok_or_else(ApiError::not_found)?;
    let place = Place::load(connection, access, row.try_get("place_id")?).await?;
    let mut messages = load_messages(connection, access, &place, &[id]).await?;
    Ok((place, messages.pop().ok_or_else(ApiError::not_found)?))
}

/// Loads a history page in three statements, independent of its number of messages.
pub async fn load_messages(
    connection: &mut PgConnection,
    access: &Access,
    place: &Place,
    ids: &[MessageId],
) -> ApiResult<Vec<Message>> {
    let rows = sqlx::query(
        "select m.*,a.name as display_name,
             case when exists(select 1 from messages target where target.id=m.reply_to
                  and (target.readers is null or $2=any(target.readers)))
                  then m.reply_to end as visible_reply_to
             from messages m
             left join accounts a on a.id=m.author_id
             where m.id=any($1)
             order by m.position,m.id",
    )
    .bind(ids)
    .bind(access.account_id)
    .fetch_all(&mut *connection)
    .await?;
    let reactions: Vec<(MessageId, String, i64, bool)> = sqlx::query_as(
        "select message_id,emoji,count(*),bool_or(account_id=$2)
             from message_reactions
             where message_id=any($1)
             group by message_id,emoji
             order by emoji",
    )
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
) -> ApiResult<MessageState> {
    let states = states(connection, access, &[place.value]).await?;
    states.into_iter().next().ok_or_else(ApiError::not_found)
}

/// Aggregates every visible place together, reusing snapshot authorization.
async fn states(
    connection: &mut PgConnection,
    access: &Access,
    places: &[MessagePlace],
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
                })
        })
        .collect())
}

pub async fn snapshot(
    connection: &mut PgConnection,
    access: &Access,
    workspaces: &[junto_protocol::snapshot::WorkspaceState],
) -> ApiResult<Vec<MessageState>> {
    let mut places: Vec<MessagePlace> = workspaces
        .iter()
        .flat_map(|workspace| {
            workspace
                .channels
                .iter()
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
    states(connection, access, &places).await
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
    };
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
        sqlx::query_scalar(
            "select case when hidden then
                case when cardinality(readers)>0 then readers else
                    array(select account_id from call_message_members where call_id=$1 order by account_id)
                end end from call_message_privacy where call_id=$1",
        )
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
    erase(connection, now, message.id).await?;
    if message.permanent {
        sqlx::query(
            "update channels set permanent_message_count=permanent_message_count-1
             where id=$1",
        )
        .bind(place.id)
        .execute(&mut *connection)
        .await?;
    }
    changed(
        connection,
        now,
        place.value,
        message.id,
        message.position,
        true,
    )
    .await
}

/// Erases content while retaining the marker required by replies and future threads.
pub(crate) async fn erase(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    id: MessageId,
) -> ApiResult<()> {
    uploads::remove_message(connection, now, id).await?;
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
        sqlx::query("update messages set document=null,system=null,author_id=null,edited_at=null,permanent=false,deleted=true,recording_id=null
             where id=$1")
            .bind(id).execute(&mut *connection).await?;
    } else {
        sqlx::query("delete from messages where id=$1")
            .bind(id)
            .execute(&mut *connection)
            .await?;
    }
    Ok(())
}

pub fn state_query() -> String {
    "select p.id,coalesce(r.position,0),coalesce(latest.position,0),
            unread.count,mentioned.count
     from unnest($1::uuid[]) p(id)
     left join message_reads r on r.place_id=p.id and r.account_id=$2
     left join lateral (
         select position from messages m
         where m.place_id=p.id and m.thread_root is null and m.readers is null
         order by position desc limit 1
     ) latest on true
     cross join lateral (
         select count(*) from (
             select 1 from messages m
             where m.place_id=p.id and m.thread_root is null
               and m.position>coalesce(r.position,0)
               and m.author_id is not null and m.author_id<>$2 and not m.deleted
             order by m.position limit 100
         ) bounded
     ) unread
     cross join lateral (
         select count(*) from (
             select 1 from message_mention_accounts mm
             where mm.place_id=p.id and mm.account_id=$2
               and mm.position>coalesce(r.position,0)
             order by mm.position
             limit 100
         ) bounded
     ) mentioned"
        .to_owned()
}
