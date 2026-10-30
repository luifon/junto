//! Full-text search over currently authorized places, with a stable rank cursor.
use crate::{
    access::Access,
    error::{ApiError, ApiResult},
    extract::{ApiQuery, Bearer},
    messages::{self, Place, Purpose, uploads},
    read,
    state::AppState,
};
use axum::{Json, extract::State};
use junto_protocol::{
    discovery::{SearchHit, SearchPage},
    error::ErrorCode,
    ids::{AccountId, ChannelId, ConversationId, MessageId, RecordingId},
};
use serde::Deserialize;
use sqlx::Row;
use uuid::Uuid;

mod files;

pub use crate::access::visibility::places;

#[utoipa::path(get,path="/api/v1/search/capabilities",tag="messages",security(("session"=[])),responses((status=200,body=junto_protocol::discovery::SearchCapabilities),ApiError))]
pub async fn capabilities(
    State(app): State<AppState>,
    bearer: Bearer,
) -> ApiResult<Json<junto_protocol::discovery::SearchCapabilities>> {
    read!(app, |connection| {
        Access::open(connection, &bearer, app.now()).await?;
        Ok(junto_protocol::discovery::SearchCapabilities::ALL)
    })
    .map(Json)
}

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
    #[serde(default)]
    pub q: String,
    #[serde(default)]
    pub files: bool,
    pub channel_id: Option<ChannelId>,
    pub author_id: Option<AccountId>,
    pub conversation_id: Option<ConversationId>,
    pub from: Option<String>,
    pub until: Option<String>,
    #[serde(default)]
    pub has_file: bool,
    #[serde(default)]
    pub has_link: bool,
    #[serde(default)]
    pub has_picture: bool,
    #[serde(default)]
    pub mentions_me: bool,
    #[serde(default)]
    pub direct: bool,
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
        || (query.q.trim().is_empty()
            && !(query.has_file
                || query.has_link
                || query.has_picture
                || query.mentions_me
                || query.direct
                || query.channel_id.is_some()
                || query.conversation_id.is_some()
                || query.author_id.is_some()
                || query.from.is_some()
                || query.until.is_some()))
        || (query.channel_id.is_some() && query.conversation_id.is_some())
        || query.q.len() > 1024
        || !(1..=200).contains(&limit)
    {
        return Err(ApiError::invalid(
            "use text or a search filter, at most 1024 query bytes and limit 1 to 200",
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
    if query.files
        && (!query.q.trim().is_empty()
            || query.has_link
            || query.has_picture
            || query.mentions_me
            || query.direct
            || query.author_id.is_some()
            || from.is_some()
            || until.is_some()
            || (query.channel_id.is_some() == query.conversation_id.is_some()))
    {
        return Err(ApiError::invalid(
            "a file list needs one scope and no text or other filters",
        ));
    }
    let (rank, id, line) = if let Some(cursor) = query.cursor.as_ref().filter(|_| !query.files) {
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
        if query.files { return files::page(connection, &access, &query, limit).await; }
        let allowed = places(connection, &access).await?;
        if let Some(channel) = query.channel_id {
            Place::channel(connection, &access, channel).await?
                .require(&access, junto_authz::WorkspacePermission::ReadMessageHistory)?;
        }
        if let Some(conversation) = query.conversation_id {
            Place::direct(connection, &access, conversation, Purpose::Read).await?;
        }
        let rows = sqlx::query("with query as (select websearch_to_tsquery( 'simple' ,$1) as q), content as ( select m.id,-1 as
             line,m.place_id,m.author_id,m.created_at,m.search_vector as
             vector,false as transcript,m.position as position,m.thread_root as thread_root,null::bigint as start_ms
             from messages m where m.place_id=any($2) and not m.deleted and m.author_id is
             not null and (m.readers is null or $3=any(m.readers))
             and (not $12 or exists(select 1 from attachments a where a.message_id=m.id and a.complete and a.removed_at is null))
             and (not $13 or jsonb_path_exists(m.document,'lax $.** ? (@.kind == \"link\" || (@.kind == \"text\" && @.text like_regex \"(https?|junto)://[^[:space:]]+\"))'))
             and (not $14 or exists(select 1 from attachments a where a.message_id=m.id and a.complete and a.removed_at is null and a.media_type like 'image/%'))
             and (not $15 or notification_mentioned($3,m.id,true,true))
             and (not $16 or exists(select 1 from conversations c where c.id=m.place_id)) union all select
             l.recording_id,l.line,r.channel_id,l.account_id,r.requested_at,l.search_vector,true,
             null::bigint,null::uuid,l.start_ms from
             transcript_lines l join recordings r on r.id=l.recording_id join transcripts t on t.recording_id=r.id join
             channels ch on ch.id=r.channel_id where r.channel_id=any($2) and t.state in ( 'ready' , 'partial' ) and
             reads_call(r.call_id,$3) and not ($12 or $13 or $14 or $15 or $16) ), hits as (select
             content.*,ts_rank_cd(vector,q) as rank from content,query where ($1 = '' or vector @@ q) and ($17::uuid is null or place_id=$17) and ($4::uuid is null or
             place_id=$4) and ($5::uuid is null or author_id=$5) and ($6::timestamptz is null or created_at >= $6) and
             ($7::timestamptz is null or created_at < $7)), page as materialized (
             select hits.id,hits.line,hits.place_id,hits.author_id,hits.created_at,hits.transcript,
                    hits.position,hits.thread_root,hits.start_ms,hits.rank,ch.workspace_id
             from hits left join channels ch on ch.id=hits.place_id
             where (rank,hits.id,line)<($8,$9,$10)
             order by rank desc,hits.id desc,line desc limit $11)
             select page.*,case when page.transcript then (
                 select l.search_text from transcript_lines l
                 where l.recording_id=page.id and l.line=page.line)
             else null::text end as text,
             case when not page.transcript then (
                 select m.document from messages m where m.id=page.id)
             else null::jsonb end as document
             from page order by page.rank desc,page.id desc,page.line desc")
            .bind(query.q.trim())
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
            .bind(query.has_file)
            .bind(query.has_link)
            .bind(query.has_picture)
            .bind(query.mentions_me)
            .bind(query.direct)
            .bind(query.conversation_id)
            .fetch_all(&mut *connection)
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
            let document = row.try_get::<Option<sqlx::types::Json<junto_protocol::message::Document>>, _>("document")?;
            let text = if let Some(document) = document {
                messages::document::preview_text(&document.0)
            } else { row.try_get::<Option<String>, _>("text")?.unwrap_or_default() };
            let text = text.chars().take(512).collect();
            results.push(SearchHit {
                message_id: (!transcript).then(|| MessageId::from_uuid(id)),
                recording_id: transcript.then(|| RecordingId::from_uuid(id)),
                line: transcript.then_some(line as u32),
                place_id: row.try_get("place_id")?,
                workspace_id: row.try_get("workspace_id")?,
                author_id: row.try_get("author_id")?,
                created_at: row.try_get("created_at")?,
                text,
                rank,
                attachments: Vec::new(),
                position: row
                    .try_get::<Option<i64>, _>("position")?
                    .map(|position| position as u64),
                thread_root: row.try_get("thread_root")?,
                start_ms: row
                    .try_get::<Option<i64>, _>("start_ms")?
                    .map(|start_ms| start_ms as u64),
            });
        }
        if query.has_file || query.has_picture {
            let ids: Vec<_> = results.iter().filter_map(|hit| hit.message_id).collect();
            let mut files = uploads::of_messages(connection, &ids).await?;
            for hit in &mut results {
                hit.attachments = hit.message_id.and_then(|id| files.remove(&id)).unwrap_or_default();
            }
        }
        Ok(SearchPage {
            results,
            next: if more { next } else { None },
        })
    })
    .map(Json)
}
