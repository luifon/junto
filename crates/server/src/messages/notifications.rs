//! Durable notices without message text, authorized again for every read and delivery.
use crate::{
    access::Access,
    error::{ApiError, ApiResult},
    events,
};
use junto_protocol::{
    discovery::{
        Notification, NotificationKind, NotificationLevel, NotificationSettings,
        ScopedNotificationSettings,
    },
    events::Event,
    ids::{AccountId, ChannelId, MessageId},
};
use sqlx::{PgConnection, Row};
use time::OffsetDateTime;
use uuid::Uuid;

pub const fn level_name(level: NotificationLevel) -> &'static str {
    match level {
        NotificationLevel::All => "all",
        NotificationLevel::Mentions => "mentions",
        NotificationLevel::Nothing => "nothing",
    }
}

pub fn stored_level(level: &str) -> ApiResult<NotificationLevel> {
    match level {
        "all" => Ok(NotificationLevel::All),
        "mentions" => Ok(NotificationLevel::Mentions),
        "nothing" => Ok(NotificationLevel::Nothing),
        _ => Err(ApiError::internal(
            &"a stored notification level is unknown",
        )),
    }
}

/// The notice of a direct-message call that `account_id` did not answer, for the line
/// `id` in the conversation `place` (ADR 0518). An account that muted the conversation
/// gets none. An account in its quiet time is no receiver of the event of the notice, as
/// for every notice.
pub async fn missed_call(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    id: MessageId,
    place: Uuid,
    account_id: AccountId,
) -> ApiResult<()> {
    let noticed = sqlx::query(
        "insert into notifications (id, account_id, message_id, kind, created_at)
         select $1, a.id, $1, 'missed_call', $3
         from accounts a cross join lateral notification_policy(a.id, $4, $3) p
         where a.id = $2 and a.status = 'active' and not p.muted
         on conflict (account_id, message_id) do nothing",
    )
    .bind(id)
    .bind(account_id)
    .bind(now)
    .bind(place)
    .execute(&mut *connection)
    .await?
    .rows_affected();
    if noticed > 0 {
        publish(connection, now, id, &[(account_id, true)], true).await?;
    }
    Ok(())
}

