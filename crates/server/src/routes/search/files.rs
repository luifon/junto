use super::{ApiError, ApiResult, Query};
use crate::{
    access::Access,
    messages::{Place, Purpose, uploads},
};
use junto_authz::WorkspacePermission;
use junto_protocol::{
    discovery::{SearchHit, SearchPage},
    message::MessagePlace,
};
use sqlx::{PgConnection, Row};
use time::OffsetDateTime;
use uuid::Uuid;

pub(super) async fn page(
    connection: &mut PgConnection,
    access: &Access,
    query: &Query,
    limit: u32,
) -> ApiResult<SearchPage> {
    let place = match (query.channel_id, query.conversation_id) {
        (Some(id), None) => {
            let place = Place::channel(connection, access, id).await?;
            place.require(access, WorkspacePermission::ReadMessageHistory)?;
            place
        }
        (None, Some(id)) => Place::direct(connection, access, id, Purpose::Read).await?,
        _ => {
            return Err(ApiError::invalid(
                "a file list needs one channel or conversation",
            ));
        }
    };
    let workspace = if let MessagePlace::Channel { workspace_id, .. } = place.value {
        Some(workspace_id)
    } else {
        None
    };
    let invalid = || ApiError::invalid("invalid file cursor");
    let (before, id) = match &query.cursor {
        None => (
            OffsetDateTime::from_unix_timestamp(253_402_300_799)
                .map_err(|error| ApiError::internal(&error))?,
            Uuid::max(),
        ),
        Some(cursor) => {
            let (time, id) = cursor
                .strip_prefix("f:")
                .and_then(|cursor| cursor.split_once(':'))
                .ok_or_else(invalid)?;
            (
                OffsetDateTime::from_unix_timestamp_nanos(time.parse().map_err(|_| invalid())?)
                    .map_err(|_| invalid())?,
                id.parse::<Uuid>().map_err(|_| invalid())?,
            )
        }
    };
    let rows = sqlx::query("with file_page as materialized (
        select distinct a.message_created_at,a.message_id from attachments a
        where a.place_id=$1 and a.attached and a.complete and a.removed_at is null and a.message_id is not null
        and (a.message_created_at,a.message_id)<($3,$4)
        and exists(select 1 from messages m where m.id=a.message_id and not m.deleted
            and m.author_id is not null and (m.readers is null or $2=any(m.readers)))
        order by a.message_created_at desc,a.message_id desc limit $5)
        select m.id,m.place_id,m.author_id,m.created_at,message_search_text(m.document) as text,m.position,m.thread_root
        from file_page p join messages m on m.id=p.message_id order by p.message_created_at desc,p.message_id desc")
        .bind(place.id).bind(access.account_id).bind(before).bind(id).bind(i64::from(limit)+1)
        .fetch_all(&mut *connection).await?;
    let more = rows.len() > limit as usize;
    let mut results = Vec::new();
    let mut next = None;
    for row in rows.into_iter().take(limit as usize) {
        let id = row.try_get("id")?;
        let created_at: OffsetDateTime = row.try_get("created_at")?;
        next = Some(format!("f:{}:{id}", created_at.unix_timestamp_nanos()));
        results.push(SearchHit {
            message_id: Some(id),
            recording_id: None,
            line: None,
            place_id: row.try_get("place_id")?,
            workspace_id: workspace,
            author_id: row.try_get("author_id")?,
            created_at,
            text: row.try_get("text")?,
            rank: 0.0,
            attachments: Vec::new(),
            position: Some(row.try_get::<i64, _>("position")? as u64),
            thread_root: row.try_get("thread_root")?,
            start_ms: None,
        });
    }
    let ids: Vec<_> = results.iter().filter_map(|hit| hit.message_id).collect();
    let mut attachments = uploads::of_messages(connection, &ids).await?;
    for hit in &mut results {
        hit.attachments = hit
            .message_id
            .and_then(|id| attachments.remove(&id))
            .unwrap_or_default();
    }
    Ok(SearchPage {
        results,
        next: more.then_some(next).flatten(),
    })
}
