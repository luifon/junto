use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use junto_authz::{Admission, Request, ServerPermission};
use junto_protocol::account::DeviceOrigin;
use junto_protocol::audit::AuditEvent;
use junto_protocol::auth::{ChallengeOperation, SessionCreated};
use junto_protocol::error::ErrorCode;
use junto_protocol::events::Event;
use junto_protocol::ids::{AccountId, InviteId, JoinRequestId, WorkspaceId};
use junto_protocol::invite::{
    ApproveJoinRequest, CreateInvite, Invite, InviteCreated, InviteRevocation, Join, JoinRequest,
    JoinRequestState, JoinRequestStatus, JoinRequestStatusQuery, RedeemInvite, Redemption,
    RequestToJoin,
};
use junto_protocol::secret::InviteCode;
use sqlx::PgConnection;
use time::OffsetDateTime;

use crate::access::{self, Access};
use crate::audit::{self, About};
use crate::db::{self, StoredText};
use crate::error::{ApiError, ApiResult, violates};
use crate::events;
use crate::events::audience::Audience;
use crate::extract::{ApiJson, ApiPath, Bearer, ClientAddress};
use crate::proof;
use crate::secrets::{CodePurpose, INVITE_CODE_SYMBOLS, hash_code, random_code};
use crate::state::AppState;
use crate::{read, transact};
use crate::{store, validate};

type InviteRow = (
    InviteId,
    AccountId,
    OffsetDateTime,
    OffsetDateTime,
    i32,
    i32,
    Option<String>,
    Vec<WorkspaceId>,
    String,
);

const INVITE_SELECT: &str = "select i.id, i.created_by, i.created_at, i.expires_at, i.max_uses, \
     i.uses, i.revocation, \
     array(select w.workspace_id from invite_workspaces w \
           join workspaces live on live.id = w.workspace_id and live.deleted_at is null \
           where w.invite_id = i.id order by w.workspace_id) \
     , creator.name from invites i join accounts creator on creator.id = i.created_by";

/// An invite and an approval name a bounded number of workspaces (ADR 0306).
fn require_few_workspaces(app: &AppState, workspace_ids: &[WorkspaceId]) -> ApiResult<()> {
    let most = app.config.limits.invite_most_workspaces;
    if workspace_ids.len() > most {
        return Err(ApiError::invalid(format!(
            "workspace_ids names at most {most} workspaces"
        )));
    }
    Ok(())
}

fn invite(row: InviteRow) -> ApiResult<Invite> {
    let (
        id,
        created_by,
        created_at,
        expires_at,
        max_uses,
        uses,
        revocation,
        workspace_ids,
        creator_name,
    ) = row;
    Ok(Invite {
        id,
        created_by,
        created_by_name: Some(creator_name),
        created_at,
        expires_at,
        max_uses: db::count(max_uses)?,
        uses: db::count(uses)?,
        workspace_ids,
        revocation: revocation
            .as_deref()
            .map(InviteRevocation::read)
            .transpose()?,
    })
}

/// Records an event about an invite for the accounts that read the invite: the holders of
/// `create_invites` who may see every workspace in `workspace_ids`, and its creator
/// (ADR 0111).
async fn announce(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    created_by: AccountId,
    workspace_ids: &[WorkspaceId],
    event: &Event,
) -> ApiResult<()> {
    let audience = Audience::Invite {
        created_by,
        workspace_ids: workspace_ids.to_vec(),
    };
    events::log::record(connection, now, &audience, event).await
}

/// Whether the caller reads each of `invites`, in their order. One statement reads the
/// memberships of the caller in the workspaces that the invites name.
async fn readable(
    connection: &mut PgConnection,
    access: &Access,
    invites: &[(AccountId, &[WorkspaceId])],
) -> ApiResult<Vec<bool>> {
    let mut named: Vec<WorkspaceId> = invites
        .iter()
        .flat_map(|(_, workspace_ids)| workspace_ids.iter().copied())
        .collect();
    named.sort_unstable();
    named.dedup();
    let member_of = access::memberships_among(connection, access.account_id, &named).await?;
    Ok(invites
        .iter()
        .map(|(created_by, workspace_ids)| {
            let is_creator = *created_by == access.account_id;
            access::reads_invite(access.actor(), is_creator, workspace_ids, &member_of)
        })
        .collect())
}

