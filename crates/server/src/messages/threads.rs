//! Bounded unread followed threads and unread mentions of other threads.
use crate::{
    access::Access,
    error::{ApiError, ApiResult},
    events,
};
use junto_protocol::{
    events::Event,
    ids::{AccountId, ChannelId, MessageId},
    message::{FollowedThreads, ThreadState, UnreadThreads},
};
use sqlx::PgConnection;
use time::OffsetDateTime;
use uuid::Uuid;

pub const STATES_QUERY: &str = "select m.id,coalesce(r.position,0),coalesce(last.position,0),
         case when coalesce(r.following,false) then (select count(*) from (
             select 1 from messages reply where reply.thread_root=m.id and reply.position>r.position
             and not reply.deleted and reply.author_id<>$1 order by reply.position limit 100) unread) else 0 end,
         (select count(*) from (select 1 from message_mention_accounts ma where ma.thread_root=m.id
             and ma.account_id=$1 and ma.unread order by ma.position limit 100) mentions),
         coalesce(r.following,false),r.follow_setting
         from unnest($2::uuid[]) with ordinality requested(id,ordinal) join messages m on m.id=requested.id
         left join thread_reads r on r.root_id=m.id and r.account_id=$1
         left join messages last on last.id=m.last_reply
         where r.account_id is not null or (select true from message_mention_accounts ma
             where ma.account_id=$1 and ma.thread_root=m.id limit 1)
         order by requested.ordinal";

pub async fn state(
    connection: &mut PgConnection,
    account: AccountId,
    root: MessageId,
) -> ApiResult<ThreadState> {
    states(connection, account, &[root])
        .await?
        .pop()
        .ok_or_else(ApiError::not_found)
}

/// A row of [`STATES_QUERY`]: the root, the read position, the newest position, the unread
/// replies, the unread mentions, whether the account follows, and its choice.
type StateRow = (MessageId, i64, i64, i64, i64, bool, Option<bool>);

async fn states(
    connection: &mut PgConnection,
    account: AccountId,
    roots: &[MessageId],
) -> ApiResult<Vec<ThreadState>> {
    let rows: Vec<StateRow> = sqlx::query_as(STATES_QUERY)
        .bind(account)
        .bind(roots)
        .fetch_all(connection)
        .await?;
    Ok(rows
        .into_iter()
        .map(
            |(root_id, last_read, latest, unread, mentions, following, follow_setting)| {
                ThreadState {
                    root_id,
                    last_read: last_read as u64,
                    latest: latest as u64,
                    unread: unread as u64,
                    mentions: mentions as u64,
                    following,
                    follow_setting,
                }
            },
        )
        .collect())
}

/// The threads of the channel `$2` that the account `$1` follows and that have a reply,
/// the one with the newest reply first, after the continuation `($3, $4)`. At most `$5`.
pub const FOLLOWED_QUERY: &str = "select root_id,latest_id from thread_reads
    where account_id=$1 and place_id=$2 and following and latest_id is not null
      and (latest_id,root_id)<($3,$4)
    order by latest_id desc,root_id desc limit $5";

/// One page of the threads of a channel that the account follows, read ones included
/// (ADR 0413). The caller has decided that the account sees the channel.
pub async fn followed(
    connection: &mut PgConnection,
    account: AccountId,
    channel: ChannelId,
    before: Option<(Uuid, Uuid)>,
    limit: u32,
) -> ApiResult<FollowedThreads> {
    let (newest, root) = before.unwrap_or((Uuid::max(), Uuid::max()));
    let mut rows: Vec<(MessageId, Uuid)> = sqlx::query_as(FOLLOWED_QUERY)
        .bind(account)
        .bind(channel)
        .bind(newest)
        .bind(root)
        .bind(i64::from(limit) + 1)
        .fetch_all(&mut *connection)
        .await?;
    let more = rows.len() > limit as usize;
    rows.truncate(limit as usize);
    let next = if more {
        rows.last().map(|(root, newest)| format!("{newest}:{root}"))
    } else {
        None
    };
    let roots: Vec<_> = rows.into_iter().map(|(id, _)| id).collect();
    Ok(FollowedThreads {
        threads: states(connection, account, &roots).await?,
        more,
        next,
    })
}

