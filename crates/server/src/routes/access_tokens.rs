//! The routes of access tokens (ADR 0203, ADR 0210). A token is created, listed and revoked
//! with a session; no access token reaches these routes.

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use junto_authz::{Request, ServerPermission};
use junto_protocol::access_token::{
    AccessTokenCreated, AccessTokenInfo, AccessTokenPage, AccessTokenQuery, CreateAccessToken,
};
use junto_protocol::audit::AuditEvent;
use junto_protocol::events::Event;
use junto_protocol::ids::{AccessTokenId, AccountId};
use junto_protocol::secret::AccessToken;
use sqlx::AssertSqlSafe;

use crate::access::Access;
use crate::audit::{self, About};
use crate::error::{ApiError, ApiResult};
use crate::events;
use crate::extract::{ApiJson, ApiPath, ApiQuery, Bearer};
use crate::secrets::{hash_access_token, new_access_token};
use crate::state::AppState;
use crate::tokens::{self, TOKEN_COLUMNS, TokenRow};
use crate::validate;
use crate::{read, transact};

const DEFAULT_PAGE: u32 = 50;
const LARGEST_PAGE: u32 = 200;

/// Creates an access token for the caller's account. The answer carries the token once. A
/// request with the token is decided with the permissions that the account has at the time
/// of that request, on the routes of the stated actions only.
#[utoipa::path(
    post,
    path = "/api/v1/access-tokens",
    tag = "access tokens",
    request_body = CreateAccessToken,
    security(("session" = [])),
    responses((status = 201, body = AccessTokenCreated), ApiError)
)]
pub async fn create_access_token(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiJson(body): ApiJson<CreateAccessToken>,
) -> ApiResult<(StatusCode, Json<AccessTokenCreated>)> {
    let name = validate::name("name", &body.name)?;
    let mut actions = body.actions.clone();
    actions.sort_unstable();
    actions.dedup();
    let bits = tokens::action_bits(&actions)
        .filter(|bits| *bits != 0)
        .ok_or_else(|| ApiError::invalid("actions names at least one action that exists"))?;
    let now = app.now();
    let limits = &app.config.limits;
    if body.expires_at <= now || body.expires_at > now + limits.access_token_longest_lifetime {
        return Err(ApiError::invalid(format!(
            "expires_at is later than now and at most {} days ahead",
            limits.access_token_longest_lifetime.whole_days()
        )));
    }
    let token = new_access_token()?;
    let info = AccessTokenInfo {
        id: AccessTokenId::generate(),
        name,
        created_by: AccountId::from_uuid(uuid::Uuid::nil()),
        actions,
        created_at: now,
        expires_at: body.expires_at,
    };
    let info = transact!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        access.require(Request::Server(ServerPermission::Administrator))?;
        let held: i64 =
            sqlx::query_scalar("select count(*) from access_tokens where created_by = $1")
                .bind(access.account_id)
                .fetch_one(&mut *connection)
                .await?;
        if held >= i64::from(limits.most_access_tokens_per_creator) {
            return Err(ApiError::limit_reached(
                "the account holds the largest number of access tokens",
            ));
        }
        let info = AccessTokenInfo {
            created_by: access.account_id,
            ..info.clone()
        };
        sqlx::query(
            "insert into access_tokens
                 (id, token_hash, created_by, name, actions, created_at, expires_at)
             values ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(info.id)
        .bind(hash_access_token(&token).as_slice())
        .bind(info.created_by)
        .bind(&info.name)
        .bind(bits)
        .bind(info.created_at)
        .bind(info.expires_at)
        .execute(&mut *connection)
        .await?;
        audit::record(
            connection,
            now,
            Some(access.account_id),
            About::Server,
            &AuditEvent::AccessTokenCreated {
                access_token_id: info.id,
                name: info.name.clone(),
                actions: info.actions.clone(),
                expires_at: info.expires_at,
            },
        )
        .await?;
        let created = Event::AccessTokenCreated {
            access_token: info.clone(),
        };
        events::publish(connection, now, &created).await?;
        Ok(info)
    })?;
    Ok((
        StatusCode::CREATED,
        Json(AccessTokenCreated {
            token: AccessToken::new(token),
            access_token: info,
        }),
    ))
}