/// Creates an invite code. The response is the only one that carries the code.
#[utoipa::path(
    post,
    path = "/api/v1/invites",
    tag = "joining",
    request_body = CreateInvite,
    security(("session" = [])),
    responses((status = 201, body = InviteCreated), ApiError)
)]
pub async fn create_invite(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiJson(body): ApiJson<CreateInvite>,
) -> ApiResult<(StatusCode, Json<InviteCreated>)> {
    let now = app.now();
    let limits = &app.config.limits;
    let expires_at = body
        .expires_at
        .unwrap_or(now + limits.invite_default_lifetime);
    if expires_at <= now || expires_at > now + limits.invite_longest_lifetime {
        return Err(ApiError::invalid(format!(
            "expires_at must be in the future and at most {} days ahead",
            limits.invite_longest_lifetime.whole_days()
        )));
    }
    let max_uses = body.max_uses.unwrap_or(1);
    if max_uses == 0 || max_uses > limits.invite_most_uses {
        return Err(ApiError::invalid(format!(
            "max_uses must be from 1 to {}",
            limits.invite_most_uses
        )));
    }
    require_few_workspaces(&app, &body.workspace_ids)?;
    let stored_max_uses = i32::try_from(max_uses).map_err(|error| ApiError::internal(&error))?;
    let code = random_code(INVITE_CODE_SYMBOLS)?;
    let id = InviteId::generate();

    let invite = transact!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        let workspace_ids = store::granted_workspaces(connection, &body.workspace_ids).await?;
        let reach = access.reach(connection, &workspace_ids).await?;
        access.require(Request::Admit {
            admission: Admission::CreateInvite,
            reach,
        })?;
        store::require_workspaces_exist(connection, &workspace_ids).await?;

        sqlx::query(
            "insert into invites (id, code_hash, created_by, created_at, expires_at, max_uses)
                 values ($1, $2, $3, $4, $5, $6)",
        )
        .bind(id)
        .bind(hash_code(CodePurpose::Invite, &code).as_slice())
        .bind(access.account_id)
        .bind(now)
        .bind(expires_at)
        .bind(stored_max_uses)
        .execute(&mut *connection)
        .await?;
        sqlx::query(
            "insert into invite_workspaces (invite_id, workspace_id)
                 select $1, workspace_id from unnest($2::uuid[]) as granted (workspace_id)",
        )
        .bind(id)
        .bind(&workspace_ids)
        .execute(&mut *connection)
        .await?;
        audit::record(
            connection,
            now,
            Some(access.account_id),
            About::Server,
            &AuditEvent::InviteCreated {
                invite_id: id,
                expires_at,
                max_uses,
                workspace_ids: workspace_ids.clone(),
            },
        )
        .await?;
        let invite = Invite {
            id,
            created_by: access.account_id,
            created_by_name: Some(
                sqlx::query_scalar("select name from accounts where id = $1")
                    .bind(access.account_id)
                    .fetch_one(&mut *connection)
                    .await?,
            ),
            created_at: now,
            expires_at,
            max_uses,
            uses: 0,
            workspace_ids,
            revocation: None,
        };
        let created = Event::InviteCreated {
            invite: invite.clone(),
        };
        let (creator, named) = (invite.created_by, &invite.workspace_ids);
        announce(connection, now, creator, named, &created).await?;
        Ok(invite)
    })?;

    Ok((
        StatusCode::CREATED,
        Json(InviteCreated {
            invite,
            code: InviteCode::new(code),
        }),
    ))
}

/// The invites that the caller reads, without their codes: the invites that the caller
/// created, and the invites whose workspaces the caller may all see.
#[utoipa::path(
    get,
    path = "/api/v1/invites",
    tag = "joining",
    security(("session" = [])),
    responses((status = 200, body = [Invite]), ApiError)
)]
pub async fn list_invites(
    State(app): State<AppState>,
    bearer: Bearer,
) -> ApiResult<Json<Vec<Invite>>> {
    let now = app.now();
    read!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        access.require(Request::Server(ServerPermission::CreateInvites))?;
        let rows: Vec<InviteRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "{INVITE_SELECT} order by i.id desc"
        )))
        .fetch_all(&mut *connection)
        .await?;
        let invites = rows
            .into_iter()
            .map(invite)
            .collect::<ApiResult<Vec<_>>>()?;
        let named: Vec<(AccountId, &[WorkspaceId])> = invites
            .iter()
            .map(|invite| (invite.created_by, invite.workspace_ids.as_slice()))
            .collect();
        let read = readable(connection, &access, &named).await?;
        Ok(invites
            .into_iter()
            .zip(read)
            .filter_map(|(invite, read)| read.then_some(invite))
            .collect())
    })
    .map(Json)
}