pub async fn create(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    id: MessageId,
    added_mentions: Option<&[AccountId]>,
) -> ApiResult<()> {
    let (place_id, channel_id): (Uuid, Option<ChannelId>) = sqlx::query_as("select m.place_id,ch.id from messages m left join channels ch on ch.id=m.place_id where m.id=$1")
        .bind(id)
        .fetch_one(&mut *connection)
        .await?;
    let candidates: Vec<AccountId> = sqlx::query_scalar("with message as materialized (select * from messages where id=$1), candidates as ( select
             unnest(mm.recipients) as id from message_mentions mm where mm.message_id=$1 union select tr.account_id
             from thread_reads tr join message m on tr.root_id=m.thread_root where not $2 and coalesce(tr.follow_setting,tr.writer) union select
             cv.low_account_id from conversations cv join message m on m.place_id=cv.id union select cv.high_account_id
             from conversations cv join message m on m.place_id=cv.id union select ns.account_id from
             notification_settings ns cross join message m join channels ch on ch.id=m.place_id where not $2 and
             m.thread_root is null and ns.scope_id in(ch.id,ch.workspace_id, '00000000-0000-0000-0000-000000000000' )
             and ns.settings->> 'level' = 'all'
             and (ns.scope_id<> '00000000-0000-0000-0000-000000000000' or exists(
                 select 1 from workspace_members wm where wm.workspace_id=ch.workspace_id and wm.account_id=ns.account_id)) union select wm.account_id from workspace_members wm join channels ch
             on ch.workspace_id=wm.workspace_id join message m on m.place_id=ch.id join workspace_notification_defaults
             d on d.workspace_id=ch.workspace_id where not $2 and m.thread_root is null and d.level= 'all' ) select
             a.id from candidates c join accounts a on a.id=c.id cross join message m where a.status= 'active' and
             a.id<>m.author_id and (not $2 or a.id=any($3))")
        .bind(id)
        .bind(added_mentions.is_some())
        .bind(added_mentions)
        .fetch_all(&mut *connection)
        .await?;
    let allowed: Vec<_> = crate::access::visibility::load(
        connection,
        &candidates,
        Some(&[place_id]),
        channel_id.is_none(),
    )
    .await?
    .into_iter()
    .filter(|(_, places)| places.ids().contains(&place_id))
    .map(|(account, _)| account)
    .collect();
    let inserted: Vec<(AccountId, String)> = sqlx::query_as("insert into notifications(id,account_id,message_id,kind,created_at) select m.id,a.id,m.id,case when cv.id
             is not null then 'direct' when notification_mentioned(a.id,m.id,p.roles,p.everyone) then 'mention' when
             m.thread_root is not null then 'thread' else 'message' end,$2 from messages m left join conversations cv
             on cv.id=m.place_id cross join unnest($3::uuid[]) a(id) cross join lateral
             notification_policy(a.id,m.place_id,$2) p where m.id=$1 and m.author_id is not null and not m.deleted and
             (m.readers is null or a.id=any(m.readers)) and not p.muted and (cv.id is not null or (p.level<> 'nothing'
             and ( notification_mentioned(a.id,m.id,p.roles,p.everyone) or (not $4 and m.thread_root is not null and
             exists(select 1 from thread_reads tr where tr.root_id=m.thread_root and tr.account_id=a.id and coalesce(tr.follow_setting,tr.writer)))
             or (not $4 and m.thread_root is null and p.level= 'all' )))) on conflict(account_id,message_id) do update
             set kind= 'mention' ,read_at=null,created_at=excluded.created_at where excluded.kind= 'mention' and
             notifications.kind in ('message','thread') returning account_id,kind")
        .bind(id)
        .bind(now)
        .bind(allowed)
        .bind(added_mentions.is_some())
        .fetch_all(&mut *connection)
        .await?;
    if !inserted.is_empty() {
        let recipients: Vec<_> = inserted
            .into_iter()
            .map(|(id, kind)| (id, kind == "mention" || kind == "direct"))
            .collect();
        publish(connection, now, id, &recipients, true).await?;
    }
    Ok(())
}

pub async fn visible(
    connection: &mut PgConnection,
    access: &Access,
    now: OffsetDateTime,
    ids: Option<&[MessageId]>,
    before: Option<Uuid>,
    limit: i64,
) -> ApiResult<Vec<Notification>> {
    let named: Option<Vec<Uuid>> = if let Some(ids) = ids {
        Some(
            sqlx::query_scalar("select distinct place_id from messages where id=any($1)")
                .bind(ids)
                .fetch_all(&mut *connection)
                .await?,
        )
    } else {
        None
    };
    let allowed =
        crate::access::visibility::load(connection, &[access.account_id], named.as_deref(), true)
            .await?
            .pop()
            .map(|(_, places)| places)
            .unwrap_or_default();
    visible_in(connection, access, now, ids, before, limit, &allowed).await
}

/// The notices that the account `$1` reads at the time `$2` in the places `$3`: the source
/// and the conditions of a statement, which adds its own conditions with `and`. `source`
/// names the notices as `n` and the message of each as `m`.
fn readable_notices(source: &str) -> String {
    format!("{source} {READABLE_CONDITIONS}")
}

/// The notices of an account with their messages, for [`readable_notices`].
const STORED_NOTICES: &str = "notifications n join messages m on m.id=n.message_id";

const READABLE_CONDITIONS: &str = "left join
             conversations cv on cv.id=m.place_id cross join lateral notification_policy($1,m.place_id,$2) p where
             n.account_id=$1 and m.place_id=any($3) and not m.deleted and (m.readers is null or $1=any(m.readers))
             and not p.muted and (cv.id is not null or p.level<> 'nothing' ) and (n.kind<> 'mention' or
             notification_mentioned($1,m.id,p.roles,p.everyone)) and (n.kind<> 'message' or p.level= 'all' ) and
             (cv.id is null or not exists(select 1 from direct_mutes dm where dm.account_id=$1 and
             dm.other_account_id=case when cv.low_account_id=$1 then cv.high_account_id else cv.low_account_id end
             and (dm.muted_until is null or dm.muted_until>$2)))";

/// Marks the unread notices that the account reads now in `places` and that were created
/// at or before `until` as read at `now`, in one statement. Returns their number. A notice
/// that no list returns to the account stays as it is, so the number tells nothing of a
/// place that the account cannot read (ADR 0412).
pub async fn mark_all_read(
    connection: &mut PgConnection,
    access: &Access,
    now: OffsetDateTime,
    until: OffsetDateTime,
    places: &crate::access::visibility::Places,
) -> ApiResult<u64> {
    Ok(sqlx::query(sqlx::AssertSqlSafe(mark_all_read_statement()))
        .bind(access.account_id)
        .bind(now)
        .bind(places.ids())
        .bind(until)
        .execute(connection)
        .await?
        .rows_affected())
}

/// The statement of [`mark_all_read`]: the account `$1`, the time `$2`, the places `$3`
/// and the bound `$4`. It reads the unread notices of the account first, through their
/// index, and decides for each of them whether the account reads it now; an account has
/// at most 500 notices after each maintenance run. The message of each one is read by its
/// identifier: without the fixed order, the statement can read every message of the
/// places first.
pub fn mark_all_read_statement() -> String {
    let readable = readable_notices(
        "unread n join lateral (select * from messages where id=n.message_id offset 0) m on true",
    );
    format!(
        "with unread as materialized (
             select * from notifications
             where account_id=$1 and read_at is null and created_at<=$4)
         update notifications set read_at=$2 where account_id=$1 and id = any(array(
             select n.id from {readable}))"
    )
}

