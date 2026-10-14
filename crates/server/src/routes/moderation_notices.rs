use axum::Json;
use axum::extract::State;
use junto_authz::Request;
use junto_protocol::account::{
    ModerationNotice, ModerationNoticeKind, ModerationNoticePage, ModerationNoticeQuery,
    ReadModerationNotices,
};
use junto_protocol::auth::ChallengeOperation;
use junto_protocol::ids::{AccountId, WorkspaceId};
use junto_protocol::keys::PublicKey;
use sqlx::PgConnection;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::access::Access;
use crate::error::{ApiError, ApiResult};
use crate::extract::{ApiJson, ApiQuery, Bearer, ClientAddress};
use crate::state::AppState;
use crate::{proof, read, transact};

pub(crate) const RETENTION: time::Duration = time::Duration::days(30);

pub(crate) async fn record(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    account_id: AccountId,
    workspace_id: Option<WorkspaceId>,
    kind: ModerationNoticeKind,
    message: Option<&str>,
) -> ApiResult<()> {
    let name: String = if let Some(workspace_id) = workspace_id {
        sqlx::query_scalar("select name from workspaces where id = $1")
            .bind(workspace_id)
            .fetch_one(&mut *connection)
            .await?
    } else {
        sqlx::query_scalar("select name from server_state")
            .fetch_one(&mut *connection)
            .await?
    };
    let kind = match kind {
        ModerationNoticeKind::WorkspaceRemoved => "workspace_removed",
        ModerationNoticeKind::ServerRemoved => "server_removed",
        ModerationNoticeKind::Banned => "banned",
        ModerationNoticeKind::Unknown => return Err(ApiError::internal(&"unknown notice kind")),
    };
    let id = Uuid::now_v7();
    sqlx::query("insert into moderation_notices(id,account_id,kind,workspace_id,name,message,occurred_at) values($1,$2,$3,$4,$5,$6,$7)")
        .bind(id).bind(account_id).bind(kind).bind(workspace_id).bind(name).bind(message).bind(now)
        .execute(&mut *connection).await?;
    sqlx::query("insert into moderation_notice_devices(public_key,notice_id,account_id) select public_key,$2,account_id from devices where account_id=$1")
        .bind(account_id).bind(id).execute(connection).await?;
    Ok(())
}

#[derive(sqlx::FromRow)]
struct NoticeRow {
    id: Uuid,
    kind: String,
    workspace_id: Option<WorkspaceId>,
    name: String,
    message: Option<String>,
    occurred_at: OffsetDateTime,
}

pub(crate) async fn page(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    account: Option<AccountId>,
    key: Option<&PublicKey>,
    before: Option<Uuid>,
) -> ApiResult<ModerationNoticePage> {
    let rows: Vec<NoticeRow> = if let Some(account) = account {
        sqlx::query_as("select id,kind,workspace_id,name,message,occurred_at from moderation_notices where account_id=$1 and occurred_at>$3 and ($2::uuid is null or id<$2) order by id desc limit 101")
            .bind(account).bind(before).bind(now - RETENTION).fetch_all(connection).await?
    } else if let Some(key) = key {
        sqlx::query_as("select n.id,n.kind,n.workspace_id,n.name,n.message,n.occurred_at from moderation_notice_devices d join moderation_notices n on n.id=d.notice_id where d.public_key=$1 and d.account_id=n.account_id and not exists (select 1 from devices current where current.public_key=d.public_key and current.account_id<>d.account_id) and n.occurred_at>$3 and ($2::uuid is null or d.notice_id<$2) order by d.notice_id desc limit 101")
            .bind(key.as_bytes().as_slice()).bind(before).bind(now - RETENTION).fetch_all(connection).await?
    } else {
        return Err(ApiError::unauthenticated());
    };
    let more = rows.len() > 100;
    let notices = rows
        .into_iter()
        .take(100)
        .map(|row| {
            let kind = match row.kind.as_str() {
                "workspace_removed" => ModerationNoticeKind::WorkspaceRemoved,
                "server_removed" => ModerationNoticeKind::ServerRemoved,
                "banned" => ModerationNoticeKind::Banned,
                _ => return Err(ApiError::internal(&"unknown stored notice kind")),
            };
            Ok(ModerationNotice {
                id: row.id,
                kind,
                workspace_id: row.workspace_id,
                name: row.name,
                message: row.message,
                at: row.occurred_at,
            })
        })
        .collect::<ApiResult<Vec<_>>>()?;
    let next_before = more
        .then(|| notices.last().map(|notice| notice.id))
        .flatten();
    Ok(ModerationNoticePage {
        notices,
        next_before,
    })
}

/// The caller's historical moderation notices, newest first, in pages of 100.
#[utoipa::path(get, path="/api/v1/account/moderation-notices", tag="moderation",
    params(ModerationNoticeQuery), security(("session"=[])),
    responses((status=200,body=ModerationNoticePage),ApiError))]
pub async fn list(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiQuery(query): ApiQuery<ModerationNoticeQuery>,
) -> ApiResult<Json<ModerationNoticePage>> {
    read!(app, |connection| {
        let access = Access::open(connection, &bearer, app.now()).await?;
        access.require(Request::OwnAccount)?;
        page(
            connection,
            app.now(),
            Some(access.account_id),
            None,
            query.before,
        )
        .await
    })
    .map(Json)
}

/// Reads notices addressed to the proven device key, without giving it a session.
/// The proof uses the dedicated moderation_notices operation; login and join ignore
/// historical notice readers. A key without notices receives an empty page.
#[utoipa::path(post, path="/api/v1/auth/moderation-notices", tag="moderation",
    request_body=ReadModerationNotices, responses((status=200,body=ModerationNoticePage),ApiError))]
pub async fn prove(
    State(app): State<AppState>,
    ClientAddress(address): ClientAddress,
    ApiJson(body): ApiJson<ReadModerationNotices>,
) -> ApiResult<Json<ModerationNoticePage>> {
    proof::limit_address(&app.challenge_requests, address, app.now())?;
    transact!(app, |connection| {
        let proven = proof::consume(
            &app,
            connection,
            address,
            &body.proof,
            ChallengeOperation::ModerationNotices,
            app.now(),
        )
        .await?;
        page(
            connection,
            app.now(),
            None,
            Some(&proven.public_key),
            body.before,
        )
        .await
    })
    .map(Json)
}
