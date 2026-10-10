use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use junto_authz::Request;
use junto_protocol::account::{AccountStatus, DeviceOrigin};
use junto_protocol::audit::AuditEvent;
use junto_protocol::auth::{
    Challenge, ChallengeOperation, ClaimOwner, CreateChallenge, Login, SessionCreated,
};
use junto_protocol::error::ErrorCode;
use junto_protocol::events::Event;
use junto_protocol::ids::{AccountId, DeviceId};

use crate::access::Access;
use crate::audit::{self, About};
use crate::db::StoredText;
use crate::error::{ApiError, ApiResult};
use crate::events;
use crate::extract::{ApiJson, Bearer, ClientAddress};
use crate::proof;
use crate::secrets::{CodePurpose, constant_time_eq, hash_code};
use crate::state::AppState;
use crate::store::{SessionEnd, Sessions};
use crate::transact;
use crate::{owner, store, validate};

/// Asks for a single-use challenge for one device key and one operation.
#[utoipa::path(
    post,
    path = "/api/v1/auth/challenges",
    tag = "auth",
    request_body = CreateChallenge,
    responses((status = 201, body = Challenge), ApiError)
)]
pub async fn create_challenge(
    State(app): State<AppState>,
    ClientAddress(address): ClientAddress,
    ApiJson(body): ApiJson<CreateChallenge>,
) -> ApiResult<(StatusCode, Json<Challenge>)> {
    proof::limit_address(&app.challenge_requests, address, app.now())?;
    let challenge = proof::issue(&app, &body).await?;
    Ok((StatusCode::CREATED, Json(challenge)))
}

/// Starts a session with a signed `login` challenge.
#[utoipa::path(
    post,
    path = "/api/v1/auth/sessions",
    tag = "auth",
    request_body = Login,
    responses((status = 201, body = SessionCreated), ApiError)
)]
pub async fn log_in(
    State(app): State<AppState>,
    ClientAddress(address): ClientAddress,
    ApiJson(body): ApiJson<Login>,
) -> ApiResult<(StatusCode, Json<SessionCreated>)> {
    let now = app.now();
    let session = transact!(app, |connection| {
        let proven = proof::consume(
            &app,
            connection,
            address,
            &body.proof,
            ChallengeOperation::Login,
            now,
        )
        .await?;
        let device: Option<(DeviceId, AccountId, String)> = sqlx::query_as(
            "select d.id, d.account_id, a.status
                 from devices d
                 join accounts a on a.id = d.account_id
                 where d.public_key = $1
                 for update of d",
        )
        .bind(proven.public_key.as_bytes().as_slice())
        .fetch_optional(&mut *connection)
        .await?;
        let (device_id, account_id, status) = device.ok_or_else(proof::unknown_device)?;
        match AccountStatus::read(&status)? {
            AccountStatus::Active => {}
            AccountStatus::Banned => return Err(proof::account_banned()),
            AccountStatus::Removed => return Err(proof::unknown_device()),
        }
        if proven.account_id != Some(account_id) {
            return Err(ApiError::new(
                ErrorCode::ChallengeInvalid,
                "the challenge names another account",
            ));
        }
        store::start_session(&app, connection, now, account_id, device_id).await
    })?;
    Ok((StatusCode::CREATED, Json(session)))
}

/// Ends the session of the request.
#[utoipa::path(
    delete,
    path = "/api/v1/auth/session",
    tag = "auth",
    security(("session" = [])),
    responses((status = 204), ApiError)
)]
pub async fn log_out(State(app): State<AppState>, bearer: Bearer) -> ApiResult<StatusCode> {
    let now = app.now();
    transact!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        access.require(Request::OwnAccount)?;
        let session = Sessions::One(access.session_id);
        store::record_ended(connection, &app.calls, now, session, SessionEnd::LoggedOut).await?;
        sqlx::query("delete from sessions where id = $1")
            .bind(access.session_id)
            .execute(&mut *connection)
            .await?;
        let logged_out = Event::DeviceLoggedOut {
            device_id: access.device_id,
        };
        events::tell_account(connection, now, access.account_id, &logged_out).await
    })?;
    Ok(StatusCode::NO_CONTENT)
}

/// Presents the owner code. On a server without an owner, the device becomes the server
/// owner. With a code that the host command issued, the device is added to the account of
/// the owner.
#[utoipa::path(
    post,
    path = "/api/v1/owner/claim",
    tag = "auth",
    request_body = ClaimOwner,
    responses((status = 201, body = SessionCreated), ApiError)
)]
pub async fn claim_owner(
    State(app): State<AppState>,
    ClientAddress(address): ClientAddress,
    ApiJson(body): ApiJson<ClaimOwner>,
) -> ApiResult<(StatusCode, Json<SessionCreated>)> {
    proof::limit_address(&app.join_attempts, address, app.now())?;
    let name = validate::name("name", &body.name)?;
    let device_name = validate::optional_name("device_name", &body.device_name)?;
    let now = app.now();
    let presented = hash_code(CodePurpose::Owner, body.owner_code.expose());

    let session = transact!(app, |connection| {
        let proven = proof::consume(
            &app,
            connection,
            address,
            &body.proof,
            ChallengeOperation::ClaimOwner,
            now,
        )
        .await?;
        let code = owner::locked_code(connection).await?;
        let Some(stored_hash) = &code.hash else {
            return Err(match code.owner {
                Some(_) => ApiError::new(
                    ErrorCode::OwnerAlreadyClaimed,
                    "the server already has an owner",
                ),
                None => owner_code_invalid(),
            });
        };
        let expired = code.expires_at.is_some_and(|expires_at| expires_at <= now);
        if expired || !constant_time_eq(stored_hash, &presented) {
            return Err(owner_code_invalid());
        }
        owner::clear_code(connection).await?;

        let (account_id, device_id) = match code.owner {
            Some(account_id) => {
                let notice = store::recover(
                    connection,
                    &app.calls,
                    now,
                    store::Recovery {
                        account_id,
                        public_key: &proven.public_key,
                        device_name: &device_name,
                        origin: DeviceOrigin::OwnerCode,
                        performed_by: None,
                    },
                )
                .await?;
                let device_id = notice.device_id.ok_or_else(ApiError::not_found)?;
                audit::record(
                    connection,
                    now,
                    None,
                    About::Server,
                    &AuditEvent::OwnerAccessRestored {
                        account_id,
                        device_id,
                    },
                )
                .await?;
                (account_id, device_id)
            }
            None => {
                let account_id = store::create_account(connection, now, &name).await?;
                let device = store::create_device(
                    connection,
                    now,
                    account_id,
                    &proven.public_key,
                    &device_name,
                    DeviceOrigin::OwnerCode,
                )
                .await?;
                sqlx::query("update server_state set owner_account_id = $1")
                    .bind(account_id)
                    .execute(&mut *connection)
                    .await?;
                let claimed = Event::OwnerChanged {
                    account_id,
                    previous_owner_id: None,
                };
                events::publish(connection, now, &claimed).await?;
                let default_workspace = store::default_workspace(connection).await?;
                store::add_members(connection, now, account_id, &[default_workspace]).await?;
                audit::record(
                    connection,
                    now,
                    Some(account_id),
                    About::Server,
                    &AuditEvent::OwnerClaimed { account_id },
                )
                .await?;
                (account_id, device.id)
            }
        };
        store::start_session(&app, connection, now, account_id, device_id).await
    })?;
    Ok((StatusCode::CREATED, Json(session)))
}

fn owner_code_invalid() -> ApiError {
    ApiError::new(ErrorCode::OwnerCodeInvalid, "the owner code is not valid")
}