/// Cancels an invite that the caller reads. Its code stops working. An invite that the
/// caller does not read is answered as an invite that does not exist.
#[utoipa::path(
    delete,
    path = "/api/v1/invites/{invite_id}",
    tag = "joining",
    params(("invite_id" = InviteId, Path)),
    security(("session" = [])),
    responses((status = 204), ApiError)
)]
pub async fn revoke_invite(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(invite_id): ApiPath<InviteId>,
) -> ApiResult<StatusCode> {
    let now = app.now();
    transact!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        access.require(Request::Server(ServerPermission::CreateInvites))?;
        let stored: Option<(AccountId, bool)> = sqlx::query_as(
            "select created_by, revoked_at is not null from invites where id = $1 for update",
        )
        .bind(invite_id)
        .fetch_optional(&mut *connection)
        .await?;
        let (created_by, revoked) = stored.ok_or_else(ApiError::not_found)?;
        let workspace_ids = named_by(connection, invite_id).await?;
        if readable(connection, &access, &[(created_by, &workspace_ids)]).await? != [true] {
            return Err(ApiError::not_found());
        }
        if revoked {
            return Ok(());
        }
        sqlx::query("update invites set revoked_at = $2, revocation = 'revoked' where id = $1")
            .bind(invite_id)
            .bind(now)
            .execute(&mut *connection)
            .await?;
        audit::record(
            connection,
            now,
            Some(access.account_id),
            About::Server,
            &AuditEvent::InviteRevoked {
                invite_id,
                invite_created_by: Some(created_by),
            },
        )
        .await?;
        let revoked = Event::InviteRevoked {
            invite_id,
            revocation: InviteRevocation::Revoked,
        };
        announce(connection, now, created_by, &workspace_ids, &revoked).await
    })?;
    Ok(StatusCode::NO_CONTENT)
}

/// Counts one use of the invite behind `code_hash`, or states why it cannot be used.
/// Returns the invite, its creator and the workspaces that it grants now.
async fn use_invite(
    connection: &mut PgConnection,
    code_hash: &[u8; 32],
    now: OffsetDateTime,
) -> ApiResult<(InviteId, AccountId, Vec<WorkspaceId>)> {
    let used: Option<(InviteId, AccountId, i32)> = sqlx::query_as(
        "update invites set uses = uses + 1
         where code_hash = $1 and revoked_at is null and expires_at > $2 and uses < max_uses
         returning id, created_by, uses",
    )
    .bind(code_hash.as_slice())
    .bind(now)
    .fetch_optional(&mut *connection)
    .await?;
    if let Some((invite_id, created_by, uses)) = used {
        let granted = granted_by(connection, invite_id).await?;
        let used = Event::InviteUsed {
            invite_id,
            uses: db::count(uses)?,
        };
        announce(connection, now, created_by, &granted, &used).await?;
        return Ok((invite_id, created_by, granted));
    }
    let row: Option<(bool, OffsetDateTime)> = sqlx::query_as(
        "select revoked_at is not null, expires_at from invites where code_hash = $1",
    )
    .bind(code_hash.as_slice())
    .fetch_optional(connection)
    .await?;
    Err(match row {
        None => ApiError::new(ErrorCode::InviteNotFound, "the invite code is unknown"),
        Some((true, _)) => ApiError::new(ErrorCode::InviteRevoked, "the invite was cancelled"),
        Some((false, expires_at)) if expires_at <= now => {
            ApiError::new(ErrorCode::InviteExpired, "the invite has expired")
        }
        Some(_) => ApiError::new(ErrorCode::InviteExhausted, "the invite has no uses left"),
    })
}

/// The workspaces that an invite names and that exist, at most 100 (ADR 0306).
async fn named_by(
    connection: &mut PgConnection,
    invite_id: InviteId,
) -> ApiResult<Vec<WorkspaceId>> {
    Ok(sqlx::query_scalar(
        "select w.workspace_id from invite_workspaces w
         join workspaces live on live.id = w.workspace_id and live.deleted_at is null
         where w.invite_id = $1
         order by w.workspace_id",
    )
    .bind(invite_id)
    .fetch_all(connection)
    .await?)
}

