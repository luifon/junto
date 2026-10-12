//! Full-text search over currently authorized places, with a stable rank cursor.
use crate::{
    access::Access,
    error::{ApiError, ApiResult},
    extract::{ApiQuery, Bearer},
    messages::Place,
    read,
    state::AppState,
};
use axum::{Json, extract::State};
use junto_protocol::{
    discovery::{SearchHit, SearchPage},
    error::ErrorCode,
    ids::{AccountId, ChannelId, MessageId, RecordingId},
};
use serde::Deserialize;
use sqlx::Row;
use uuid::Uuid;

pub use crate::access::visibility::places;

pub fn read_rate(app: &AppState, access: &Access, counted: &mut bool) -> ApiResult<()> {
    if !*counted {
        app.message_history
            .attempt_of(access.account_id, app.now())
            .map_err(|error| {
                ApiError::new(ErrorCode::RateLimited, "too many history requests")
                    .retry_after(error.retry_after)
            })?;
        *counted = true;
    }
    Ok(())
}

#[derive(Deserialize, utoipa::IntoParams)]
pub struct Query {
    pub q: String,
    pub channel_id: Option<ChannelId>,
    pub author_id: Option<AccountId>,
    pub from: Option<String>,
    pub until: Option<String>,
    pub limit: Option<u32>,
    pub cursor: Option<String>,
}

#[utoipa::path(get,path="/api/v1/search",tag="messages",params(Query),security(("session"=[])),responses((status=200,body=SearchPage),ApiError))]
pub async fn search(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiQuery(query): ApiQuery<Query>,
) -> ApiResult<Json<SearchPage>> {
    let limit = query.limit.unwrap_or(50);
    if query.q.contains('\0')
        || query.q.trim().is_empty()
        || query.q.len() > 1024
        || !(1..=200).contains(&limit)
    {
        return Err(ApiError::invalid(
            "use a query of 1 to 1024 bytes and limit 1 to 200",
        ));
    }
    let date = |value: &Option<String>| {
        value
            .as_ref()
            .map(|text| {
                time::OffsetDateTime::parse(text, &time::format_description::well_known::Rfc3339)
                    .map_err(|_| ApiError::invalid("invalid date"))
            })
            .transpose()
    };
    let from = date(&query.from)?;
    let until = date(&query.until)?;
    if from.zip(until).is_some_and(|(a, b)| a >= b) {
        return Err(ApiError::invalid("from must precede until"));
    }
    let (rank, id, line) = if let Some(cursor) = &query.cursor {
        let mut fields = cursor.split(':');
        let invalid = || ApiError::invalid("invalid search cursor");
        let rank: f32 = fields
            .next()
            .ok_or_else(invalid)?
            .parse()
            .map_err(|_| invalid())?;
        let id: Uuid = fields
            .next()
            .ok_or_else(invalid)?
            .parse()
            .map_err(|_| invalid())?;
        let line: i32 = fields
            .next()
            .ok_or_else(invalid)?
            .parse()
            .map_err(|_| invalid())?;
        if fields.next().is_some() || !rank.is_finite() || rank < 0.0 || line < -1 {
            return Err(invalid());
        }
        (rank, id, line)
    } else {
        (f32::MAX, Uuid::max(), i32::MAX)
    };
    let mut counted = false;
    read!(app, |connection| {
        let access = Access::open(connection, &bearer, app.now()).await?;
        read_rate(&app, &access, &mut counted)?;
        let allowed = places(connection, &access).await?;
        if let Some(channel) = query.channel_id {
            Place::channel(connection, &access, channel).await?;
        }
        let rows = sqlx::query("with query as (select websearch_to_tsquery( 'simple' ,$1) as q), content as ( select m.id,-1 as
             line,m.place_id,m.author_id,m.created_at,message_search_text(m.document) as text,m.search_vector as
             vector,false as transcript,m.position as position,m.thread_root as thread_root,null::bigint as start_ms
             from messages m where m.place_id=any($2) and not m.deleted and m.author_id is
             not null and (m.readers is null or $3=any(m.readers)) union all select
             l.recording_id,l.line,r.channel_id,l.account_id,r.requested_at,l.search_text,l.search_vector,true,
             null::bigint,null::uuid,l.start_ms from
             transcript_lines l join recordings r on r.id=l.recording_id join transcripts t on t.recording_id=r.id join
             channels ch on ch.id=r.channel_id where r.channel_id=any($2) and t.state in ( 'ready' , 'partial' ) and
             reads_call(r.call_id,$3) ), hits as (select
             content.*,ts_rank_cd(vector,q) as rank from content,query where vector @@ q and ($4::uuid is null or
             place_id=$4) and ($5::uuid is null or author_id=$5) and ($6::timestamptz is null or created_at >= $6) and
             ($7::timestamptz is null or created_at < $7)) select * from hits where (rank,id,line)<($8,$9,$10) order by
             rank desc,id desc,line desc limit $11")
            .bind(&query.q)
            .bind(allowed.ids())
            .bind(access.account_id)
            .bind(query.channel_id)
            .bind(query.author_id)
            .bind(from)
            .bind(until)
            .bind(rank)
            .bind(id)
            .bind(line)
            .bind(i64::from(limit) + 1)
            .fetch_all(connection)
            .await?;
        let more = rows.len() > limit as usize;
        let mut results = Vec::new();
        let mut next = None;
        for row in rows.into_iter().take(limit as usize) {
            let id: Uuid = row.try_get("id")?;
            let line: i32 = row.try_get("line")?;
            let rank: f32 = row.try_get("rank")?;
            next = Some(format!("{rank}:{id}:{line}"));
            let transcript: bool = row.try_get("transcript")?;
            results.push(SearchHit {
                message_id: (!transcript).then(|| MessageId::from_uuid(id)),
                recording_id: transcript.then(|| RecordingId::from_uuid(id)),
                line: transcript.then_some(line as u32),
                place_id: row.try_get("place_id")?,
                author_id: row.try_get("author_id")?,
                created_at: row.try_get("created_at")?,
                text: row.try_get("text")?,
                rank,
                position: row
                    .try_get::<Option<i64>, _>("position")?
                    .map(|position| position as u64),
                thread_root: row.try_get("thread_root")?,
                start_ms: row
                    .try_get::<Option<i64>, _>("start_ms")?
                    .map(|start_ms| start_ms as u64),
            });
        }
        Ok(SearchPage {
            results,
            next: if more { next } else { None },
        })
    })
    .map(Json)
}
