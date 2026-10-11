use std::ops::Deref;
use std::sync::Arc;

use junto_protocol::ids::{AccountId, ServerId};
use sqlx::{Postgres, Transaction};
use time::OffsetDateTime;

use crate::bodies::BodyBudget;
use crate::calls::Calls;
use crate::clock::Clock;
use crate::config::Config;
use crate::database::{Attempt, Database, Mode, Step};
use crate::error::ApiResult;
use crate::events::hub::Streams;
use crate::media::MediaServer;
use crate::rate_limit::{Outstanding, RateLimiter};
use crate::recordings::{Backend, Recordings};

#[derive(Clone)]
pub struct AppState(Arc<Shared>);

pub struct Shared {
    pub database: Database,
    pub config: Config,
    pub clock: Arc<dyn Clock>,
    pub server_id: ServerId,
    pub join_attempts: RateLimiter,
    pub challenge_requests: RateLimiter,
    pub wrong_signatures: RateLimiter,
    pub redemptions_by_address: RateLimiter,
    pub redemptions_by_account: RateLimiter<AccountId>,
    pub outstanding_challenges: Outstanding,
    pub stream_opens: RateLimiter<AccountId>,
    pub streams: Arc<Streams>,
    pub calls: Calls,
    pub recordings: Recordings,
    pub bodies: Arc<BodyBudget>,
}

impl AppState {
    pub fn new(
        database: Database,
        config: Config,
        clock: Arc<dyn Clock>,
        server_id: ServerId,
        media: Option<Arc<dyn MediaServer>>,
        recorder: Option<Backend>,
    ) -> Self {
        let limits = &config.limits;
        let entries = limits.limiter_entries;
        let join_attempts = RateLimiter::with_capacity(limits.join_attempts, entries);
        let challenge_requests = RateLimiter::with_capacity(limits.challenge_requests, entries);
        let wrong_signatures = RateLimiter::with_capacity(limits.wrong_signatures, entries);
        let redemptions_by_address = RateLimiter::with_capacity(limits.redemptions, entries);
        let redemptions_by_account = RateLimiter::with_capacity(limits.redemptions, entries);
        let outstanding_challenges = Outstanding::new(limits.most_outstanding_challenges);
        let stream_opens = RateLimiter::with_capacity(limits.stream.opens, entries);
        let streams = Arc::new(Streams::new(&limits.stream));
        let calls = Calls::new(media, server_id, limits);
        let recordings = Recordings::new(recorder);
        let bodies = Arc::new(BodyBudget::new(
            limits.request_bodies_held_bytes,
            limits.request_bodies_held_per_address,
        ));
        Self(Arc::new(Shared {
            database,
            config,
            clock,
            server_id,
            join_attempts,
            challenge_requests,
            wrong_signatures,
            redemptions_by_address,
            redemptions_by_account,
            outstanding_challenges,
            stream_opens,
            streams,
            calls,
            recordings,
            bodies,
        }))
    }

    pub async fn begin(
        &self,
        mode: Mode,
        attempt: &Attempt,
    ) -> Result<Transaction<'static, Postgres>, sqlx::Error> {
        self.database.begin(mode, attempt).await
    }

    pub async fn finish<T>(
        &self,
        mode: Mode,
        transaction: Transaction<'static, Postgres>,
        outcome: ApiResult<T>,
        attempt: &mut Attempt,
    ) -> Step<ApiResult<T>> {
        self.database
            .finish(mode, transaction, outcome, attempt)
            .await
    }

    /// The database for the transactions of the event stream that no request waits for.
    /// Used with the same macros as the state itself.
    pub fn unattended(&self) -> Unattended<'_> {
        Unattended(self)
    }

    /// The database for the transactions of a background task that no request waits for,
    /// on the connections of the requests. The commit gate of the tests does not hold them.
    pub fn background(&self) -> Background<'_> {
        Background(self)
    }

    /// The current time, cut to the microsecond that PostgreSQL stores.
    pub fn now(&self) -> OffsetDateTime {
        let now = self.clock.now();
        now.replace_nanosecond(now.microsecond() * 1_000)
            .unwrap_or(now)
    }
}

pub struct Background<'a>(&'a AppState);

impl Background<'_> {
    pub async fn begin(
        &self,
        mode: Mode,
        attempt: &Attempt,
    ) -> Result<Transaction<'static, Postgres>, sqlx::Error> {
        self.0.database.begin(mode, attempt).await
    }

    pub async fn finish<T>(
        &self,
        mode: Mode,
        transaction: Transaction<'static, Postgres>,
        outcome: ApiResult<T>,
        attempt: &mut Attempt,
    ) -> Step<ApiResult<T>> {
        self.0
            .database
            .finish_unattended(mode, transaction, outcome, attempt)
            .await
    }
}

pub struct Unattended<'a>(&'a AppState);

impl Unattended<'_> {
    pub async fn begin(
        &self,
        mode: Mode,
        attempt: &Attempt,
    ) -> Result<Transaction<'static, Postgres>, sqlx::Error> {
        self.0.database.begin_stream(mode, attempt).await
    }

    pub async fn finish<T>(
        &self,
        mode: Mode,
        transaction: Transaction<'static, Postgres>,
        outcome: ApiResult<T>,
        attempt: &mut Attempt,
    ) -> Step<ApiResult<T>> {
        self.0
            .database
            .finish_unattended(mode, transaction, outcome, attempt)
            .await
    }
}

impl Deref for AppState {
    type Target = Shared;

    fn deref(&self) -> &Shared {
        &self.0
    }
}