/// The workspaces that an invite grants now: the ones that it names and that exist. An
/// invite whose workspaces were all deleted grants nothing and counts as cancelled
/// (ADR 0308).
async fn granted_by(
    connection: &mut PgConnection,
    invite_id: InviteId,
) -> ApiResult<Vec<WorkspaceId>> {
    let workspace_ids = named_by(connection, invite_id).await?;
    if workspace_ids.is_empty() {
        return Err(ApiError::new(
            ErrorCode::InviteRevoked,
            "every workspace of the invite was deleted",
        ));
    }
    Ok(workspace_ids)
}

/// Adds the caller's account to the workspaces of an invite that it is not yet a member of.
/// The invite counts one use, under the same rules as a join. Attempts are limited per
/// account and per client address.
#[utoipa::path(
    post,
    path = "/api/v1/invites/redemption",
    tag = "joining",
    request_body = RedeemInvite,
    security(("session" = [])),
    responses((status = 200, body = Redemption), ApiError)
)]
pub async fn redeem_invite(
    State(app): State<AppState>,
    bearer: Bearer,
    ClientAddress(address): ClientAddress,
    ApiJson(body): ApiJson<RedeemInvite>,
) -> ApiResult<Json<Redemption>> {
    let now = app.now();
    proof::limit_address(&app.redemptions_by_address, address, now)?;
    let account_id = read!(app, |connection| {
        Ok(Access::open(connection, &bearer, now).await?.account_id)
    })?;
    proof::limit(&app.redemptions_by_account, account_id, now)?;
    let code_hash = hash_code(CodePurpose::Invite, body.invite_code.expose());
    transact!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        access.require(Request::OwnAccount)?;
        let (invite_id, invite_created_by, granted) =
            use_invite(connection, &code_hash, now).await?;

        let workspace_ids: Vec<WorkspaceId> = sqlx::query_scalar(
            "insert into workspace_members (workspace_id, account_id, joined_at)
             select workspace_id, $2, $3 from unnest($1::uuid[]) as granted (workspace_id)
             on conflict do nothing
             returning workspace_id",
        )
        .bind(&granted)
        .bind(access.account_id)
        .bind(now)
        .fetch_all(&mut *connection)
        .await?;
        if workspace_ids.is_empty() {
            return Err(ApiError::conflict(
                "the account is already a member of every workspace of the invite",
            ));
        }
        let mut workspace_ids = workspace_ids;
        workspace_ids.sort();
        store::announce_members(connection, now, access.account_id, &workspace_ids).await?;
        audit::record(
            connection,
            now,
            Some(access.account_id),
            About::Server,
            &AuditEvent::InviteRedeemed {
                account_id: access.account_id,
                invite_id,
                invite_created_by,
                workspace_ids: workspace_ids.clone(),
            },
        )
        .await?;
        Ok(Redemption { workspace_ids })
    })
    .map(Json)
}

/// Creates an account with an invite code and starts its first session.
#[utoipa::path(
    post,
    path = "/api/v1/join",
    tag = "joining",
    request_body = Join,
    responses((status = 201, body = SessionCreated), ApiError)
)]
pub async fn join(
    State(app): State<AppState>,
    ClientAddress(address): ClientAddress,
    ApiJson(body): ApiJson<Join>,
) -> ApiResult<(StatusCode, Json<SessionCreated>)> {
    proof::limit_address(&app.join_attempts, address, app.now())?;
    let name = validate::name("name", &body.name)?;
    let device_name = validate::optional_name("device_name", &body.device_name)?;
    let now = app.now();
    let code_hash = hash_code(CodePurpose::Invite, body.invite_code.expose());

    let session = transact!(app, |connection| {
        let proven = proof::consume(
            &app,
            connection,
            address,
            &body.proof,
            ChallengeOperation::Join,
            now,
        )
        .await?;
        let (invite_id, invite_created_by, workspace_ids) =
            use_invite(connection, &code_hash, now).await?;
        let account_id = store::create_account(connection, now, &name).await?;
        let device = store::create_device(
            connection,
            now,
            account_id,
            &proven.public_key,
            &device_name,
            DeviceOrigin::Invite,
        )
        .await?;
        store::add_members(connection, now, account_id, &workspace_ids).await?;
        audit::record(
            connection,
            now,
            Some(account_id),
            About::Server,
            &AuditEvent::AccountJoined {
                account_id,
                invite_id,
                invite_created_by,
            },
        )
        .await?;
        store::start_session(&app, connection, now, account_id, device.id).await
    })?;
    Ok((StatusCode::CREATED, Json(session)))
}

