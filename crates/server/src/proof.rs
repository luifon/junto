use std::net::IpAddr;

use ed25519_dalek::{Signature, VerifyingKey};
use junto_protocol::account::AccountStatus;
use junto_protocol::auth::{Challenge, ChallengeOperation, CreateChallenge, Proof};
use junto_protocol::error::ErrorCode;
use junto_protocol::ids::{AccountId, ChallengeId};
use junto_protocol::keys::{Nonce, PublicKey};
use sqlx::PgConnection;
use time::OffsetDateTime;

use crate::db::{self, StoredText};
use crate::error::{ApiError, ApiResult};
use crate::rate_limit::RateLimiter;
use crate::secrets::random_bytes;
use crate::state::AppState;
use crate::transact;

/// A device key whose holder answered a challenge of the expected operation.
#[derive(Debug, Clone, Copy)]
pub struct Proven {
    pub public_key: PublicKey,
    pub account_id: Option<AccountId>,
}

type StoredChallenge = (Vec<u8>, String, Option<AccountId>, Vec<u8>, OffsetDateTime);

pub fn limit<K>(limiter: &RateLimiter<K>, key: K, now: OffsetDateTime) -> ApiResult<()>
where
    K: std::hash::Hash + Eq + Copy,
{
    limiter.attempt_of(key, now).map_err(|exceeded| {
        ApiError::new(ErrorCode::RateLimited, "too many attempts").retry_after(exceeded.retry_after)
    })
}

pub fn limit_address(limiter: &RateLimiter, address: IpAddr, now: OffsetDateTime) -> ApiResult<()> {
    limiter.attempt(address, now).map_err(|exceeded| {
        ApiError::new(
            ErrorCode::RateLimited,
            "too many attempts from this address",
        )
        .retry_after(exceeded.retry_after)
    })
}

pub async fn issue(app: &AppState, request: &CreateChallenge) -> ApiResult<Challenge> {
    if VerifyingKey::from_bytes(request.public_key.as_bytes()).is_err() {
        return Err(ApiError::invalid("public_key is not an Ed25519 public key"));
    }
    let now = app.now();
    let lifetime = app.config.limits.challenge_lifetime;
    let counted_until = now + lifetime;
    app.outstanding_challenges
        .reserve(now, counted_until)
        .map_err(|exceeded| {
            ApiError::new(
                ErrorCode::ChallengesFull,
                "too many challenges wait for an answer on this server",
            )
            .retry_after(exceeded.retry_after)
        })?;
    let stored = store(app, request, now).await;
    if stored.is_err() {
        app.outstanding_challenges.release(counted_until);
    }
    stored
}

async fn store(
    app: &AppState,
    request: &CreateChallenge,
    now: OffsetDateTime,
) -> ApiResult<Challenge> {
    let key = request.public_key.as_bytes().as_slice();
    let whole_second = now.replace_nanosecond(0).unwrap_or(now);
    let nonce = Nonce::from_bytes(random_bytes()?);
    let id = ChallengeId::generate();

    transact!(app, |connection| {
        let account_id = match request.operation {
            ChallengeOperation::Login => {
                let device: Option<(AccountId, String)> = sqlx::query_as(
                    "select a.id, a.status from devices d
                     join accounts a on a.id = d.account_id
                     where d.public_key = $1",
                )
                .bind(key)
                .fetch_optional(&mut *connection)
                .await?;
                let (account_id, status) = device.ok_or_else(unknown_device)?;
                match AccountStatus::read(&status)? {
                    AccountStatus::Active => Some(account_id),
                    AccountStatus::Banned => return Err(account_banned()),
                    AccountStatus::Removed => return Err(unknown_device()),
                }
            }
            ChallengeOperation::ClaimOwner
            | ChallengeOperation::Join
            | ChallengeOperation::RequestToJoin
            | ChallengeOperation::JoinRequestStatus
            | ChallengeOperation::LinkDevice => None,
        };
        let challenge = Challenge {
            id,
            server_id: app.server_id,
            operation: request.operation,
            account_id,
            public_key: request.public_key,
            nonce,
            expires_at: whole_second + app.config.limits.challenge_lifetime,
        };
        sqlx::query(
            "insert into challenges (id, public_key, operation, account_id, nonce, expires_at)
             values ($1, $2, $3, $4, $5, $6)
             on conflict on constraint challenges_one_per_key_and_operation do update set
                 id = excluded.id,
                 account_id = excluded.account_id,
                 nonce = excluded.nonce,
                 expires_at = excluded.expires_at,
                 consumed_at = null",
        )
        .bind(challenge.id)
        .bind(key)
        .bind(challenge.operation.stored())
        .bind(challenge.account_id)
        .bind(challenge.nonce.as_bytes().as_slice())
        .bind(challenge.expires_at)
        .execute(&mut *connection)
        .await?;
        Ok(challenge)
    })
}

