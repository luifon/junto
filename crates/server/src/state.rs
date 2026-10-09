use std::ops::Deref;
use std::sync::Arc;

use junto_protocol::ids::{AccountId, ServerId};
use sqlx::{Postgres, Transaction};
use time::OffsetDateTime;

use crate::clock::Clock;
use crate::config::Config;
use crate::database::{Attempt, Database, Mode, Step};
use crate::error::ApiResult;
use crate::rate_limit::{Outstanding, RateLimiter};

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
}

impl AppState {
    pub fn new(
        database: Database,
        config: Config,
        clock: Arc<dyn Clock>,
        server_id: ServerId,
    ) -> Self {
        let limits = &config.limits;
        let entries = limits.limiter_entries;
        let join_attempts = RateLimiter::with_capacity(limits.join_attempts, entries);
        let challenge_requests = RateLimiter::with_capacity(limits.challenge_requests, entries);
        let wrong_signatures = RateLimiter::with_capacity(limits.wrong_signatures, entries);
        let redemptions_by_address = RateLimiter::with_capacity(limits.redemptions, entries);
        let redemptions_by_account = RateLimiter::with_capacity(limits.redemptions, entries);
        let outstanding_challenges = Outstanding::new(limits.most_outstanding_challenges);
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
        transaction: Transaction<'static, Postgres>,
        outcome: ApiResult<T>,
        attempt: &mut Attempt,
    ) -> Step<ApiResult<T>> {
        self.database.finish(transaction, outcome, attempt).await
    }

    /// The current time, cut to the microsecond that PostgreSQL stores.
    pub fn now(&self) -> OffsetDateTime {
        let now = self.clock.now();
        now.replace_nanosecond(now.microsecond() * 1_000)
            .unwrap_or(now)
    }
}

impl Deref for AppState {
    type Target = Shared;

    fn deref(&self) -> &Shared {
        &self.0
    }
}
