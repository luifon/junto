//! Current mention nodes and one notification for each newly mentioned account.
use super::{Place, document};
use crate::{
    access::Access,
    error::{ApiError, ApiResult},
    events,
};
use junto_authz::{Request, WorkspacePermission};
use junto_protocol::{
    events::Event,
    ids::{AccountId, MessageId, WorkspaceRoleId},
    message::{Document, Inline, MessagePlace},
};
use sqlx::{PgConnection, types::Json};
use time::OffsetDateTime;

#[derive(sqlx::FromRow)]
struct MentionRecipients {
    account_id: Option<AccountId>,
    role_id: Option<WorkspaceRoleId>,
    everyone: bool,
    recipients: Vec<AccountId>,
}

pub async fn mentions(
    connection: &mut PgConnection,
    access: &Access,
    place: &Place,
    id: MessageId,
    doc: &Document,
    now: OffsetDateTime,
) -> ApiResult<Vec<AccountId>> {
    let mut added_recipients = Vec::new();
    let previous: Option<Json<Document>> =
        sqlx::query_scalar("select document from messages where id=$1")
            .bind(id)
            .fetch_one(&mut *connection)
            .await?;
    let was_mentioned: bool =
        sqlx::query_scalar("select exists(select 1 from message_mentions where message_id=$1)")
            .bind(id)
            .fetch_one(&mut *connection)
            .await?;
    let old_nodes = previous
        .as_ref()
        .filter(|_| was_mentioned)
        .map(|document| document::validate(&document.0))
        .transpose()?
        .unwrap_or_default();
    let nodes = document::validate(doc)?;
    let prior_recipients: Vec<AccountId> = sqlx::query_scalar(
        "select distinct a.id
             from accounts a
             join message_mentions mm on mm.message_id=$1
             where mm.account_id=a.id or mm.everyone
           or exists(select 1
             from member_roles mr
             where mr.account_id=a.id
             and mr.role_id=mm.role_id)
           or exists(select 1
             from workspace_roles r
             join workspace_members wm on wm.workspace_id=r.workspace_id
             where r.id=mm.role_id
             and r.is_everyone
             and wm.account_id=a.id)",
    )
    .bind(id)
    .fetch_all(&mut *connection)
    .await?;

    let old_recipients: Vec<MentionRecipients> = sqlx::query_as(
        "select account_id,role_id,everyone,recipients from message_mentions where message_id=$1",
    )
    .bind(id)
    .fetch_all(&mut *connection)
    .await?;
    sqlx::query("delete from message_mentions where message_id = $1")
        .bind(id)
        .execute(&mut *connection)
        .await?;
    let mut distinct = std::collections::HashSet::new();
    for node in nodes {
        let (account, role, everyone) = match node {
            Inline::Account { account_id } => (Some(*account_id), None, false),
            Inline::Role { role_id } => (None, Some(*role_id), false),
            Inline::Everyone => (None, None, true),
            _ => continue,
        };
        if !distinct.insert((account, role, everyone)) {
            continue;
        }
        if distinct.len() > 100 {
            return Err(ApiError::invalid("a message has at most 100 mentions"));
        }
        if !old_nodes.contains(&node) {
            match place.value {
                MessagePlace::Channel { workspace_id, .. } => {
                    if everyone {
                        access.require(Request::Channel {
                            scope: place.scope.as_ref().ok_or_else(ApiError::not_found)?,
                            permission: WorkspacePermission::MentionEveryone,
                        })?;
                    }
                    if let Some(account) = account {
                        let exists: bool = sqlx::query_scalar(
                            "select exists(select 1
             from workspace_members m
             join accounts a on a.id=m.account_id
             where m.workspace_id=$1
             and m.account_id=$2
             and a.status='active')",
                        )
                        .bind(workspace_id)
                        .bind(account)
                        .fetch_one(&mut *connection)
                        .await?;
                        if !exists {
                            return Err(ApiError::invalid("mentioned account is not a member"));
                        }
                    }
                    if let Some(role) = role {
                        let is_everyone: bool = sqlx::query_scalar(
                            "select is_everyone from workspace_roles
                             where workspace_id=$1 and id=$2",
                        )
                        .bind(workspace_id)
                        .bind(role)
                        .fetch_optional(&mut *connection)
                        .await?
                        .ok_or_else(|| {
                            ApiError::invalid("mentioned role is not in this workspace")
                        })?;
                        place.require(
                            access,
                            if is_everyone {
                                WorkspacePermission::MentionEveryone
                            } else {
                                WorkspacePermission::MentionRoles
                            },
                        )?;
                    }
                }
                MessagePlace::Direct {
                    low_account_id,
                    high_account_id,
                    ..
                } => {
                    if role.is_some()
                        || everyone
                        || account.is_some_and(|a| a != low_account_id && a != high_account_id)
                    {
                        return Err(ApiError::invalid(
                            "a direct message mentions only its participants",
                        ));
                    }
                }
            }
        }
        sqlx::query("insert into message_mentions(message_id,account_id,role_id,everyone,place_id,position,recipients)
             select $1,$2,$3,$4,m.place_id,m.position,coalesce($5::uuid[],array(
                 select a.id from accounts a where a.status='active' and (
                     a.id=$2 or exists(
                         select 1 from workspace_members wm
                         join channels ch on ch.workspace_id=wm.workspace_id
                         where ch.id=m.place_id and wm.account_id=a.id
                           and ($4 or exists(select 1 from member_roles mr
                               where mr.account_id=a.id and mr.role_id=$3)
                               or exists(select 1 from workspace_roles wr where wr.id=$3 and wr.is_everyone))
                     )
                 )
             )) from messages m where m.id=$1")
            .bind(id)
            .bind(account)
            .bind(role)
            .bind(everyone)
            .bind(
                old_recipients
                    .iter()
                    .find(|old| {
                        (old.account_id, old.role_id, old.everyone) == (account, role, everyone)
                    })
                    .map(|old| &old.recipients),
            )
            .execute(&mut *connection)
            .await?;
    }
    sqlx::query("delete from message_mention_accounts where message_id=$1")
        .bind(id)
        .execute(&mut *connection)
        .await?;
    sqlx::query("insert into message_mention_accounts(place_id,account_id,position,message_id,thread_root,unread)
         select distinct m.place_id,recipient,m.position,m.id,m.thread_root,
             m.position>coalesce((select position from thread_reads where root_id=m.thread_root and account_id=recipient),0) from messages m
         join message_mentions mm on mm.message_id=m.id
         cross join lateral unnest(mm.recipients) recipient
         where m.id=$1 and m.author_id<>recipient and not m.deleted")
        .bind(id)
        .execute(&mut *connection)
        .await?;
    sqlx::query(
        "insert into thread_reads(root_id,account_id,following,rule_following)
         select distinct m.thread_root,mm.account_id,true,true from messages m
         join message_mentions mm on mm.message_id=m.id
         where m.id=$1 and m.thread_root is not null and mm.account_id is not null
         on conflict(root_id,account_id) do update set rule_following=true,
             following=coalesce(thread_reads.follow_setting,true),
             latest=excluded.latest,latest_id=excluded.latest_id",
    )
    .bind(id)
    .execute(&mut *connection)
    .await?;

    if let MessagePlace::Channel {
        workspace_id,
        channel_id,
    } = place.value
    {
        let recipients: Vec<AccountId> = sqlx::query_scalar(
            "insert into message_mentions_notified(message_id,account_id)
             select distinct $1, wm.account_id
             from workspace_members wm
             join accounts a on a.id=wm.account_id
             join message_mentions mm on mm.message_id=$1
             where wm.workspace_id=$2
             and a.status='active'
             and wm.account_id<>$3
             and not(wm.account_id=any($4))
             and (mm.account_id=wm.account_id or mm.everyone
                    or exists(select 1
             from member_roles mr
             where mr.account_id=wm.account_id
             and mr.role_id=mm.role_id)
                    or exists(select 1
             from workspace_roles r
             where r.id=mm.role_id
             and r.is_everyone))
             on conflict do nothing
             returning account_id",
        )
        .bind(id)
        .bind(workspace_id)
        .bind(access.account_id)
        .bind(&prior_recipients)
        .fetch_all(&mut *connection)
        .await?;
        let position: i64 = sqlx::query_scalar("select position from messages where id=$1")
            .bind(id)
            .fetch_one(&mut *connection)
            .await?;
        added_recipients = recipients.clone();
        if !recipients.is_empty() {
            let notice_id = uuid::Uuid::now_v7();
            events::log::record_at_id(
                connection,
                now,
                notice_id,
                &events::audience::Audience::Mention {
                    workspace_id,
                    channel_id,
                    message_id: id,
                    account_id: None,
                    notice_id: Some(notice_id),
                },
                &Event::MessageMentioned {
                    workspace_id,
                    channel_id,
                    message_id: id,
                    position: position as u64,
                },
            )
            .await?;
            sqlx::query("insert into message_mention_receivers(event_id,account_id) select $1,account_id from unnest($2::uuid[]) account_id where not notification_quiet(account_id,$3)")
                .bind(notice_id)
                .bind(&recipients)
                .bind(now)
                .execute(connection)
                .await?;
        }
    }
    Ok(added_recipients)
}

/// The messages of `ids` that mention the account now: the account is not the author, the
/// message is not deleted, and the notification settings of the account for the place do
/// not mute it. One statement for all of them.
pub async fn mentioned_among(
    connection: &mut PgConnection,
    account: AccountId,
    ids: &[MessageId],
    now: OffsetDateTime,
) -> ApiResult<Vec<MessageId>> {
    Ok(sqlx::query_scalar("select m.id from messages m cross join lateral notification_policy($2,m.place_id,$3) p
         where m.id=any($1) and m.author_id<>$2 and not m.deleted and not p.muted and p.level<>'nothing'
         and notification_mentioned($2,m.id,p.roles,p.everyone)")
        .bind(ids)
        .bind(account)
        .bind(now)
        .fetch_all(connection)
        .await?)
}