pub async fn visible_in(
    connection: &mut PgConnection,
    access: &Access,
    now: OffsetDateTime,
    ids: Option<&[MessageId]>,
    before: Option<Uuid>,
    limit: i64,
    places: &crate::access::visibility::Places,
) -> ApiResult<Vec<Notification>> {
    // The cursor is a notice that this caller reads now. A notice that is hidden from the
    // caller is not a cursor, as a notice that does not exist is not (ADR 0205).
    let after: Option<(OffsetDateTime, Uuid)> = match before {
        None => None,
        Some(before) => {
            let cursor: Option<(OffsetDateTime, Uuid)> =
                sqlx::query_as(sqlx::AssertSqlSafe(format!(
                    "select n.created_at,n.id from {} and n.id=$4",
                    readable_notices(STORED_NOTICES)
                )))
                .bind(access.account_id)
                .bind(now)
                .bind(places.ids())
                .bind(before)
                .fetch_optional(&mut *connection)
                .await?;
            Some(cursor.ok_or_else(|| ApiError::invalid("invalid notification cursor"))?)
        }
    };
    let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
        "select n.* from {} and ($4::uuid[] is null or n.message_id=any($4)) and
             ($5::timestamptz is null or (n.created_at,n.id)<($5,$6)) order by n.created_at desc,n.id
             desc limit $7",
        readable_notices(STORED_NOTICES)
    )))
    .bind(access.account_id)
    .bind(now)
    .bind(places.ids())
    .bind(ids)
    .bind(after.map(|(created_at, _)| created_at))
    .bind(after.map(|(_, id)| id))
    .bind(limit)
    .fetch_all(connection)
    .await?;
    rows.into_iter()
        .map(|row| {
            Ok(Notification {
                id: row.try_get("id")?,
                message_id: row.try_get("message_id")?,
                kind: match row.try_get::<&str, _>("kind")? {
                    "message" => NotificationKind::Message,
                    "mention" => NotificationKind::Mention,
                    "direct" => NotificationKind::Direct,
                    "thread" => NotificationKind::Thread,
                    "missed_call" => NotificationKind::MissedCall,
                    _ => return Err(ApiError::invalid("invalid notice kind")),
                },
                created_at: row.try_get("created_at")?,
                read_at: row.try_get("read_at")?,
            })
        })
        .collect()
}

pub async fn settings(
    connection: &mut PgConnection,
    access: &Access,
    allowed: &crate::access::visibility::Places,
) -> ApiResult<Vec<ScopedNotificationSettings>> {
    let rows: Vec<(Uuid, sqlx::types::Json<NotificationSettings>)> = sqlx::query_as(
        "select scope_id,settings from notification_settings where account_id=$1 order by scope_id",
    )
    .bind(access.account_id)
    .fetch_all(connection)
    .await?;
    let places = allowed.ids();
    Ok(rows
        .into_iter()
        .filter(|(id, _)| {
            id.is_nil()
                || places.contains(id)
                || allowed
                    .workspaces
                    .iter()
                    .any(|workspace| workspace.as_uuid() == *id)
        })
        .map(|(scope_id, value)| ScopedNotificationSettings {
            scope_id,
            settings: value.0,
        })
        .collect())
}

pub async fn quiet(
    connection: &mut PgConnection,
    account: AccountId,
    now: OffsetDateTime,
) -> ApiResult<bool> {
    Ok(sqlx::query_scalar("select notification_quiet($1,$2)")
        .bind(account)
        .bind(now)
        .fetch_one(connection)
        .await?)
}

pub async fn publish(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    id: MessageId,
    recipients: &[(AccountId, bool)],
    alert: bool,
) -> ApiResult<()> {
    let notice_id = Uuid::now_v7();
    events::log::record_at_id(
        connection,
        now,
        notice_id,
        &events::audience::Audience::Notification {
            message_id: id,
            notice_id,
        },
        &Event::NotificationChanged { message_id: id },
    )
    .await?;
    let accounts: Vec<_> = recipients.iter().map(|(id, _)| *id).collect();
    let background: Vec<_> = recipients
        .iter()
        .map(|(_, background)| *background)
        .collect();
    sqlx::query("insert into notification_receivers(event_id,account_id,background,alert) select
             $1,account_id,background,$4 from unnest($2::uuid[],$3::boolean[]) receivers(account_id,background) where
             not $4 or not notification_quiet(account_id,$5)")
        .bind(notice_id)
        .bind(accounts)
        .bind(background)
        .bind(alert)
        .bind(now)
        .execute(connection)
        .await?;
    Ok(())
}