/// The access tokens that have not expired: every token for a caller with the administrator
/// permission, the caller's own tokens for every other caller. Newest first.
#[utoipa::path(
    get,
    path = "/api/v1/access-tokens",
    tag = "access tokens",
    params(AccessTokenQuery),
    security(("session" = [])),
    responses((status = 200, body = AccessTokenPage), ApiError)
)]
pub async fn list_access_tokens(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiQuery(query): ApiQuery<AccessTokenQuery>,
) -> ApiResult<Json<AccessTokenPage>> {
    let size = match query.limit.unwrap_or(DEFAULT_PAGE) {
        limit @ 1..=LARGEST_PAGE => i64::from(limit),
        _ => {
            return Err(ApiError::invalid(format!(
                "limit must be from 1 to {LARGEST_PAGE}"
            )));
        }
    };
    let now = app.now();
    read!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        access.require(Request::OwnAccount)?;
        let sees_all = access.allows(Request::Server(ServerPermission::Administrator));
        let rows: Vec<TokenRow> = if sees_all {
            sqlx::query_as(LIST_ALL)
                .bind(now)
                .bind(query.before)
                .bind(size + 1)
                .fetch_all(&mut *connection)
                .await?
        } else {
            sqlx::query_as(LIST_OWN)
                .bind(now)
                .bind(query.before)
                .bind(size + 1)
                .bind(access.account_id)
                .fetch_all(&mut *connection)
                .await?
        };
        let size = usize::try_from(size).unwrap_or(usize::MAX);
        let has_more = rows.len() > size;
        let access_tokens = rows
            .into_iter()
            .take(size)
            .map(tokens::info)
            .collect::<ApiResult<Vec<_>>>()?;
        let next_before = if has_more {
            access_tokens.last().map(|token| token.id)
        } else {
            None
        };
        Ok(AccessTokenPage {
            access_tokens,
            next_before,
        })
    })
    .map(Json)
}

/// The statements of the list. They are constants so that a test can read their plans.
pub const LIST_ALL: &str = "select id, name, created_by, actions, created_at, expires_at
     from access_tokens
     where expires_at > $1 and ($2::uuid is null or id < $2)
     order by id desc
     limit $3";
pub const LIST_OWN: &str = "select id, name, created_by, actions, created_at, expires_at
     from access_tokens
     where expires_at > $1 and ($2::uuid is null or id < $2) and created_by = $4
     order by id desc
     limit $3";

/// Revokes an access token: the account that created it, or a caller with the administrator
/// permission. The token stops at once.
#[utoipa::path(
    delete,
    path = "/api/v1/access-tokens/{access_token_id}",
    tag = "access tokens",
    params(("access_token_id" = AccessTokenId, Path)),
    security(("session" = [])),
    responses((status = 204), ApiError)
)]
pub async fn revoke_access_token(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(access_token_id): ApiPath<AccessTokenId>,
) -> ApiResult<StatusCode> {
    let now = app.now();
    transact!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        access.require(Request::OwnAccount)?;
        // A caller who may not see the token does not lock its row and does not wait for
        // it, so that the answer for it is the answer for a token that does not exist.
        let sees_all = access.allows(Request::Server(ServerPermission::Administrator));
        let stored: Option<TokenRow> = sqlx::query_as(AssertSqlSafe(format!(
            "select {TOKEN_COLUMNS} from access_tokens
             where id = $1 and ($2 or created_by = $3)
             for update"
        )))
        .bind(access_token_id)
        .bind(sees_all)
        .bind(access.account_id)
        .fetch_optional(&mut *connection)
        .await?;
        let token = tokens::info(stored.ok_or_else(ApiError::not_found)?)?;
        sqlx::query("delete from access_tokens where id = $1")
            .bind(access_token_id)
            .execute(&mut *connection)
            .await?;
        audit::record(
            connection,
            now,
            Some(access.account_id),
            About::Server,
            &AuditEvent::AccessTokenRevoked {
                access_token_id,
                created_by: token.created_by,
            },
        )
        .await?;
        let revoked = Event::AccessTokenRevoked { access_token_id };
        events::publish(connection, now, &revoked).await
    })?;
    Ok(StatusCode::NO_CONTENT)
}