/// Asks to join without a code. The request carries a name and waits for an approval.
#[utoipa::path(
    post,
    path = "/api/v1/join-requests",
    tag = "joining",
    request_body = RequestToJoin,
    responses((status = 201, body = JoinRequest), ApiError)
)]
pub async fn request_to_join(
    State(app): State<AppState>,
    ClientAddress(address): ClientAddress,
    ApiJson(body): ApiJson<RequestToJoin>,
) -> ApiResult<(StatusCode, Json<JoinRequest>)> {
    proof::limit_address(&app.join_attempts, address, app.now())?;
    let name = validate::name("name", &body.name)?;
    let now = app.now();
    let limits = &app.config.limits;
    let request = JoinRequest {
        id: JoinRequestId::generate(),
        name,
        created_at: now,
        expires_at: now + limits.join_request_lifetime,
    };

    transact!(app, |connection| {
        let proven = proof::consume(
            &app,
            connection,
            address,
            &body.proof,
            ChallengeOperation::RequestToJoin,
            now,
        )
        .await?;
        store::require_key_unregistered(connection, &proven.public_key).await?;
        sqlx::query(
            "update join_requests set state = 'expired'
             where public_key = $1 and state = 'pending' and expires_at <= $2",
        )
        .bind(proven.public_key.as_bytes().as_slice())
        .bind(now)
        .execute(&mut *connection)
        .await?;
        let pending: i64 = sqlx::query_scalar(
            "select count(*) from join_requests where state = 'pending' and expires_at > $1",
        )
        .bind(now)
        .fetch_one(&mut *connection)
        .await?;
        if pending >= i64::from(limits.most_pending_join_requests) {
            return Err(ApiError::new(
                ErrorCode::JoinRequestsFull,
                "too many join requests wait for an answer",
            ));
        }
        let inserted = sqlx::query(
            "insert into join_requests (id, public_key, name, state, created_at, expires_at)
             values ($1, $2, $3, 'pending', $4, $5)",
        )
        .bind(request.id)
        .bind(proven.public_key.as_bytes().as_slice())
        .bind(&request.name)
        .bind(request.created_at)
        .bind(request.expires_at)
        .execute(&mut *connection)
        .await;
        match inserted {
            Ok(_) => {
                let created = Event::JoinRequestCreated {
                    join_request: request.clone(),
                };
                events::publish(connection, now, &created).await
            }
            Err(error) if violates(&error, "join_requests_one_pending_per_key") => {
                Err(ApiError::new(
                    ErrorCode::JoinRequestPending,
                    "a join request of this device key already waits for an answer",
                ))
            }
            Err(error) => Err(error.into()),
        }
    })?;
    Ok((StatusCode::CREATED, Json(request)))
}

/// The result of the latest join request of a device key, given to the holder of that key.
#[utoipa::path(
    post,
    path = "/api/v1/join-requests/status",
    tag = "joining",
    request_body = JoinRequestStatusQuery,
    responses((status = 200, body = JoinRequestStatus), ApiError)
)]
pub async fn join_request_status(
    State(app): State<AppState>,
    ClientAddress(address): ClientAddress,
    ApiJson(body): ApiJson<JoinRequestStatusQuery>,
) -> ApiResult<Json<JoinRequestStatus>> {
    let now = app.now();
    transact!(app, |connection| {
        let proven = proof::consume(
            &app,
            connection,
            address,
            &body.proof,
            ChallengeOperation::JoinRequestStatus,
            now,
        )
        .await?;
        let row: Option<(JoinRequestId, String, OffsetDateTime, Option<AccountId>)> =
            sqlx::query_as(
                "select id, state, expires_at, account_id from join_requests
                 where public_key = $1
                 order by created_at desc, id desc
                 limit 1",
            )
            .bind(proven.public_key.as_bytes().as_slice())
            .fetch_optional(&mut *connection)
            .await?;
        let (id, state, expires_at, account_id) = row.ok_or_else(ApiError::not_found)?;
        let state = match state.as_str() {
            "pending" if expires_at <= now => JoinRequestState::Expired,
            "pending" => JoinRequestState::Pending,
            "expired" => JoinRequestState::Expired,
            "approved" => JoinRequestState::Approved,
            "denied" => JoinRequestState::Denied,
            other => return Err(ApiError::internal(&format_args!("unknown state {other:?}"))),
        };
        Ok(JoinRequestStatus {
            id,
            state,
            account_id,
        })
    })
    .map(Json)
}

