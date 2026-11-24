//! The relay of a device link on the first server (ADR 0104). The approval of the link
//! creates it with the offer of the approving device; the new device reads the offer and
//! writes its answer, and the approving device reads the answer. Only these two devices of
//! the account reach it, each through its own session, and it ends after a fixed time.

use axum::{Json, extract::State, http::StatusCode};
use junto_authz::Request;
use junto_protocol::{
    device_relay::{AnswerRelay, RelayAnswer, RelayOffer, Sealed},
    events::Event,
    ids::DeviceId,
};
use sqlx::PgConnection;
use time::OffsetDateTime;

use crate::{
    access::Access,
    db,
    error::{ApiError, ApiResult},
    events,
    extract::{ApiJson, ApiPath, Bearer},
    state::AppState,
    {read, transact},
};

/// The most answers that the new device writes to one relay.
const MOST_ANSWERS: i32 = 256;

/// A relay is reached through the session of a device, never with an access token.
fn device_session(access: &Access) -> ApiResult<()> {
    access.require(Request::OwnAccount)?;
    if access.token_id.is_some() {
        return Err(ApiError::invalid(
            "the relay of a device link needs the session of a device",
        ));
    }
    Ok(())
}

/// Creates the relay of the link of `new_device`, which the caller's device approved in the
/// same transaction.
pub(crate) async fn create(
    app: &AppState,
    connection: &mut PgConnection,
    access: &Access,
    new_device: DeviceId,
    offer: &Sealed,
) -> ApiResult<()> {
    device_session(access)?;
    let now = app.now();
    sqlx::query(
        "insert into device_link_relays
             (new_device_id, account_id, old_device_id, offer, created_at, expires_at)
         values ($1, $2, $3, $4, $5, $6)",
    )
    .bind(new_device)
    .bind(access.account_id)
    .bind(access.device_id)
    .bind(offer.as_bytes())
    .bind(now)
    .bind(now + app.config.limits.device_link_relay_lifetime)
    .execute(&mut *connection)
    .await?;
    events::tell_account(
        connection,
        now,
        access.account_id,
        &Event::DeviceLinkRelayChanged {
            device_id: new_device,
        },
    )
    .await
}

fn sealed(bytes: Vec<u8>) -> ApiResult<Sealed> {
    Sealed::new(bytes).map_err(|error| ApiError::internal(&error))
}

type OfferRow = (DeviceId, Vec<u8>, Vec<u8>, OffsetDateTime);

/// The offer for the calling device, from the device that approved it.
#[utoipa::path(
    get,
    path = "/api/v1/account/device-link-relay",
    tag = "account",
    security(("session" = [])),
    responses((status = 200, body = RelayOffer), ApiError)
)]
pub async fn read_offer(
    State(app): State<AppState>,
    bearer: Bearer,
) -> ApiResult<Json<RelayOffer>> {
    let now = app.now();
    read!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        device_session(&access)?;
        let row: Option<OfferRow> = sqlx::query_as(
            "select r.old_device_id, d.public_key, r.offer, r.expires_at
             from device_link_relays r join devices d on d.id = r.old_device_id
             where r.new_device_id = $1 and r.account_id = $2 and r.expires_at > $3",
        )
        .bind(access.device_id)
        .bind(access.account_id)
        .bind(now)
        .fetch_optional(&mut *connection)
        .await?;
        let (old_device_id, public_key, offer, expires_at) = row.ok_or_else(ApiError::not_found)?;
        Ok(RelayOffer {
            old_device_id,
            old_public_key: db::public_key(&public_key)?,
            offer: sealed(offer)?,
            expires_at,
        })
    })
    .map(Json)
}

/// Writes the answer of the calling device, which replaces its earlier answer. The device
/// that approved it is told.
#[utoipa::path(
    put,
    path = "/api/v1/account/device-link-relay/answer",
    tag = "account",
    request_body = AnswerRelay,
    security(("session" = [])),
    responses((status = 204), ApiError)
)]
pub async fn write_answer(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiJson(body): ApiJson<AnswerRelay>,
) -> ApiResult<StatusCode> {
    let now = app.now();
    transact!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        device_session(&access)?;
        let answers: Option<i32> = sqlx::query_scalar(
            "select answers from device_link_relays
             where new_device_id = $1 and account_id = $2 and expires_at > $3
             for update",
        )
        .bind(access.device_id)
        .bind(access.account_id)
        .bind(now)
        .fetch_optional(&mut *connection)
        .await?;
        let answers = answers.ok_or_else(ApiError::not_found)?;
        if answers >= MOST_ANSWERS {
            return Err(ApiError::invalid(
                "the relay of this device link takes no further answer",
            ));
        }
        sqlx::query(
            "update device_link_relays set answer = $2, answers = answers + 1
             where new_device_id = $1",
        )
        .bind(access.device_id)
        .bind(body.answer.as_bytes())
        .execute(&mut *connection)
        .await?;
        events::tell_account(
            connection,
            now,
            access.account_id,
            &Event::DeviceLinkRelayChanged {
                device_id: access.device_id,
            },
        )
        .await
    })?;
    Ok(StatusCode::NO_CONTENT)
}

type AnswerRow = (Vec<u8>, Option<Vec<u8>>, OffsetDateTime);

/// The answer of the new device `device_id`, for the device that approved it.
#[utoipa::path(
    get,
    path = "/api/v1/account/device-link-relays/{device_id}",
    tag = "account",
    params(("device_id" = DeviceId, Path)),
    security(("session" = [])),
    responses((status = 200, body = RelayAnswer), ApiError)
)]
pub async fn read_answer(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(device_id): ApiPath<DeviceId>,
) -> ApiResult<Json<RelayAnswer>> {
    let now = app.now();
    read!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        device_session(&access)?;
        let row: Option<AnswerRow> = sqlx::query_as(
            "select d.public_key, r.answer, r.expires_at
             from device_link_relays r join devices d on d.id = r.new_device_id
             where r.new_device_id = $1 and r.old_device_id = $2 and r.account_id = $3
                 and r.expires_at > $4",
        )
        .bind(device_id)
        .bind(access.device_id)
        .bind(access.account_id)
        .bind(now)
        .fetch_optional(&mut *connection)
        .await?;
        let (public_key, answer, expires_at) = row.ok_or_else(ApiError::not_found)?;
        Ok(RelayAnswer {
            new_device_id: device_id,
            new_public_key: db::public_key(&public_key)?,
            answer: answer.map(sealed).transpose()?,
            expires_at,
        })
    })
    .map(Json)
}

/// Ends the relay of the new device `device_id`. Either of its two devices ends it.
#[utoipa::path(
    delete,
    path = "/api/v1/account/device-link-relays/{device_id}",
    tag = "account",
    params(("device_id" = DeviceId, Path)),
    security(("session" = [])),
    responses((status = 204), ApiError)
)]
pub async fn end(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(device_id): ApiPath<DeviceId>,
) -> ApiResult<StatusCode> {
    let now = app.now();
    transact!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        device_session(&access)?;
        let ended = sqlx::query(
            "delete from device_link_relays
             where new_device_id = $1 and account_id = $2
                 and (old_device_id = $3 or new_device_id = $3)",
        )
        .bind(device_id)
        .bind(access.account_id)
        .bind(access.device_id)
        .execute(&mut *connection)
        .await?;
        if ended.rows_affected() == 0 {
            return Err(ApiError::not_found());
        }
        Ok(())
    })?;
    Ok(StatusCode::NO_CONTENT)
}