/// Checks the signature of `proof` and marks its challenge as used, inside the transaction
/// of the operation that the challenge authorizes.
///
/// The challenge is used when that transaction commits. When the transaction does not
/// commit, the challenge is unused and the same signed request can be sent again until the
/// challenge expires. A valid signature for an unexpired, unused challenge of `operation`
/// always succeeds. A wrong signature counts against `address` only.
pub async fn consume(
    app: &AppState,
    connection: &mut PgConnection,
    address: IpAddr,
    proof: &Proof,
    operation: ChallengeOperation,
    now: OffsetDateTime,
) -> ApiResult<Proven> {
    let used: Option<StoredChallenge> = sqlx::query_as(
        "update challenges set consumed_at = $2
         where id = $1 and consumed_at is null
         returning public_key, operation, account_id, nonce, expires_at",
    )
    .bind(proof.challenge_id)
    .bind(now)
    .fetch_optional(connection)
    .await?;
    let Some((public_key, stored_operation, account_id, nonce, expires_at)) = used else {
        return Err(challenge_invalid());
    };
    let challenge = Challenge {
        id: proof.challenge_id,
        server_id: app.server_id,
        operation: ChallengeOperation::read(&stored_operation)?,
        account_id,
        public_key: db::public_key(&public_key)?,
        nonce: Nonce::try_from(nonce.as_slice()).map_err(|error| ApiError::internal(&error))?,
        expires_at,
    };

    if challenge.operation != operation {
        return Err(challenge_invalid());
    }
    if challenge.expires_at <= now {
        return Err(ApiError::new(
            ErrorCode::ChallengeExpired,
            "the challenge has expired",
        ));
    }
    if signature_is_valid(&challenge, proof) {
        return Ok(Proven {
            public_key: challenge.public_key,
            account_id: challenge.account_id,
        });
    }
    Err(match app.wrong_signatures.attempt(address, now) {
        Ok(()) => ApiError::new(
            ErrorCode::InvalidSignature,
            "the signature does not match the challenge",
        ),
        Err(exceeded) => ApiError::new(
            ErrorCode::TooManyAttempts,
            "too many wrong signatures from this address",
        )
        .retry_after(exceeded.retry_after),
    })
}

fn signature_is_valid(challenge: &Challenge, proof: &Proof) -> bool {
    let Ok(key) = VerifyingKey::from_bytes(challenge.public_key.as_bytes()) else {
        return false;
    };
    let signature = Signature::from_bytes(proof.signature.as_bytes());
    key.verify_strict(&challenge.signing_input(), &signature)
        .is_ok()
}

fn challenge_invalid() -> ApiError {
    ApiError::new(
        ErrorCode::ChallengeInvalid,
        "the challenge is unknown, already used or for another operation",
    )
}

pub fn unknown_device() -> ApiError {
    ApiError::new(
        ErrorCode::UnknownDevice,
        "the device key belongs to no account",
    )
}

pub fn account_banned() -> ApiError {
    ApiError::new(ErrorCode::AccountBanned, "the account is banned")
}