/// The join requests that wait for an answer.
#[utoipa::path(
    get,
    path = "/api/v1/join-requests",
    tag = "joining",
    security(("session" = [])),
    responses((status = 200, body = [JoinRequest]), ApiError)
)]
pub async fn list_join_requests(
    State(app): State<AppState>,
    bearer: Bearer,
) -> ApiResult<Json<Vec<JoinRequest>>> {
    let now = app.now();
    read!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        access.require(Request::Server(ServerPermission::ApproveJoinRequests))?;
        let rows: Vec<(JoinRequestId, String, OffsetDateTime, OffsetDateTime)> = sqlx::query_as(
            "select id, name, created_at, expires_at from join_requests
             where state = 'pending' and expires_at > $1
             order by created_at, id",
        )
        .bind(now)
        .fetch_all(&mut *connection)
        .await?;
        Ok(rows
            .into_iter()
            .map(|(id, name, created_at, expires_at)| JoinRequest {
                id,
                name,
                created_at,
                expires_at,
            })
            .collect())
    })
    .map(Json)
}

async fn pending_request(
    connection: &mut PgConnection,
    join_request_id: JoinRequestId,
    now: OffsetDateTime,
) -> ApiResult<(Vec<u8>, String)> {
    let row: Option<(Vec<u8>, String, String, OffsetDateTime)> = sqlx::query_as(
        "select public_key, name, state, expires_at from join_requests where id = $1 for update",
    )
    .bind(join_request_id)
    .fetch_optional(connection)
    .await?;
    let (public_key, name, state, expires_at) = row.ok_or_else(ApiError::not_found)?;
    if state != "pending" || expires_at <= now {
        return Err(ApiError::conflict(
            "the join request no longer waits for an answer",
        ));
    }
    Ok((public_key, name))
}

/// Approves a join request and creates the account in the named workspaces.
#[utoipa::path(
    post,
    path = "/api/v1/join-requests/{join_request_id}/approval",
    tag = "joining",
    params(("join_request_id" = JoinRequestId, Path)),
    request_body = ApproveJoinRequest,
    security(("session" = [])),
    responses((status = 200, body = JoinRequestStatus), ApiError)
)]
pub async fn approve_join_request(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(join_request_id): ApiPath<JoinRequestId>,
    ApiJson(body): ApiJson<ApproveJoinRequest>,
) -> ApiResult<Json<JoinRequestStatus>> {
    require_few_workspaces(&app, &body.workspace_ids)?;
    let now = app.now();
    transact!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        let workspace_ids = store::granted_workspaces(connection, &body.workspace_ids).await?;
        let reach = access.reach(connection, &workspace_ids).await?;
        access.require(Request::Admit {
            admission: Admission::ApproveJoinRequest,
            reach,
        })?;
        store::require_workspaces_exist(connection, &workspace_ids).await?;
        let (public_key, name) = pending_request(connection, join_request_id, now).await?;
        let public_key = db::public_key(&public_key)?;

        let account_id = store::create_account(connection, now, &name).await?;
        store::create_device(
            connection,
            now,
            account_id,
            &public_key,
            "",
            DeviceOrigin::JoinRequest,
        )
        .await?;
        store::add_members(connection, now, account_id, &workspace_ids).await?;
        sqlx::query(
            "update join_requests
             set state = 'approved', decided_at = $2, decided_by = $3, account_id = $4
             where id = $1",
        )
        .bind(join_request_id)
        .bind(now)
        .bind(access.account_id)
        .bind(account_id)
        .execute(&mut *connection)
        .await?;
        audit::record(
            connection,
            now,
            Some(access.account_id),
            About::Server,
            &AuditEvent::JoinRequestApproved {
                join_request_id,
                account_id,
                workspace_ids,
            },
        )
        .await?;
        let decided = Event::JoinRequestDecided {
            join_request_id,
            state: JoinRequestState::Approved,
            account_id: Some(account_id),
        };
        events::publish(connection, now, &decided).await?;
        Ok(JoinRequestStatus {
            id: join_request_id,
            state: JoinRequestState::Approved,
            account_id: Some(account_id),
        })
    })
    .map(Json)
}