/// Stores the choice of the account to follow the thread or not, or removes the choice
/// with `None`, and tells the account when something changed (ADR 0412). An account that
/// begins to follow has read the thread up to its newest reply.
pub async fn set_follow(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    access: &Access,
    root: MessageId,
    choice: Option<bool>,
) -> ApiResult<()> {
    let changed: Option<(bool, Option<bool>)> = match choice {
        Some(following) => {
            sqlx::query_as(
                "insert into thread_reads(root_id,account_id,following,follow_setting,position)
                 select m.id,$2,$3,$3,coalesce(reply.position,0)
                 from messages m left join messages reply on reply.id=m.last_reply where m.id=$1
                 on conflict(root_id,account_id) do update set
                     follow_setting=excluded.follow_setting,following=excluded.following,
                     latest=excluded.latest,latest_id=excluded.latest_id,
                     position=case when excluded.following and not thread_reads.following
                         then greatest(thread_reads.position,excluded.position)
                         else thread_reads.position end
                 where thread_reads.follow_setting is distinct from excluded.follow_setting
                 returning following,follow_setting",
            )
            .bind(root)
            .bind(access.account_id)
            .bind(following)
            .fetch_optional(&mut *connection)
            .await?
        }
        None => {
            sqlx::query_as(
                "update thread_reads r set follow_setting=null,following=r.rule_following,
                     latest=coalesce(reply.position,0),latest_id=m.last_reply,
                     position=case when r.rule_following and not r.following
                         then greatest(r.position,coalesce(reply.position,0)) else r.position end
                 from messages m left join messages reply on reply.id=m.last_reply
                 where r.root_id=$1 and r.account_id=$2 and m.id=r.root_id
                   and r.follow_setting is not null
                 returning r.following,r.follow_setting",
            )
            .bind(root)
            .bind(access.account_id)
            .fetch_optional(&mut *connection)
            .await?
        }
    };
    if let Some((following, follow_setting)) = changed {
        events::tell_account(
            connection,
            now,
            access.account_id,
            &Event::ThreadFollowChanged {
                root_id: root,
                following,
                follow_setting,
            },
        )
        .await?;
    }
    Ok(())
}

pub const UNREAD_QUERY: &str = "with followed as (
    select root_id,latest_id as newest from thread_reads
    where account_id=$1 and following and position<latest and place_id=any($2)
      and (latest_id,root_id)<($3,$4) order by latest_id desc,root_id desc limit $5
), mentioned as (
    select ma.thread_root as root_id,ma.message_id as newest from message_mention_accounts ma
    where ma.account_id=$1 and ma.thread_head and ma.place_id=any($2)
      and (ma.message_id,ma.thread_root)<($3,$4)
      and not exists(select 1 from thread_reads r where r.account_id=$1 and r.root_id=ma.thread_root
          and r.following and r.position<r.latest)
    order by ma.message_id desc,ma.thread_root desc limit $5
) select root_id,newest from (select * from followed union all select * from mentioned) candidates
  order by newest desc,root_id desc limit $5";

pub async fn unread(
    connection: &mut PgConnection,
    account: AccountId,
    channels: &[ChannelId],
    before: Option<(Uuid, Uuid)>,
    limit: u32,
) -> ApiResult<UnreadThreads> {
    let (newest, root) = before.unwrap_or((Uuid::max(), Uuid::max()));
    let mut rows: Vec<(MessageId, Uuid)> = sqlx::query_as(UNREAD_QUERY)
        .bind(account)
        .bind(channels)
        .bind(newest)
        .bind(root)
        .bind(i64::from(limit) + 1)
        .fetch_all(&mut *connection)
        .await?;
    let more = rows.len() > limit as usize;
    rows.truncate(limit as usize);
    let next = if more {
        rows.last().map(|(root, newest)| format!("{newest}:{root}"))
    } else {
        None
    };
    let roots: Vec<_> = rows.into_iter().map(|(id, _)| id).collect();
    Ok(UnreadThreads {
        threads: states(connection, account, &roots).await?,
        more,
        next,
    })
}

pub async fn advance(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    access: &Access,
    root: MessageId,
    position: i64,
) -> ApiResult<()> {
    let changed = sqlx::query(
        "insert into thread_reads(root_id,account_id,position) values($1,$2,$3)
        on conflict(root_id,account_id) do update set position=excluded.position
        where thread_reads.position<excluded.position",
    )
    .bind(root)
    .bind(access.account_id)
    .bind(position)
    .execute(&mut *connection)
    .await?
    .rows_affected();
    if changed > 0 {
        sqlx::query(
            "update message_mention_accounts set unread=false where account_id=$1
            and thread_root=$2 and unread and position<=$3",
        )
        .bind(access.account_id)
        .bind(root)
        .bind(position)
        .execute(&mut *connection)
        .await?;
        sqlx::query(
            "update notifications n set read_at=$1 from messages m where n.message_id=m.id
            and n.account_id=$2 and m.thread_root=$3 and m.position<=$4 and n.read_at is null",
        )
        .bind(now)
        .bind(access.account_id)
        .bind(root)
        .bind(position)
        .execute(&mut *connection)
        .await?;
        events::tell_account(
            connection,
            now,
            access.account_id,
            &Event::ThreadRead {
                root_id: root,
                position: position as u64,
            },
        )
        .await?;
    }
    Ok(())
}