/// Denies a join request.
#[utoipa::path(
    post,
    path = "/api/v1/join-requests/{join_request_id}/denial",
    tag = "joining",
    params(("join_request_id" = JoinRequestId, Path)),
    security(("session" = [])),
    responses((status = 200, body = JoinRequestStatus), ApiError)
)]
pub async fn deny_join_request(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(join_request_id): ApiPath<JoinRequestId>,
) -> ApiResult<Json<JoinRequestStatus>> {
    let now = app.now();
    transact!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        access.require(Request::Server(ServerPermission::ApproveJoinRequests))?;
        pending_request(connection, join_request_id, now).await?;
        sqlx::query(
            "update join_requests set state = 'denied', decided_at = $2, decided_by = $3
             where id = $1",
        )
        .bind(join_request_id)
        .bind(now)
        .bind(access.account_id)
        .execute(&mut *connection)
        .await?;
        audit::record(
            connection,
            now,
            Some(access.account_id),
            About::Server,
            &AuditEvent::JoinRequestDenied { join_request_id },
        )
        .await?;
        let decided = Event::JoinRequestDecided {
            join_request_id,
            state: JoinRequestState::Denied,
            account_id: None,
        };
        events::publish(connection, now, &decided).await?;
        Ok(JoinRequestStatus {
            id: join_request_id,
            state: JoinRequestState::Denied,
            account_id: None,
        })
    })
    .map(Json)
}

/// Ends grants made by a removed workspace member, in the removal transaction.
pub(crate) async fn withdraw_workspace(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    actor: AccountId,
    creator: AccountId,
    workspace: WorkspaceId,
) -> ApiResult<()> {
    let mut after = uuid::Uuid::nil();
    loop {
        let rows: Vec<InviteRow> = sqlx::query_as(sqlx::AssertSqlSafe(withdrawal_statement()))
            .bind(creator)
            .bind(after)
            .fetch_all(&mut *connection)
            .await?;
        if rows.is_empty() {
            return Ok(());
        }
        for row in rows {
            let mut changed = invite(row)?;
            after = changed.id.as_uuid();
            if !changed.workspace_ids.contains(&workspace) {
                continue;
            }
            if changed.workspace_ids.len() <= 1 {
                sqlx::query("update invites set revoked_at=$2, revocation='revoked' where id=$1")
                    .bind(changed.id)
                    .bind(now)
                    .execute(&mut *connection)
                    .await?;
                audit::record(
                    connection,
                    now,
                    Some(actor),
                    About::Server,
                    &AuditEvent::InviteRevoked {
                        invite_id: changed.id,
                        invite_created_by: Some(creator),
                    },
                )
                .await?;
                announce(
                    connection,
                    now,
                    creator,
                    &changed.workspace_ids,
                    &Event::InviteRevoked {
                        invite_id: changed.id,
                        revocation: InviteRevocation::Revoked,
                    },
                )
                .await?;
            } else {
                sqlx::query("delete from invite_workspaces where invite_id=$1 and workspace_id=$2")
                    .bind(changed.id)
                    .bind(workspace)
                    .execute(&mut *connection)
                    .await?;
                changed.workspace_ids.retain(|id| *id != workspace);
                audit::record(
                    connection,
                    now,
                    Some(actor),
                    About::Server,
                    &AuditEvent::InviteWorkspacesChanged {
                        invite_id: changed.id,
                        invite_created_by: creator,
                        removed_workspace_id: workspace,
                    },
                )
                .await?;
                announce(
                    connection,
                    now,
                    creator,
                    &changed.workspace_ids,
                    &Event::InviteChanged {
                        invite: changed.clone(),
                    },
                )
                .await?;
            }
        }
    }
}

/// The fixed-size creator page used while withdrawing a workspace's grants.
pub fn withdrawal_statement() -> String {
    format!(
        "{INVITE_SELECT} where i.created_by=$1 and i.revoked_at is null and i.id>$2 order by i.id limit 100"
    )
}
