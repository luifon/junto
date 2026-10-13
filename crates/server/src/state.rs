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
    pub account_changes: RateLimiter<AccountId>,
    pub logo_reads: RateLimiter,
    pub audit_reads: RateLimiter<AccountId>,
    pub link_code_attempts: RateLimiter<AccountId>,
    pub outstanding_challenges: Outstanding,
    pub attachment_writes: RateLimiter<[u8; 32]>,
    /// The disk of the attachments, whose free space the storage guard reads.
    pub attachment_space: crate::storage::Space,
    pub attachment_checks: Arc<crate::messages::transfers::Checks>,
    pub attachment_downloads: Arc<crate::messages::transfers::Transfers>,
    pub attachment_transfers: Arc<crate::messages::transfers::Transfers>,
    pub message_history: RateLimiter<AccountId>,
    /// The reads of one message, which a client makes for each `message_changed` event
    /// (ADR 0408).
    pub message_reads: RateLimiter<AccountId>,
    pub message_writes: RateLimiter<AccountId>,
    pub stream_opens: RateLimiter<AccountId>,
    pub streams: Arc<Streams>,
    pub calls: Calls,
    pub recordings: Recordings,
    pub exports: crate::exports::Exports,
    pub purges: crate::purge::Purges,
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
        let account_changes = RateLimiter::with_capacity(limits.account_changes, entries);
        let link_code_attempts = RateLimiter::with_capacity(limits.link_code_attempts, entries);
        let outstanding_challenges = Outstanding::new(limits.most_outstanding_challenges);
        let message_writes = RateLimiter::with_capacity(
            crate::config::RateLimit {
                attempts: 120,
                window: time::Duration::minutes(1),
            },
            entries,
        );
        let stream_opens = RateLimiter::with_capacity(limits.stream.opens, entries);
        let streams = Arc::new(Streams::new(&limits.stream));
        let calls = Calls::new(media, server_id, limits);
        let recordings = Recordings::new(recorder, &limits.recordings);
        let exports = crate::exports::Exports::new(config.exports_directory.clone());
        let attachment_space = crate::storage::Space::of(config.attachments_directory.clone());
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
            account_changes,
            logo_reads: RateLimiter::with_capacity(
                crate::config::RateLimit {
                    attempts: 120,
                    window: time::Duration::minutes(1),
                },
                entries,
            ),
            audit_reads: RateLimiter::with_capacity(
                crate::config::RateLimit {
                    attempts: 120,
                    window: time::Duration::minutes(1),
                },
                entries,
            ),
            link_code_attempts,
            outstanding_challenges,
            attachment_writes: RateLimiter::with_capacity(
                crate::config::RateLimit {
                    attempts: 1200,
                    window: time::Duration::minutes(1),
                },
                entries,
            ),
            attachment_space,
            attachment_checks: Arc::new(crate::messages::transfers::Checks::default()),
            attachment_downloads: Arc::new(
                crate::messages::transfers::Transfers::attachment_downloads(),
            ),
            attachment_transfers: Arc::new(
                crate::messages::transfers::Transfers::attachment_uploads(),
            ),
            message_history: RateLimiter::with_capacity(
                crate::config::RateLimit {
                    attempts: 120,
                    window: time::Duration::minutes(1),
                },
                entries,
            ),
            message_reads: RateLimiter::with_capacity(
                crate::config::RateLimit {
                    attempts: 1200,
                    window: time::Duration::minutes(1),
                },
                entries,
            ),
            message_writes,
            stream_opens,
            streams,
            calls,
            recordings,
            exports,
            purges: crate::purge::Purges::default(),
            bodies,
        }))
    }

    pub async fn acquire_for_transaction(
        &self,
        attempt: &Attempt,
    ) -> Result<sqlx::pool::PoolConnection<Postgres>, sqlx::Error> {
        self.database.acquire_for_transaction(attempt).await
    }

    pub async fn finish<T>(
        &self,
        mode: Mode,
        transaction: Transaction<'_, Postgres>,
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

    /// The free-space reserve shared by recordings, attachments and exports (ADR 0006).
    pub fn storage_reserve_bytes(&self) -> u64 {
        self.config.limits.recordings.reserve_bytes
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
    pub async fn acquire_for_transaction(
        &self,
        attempt: &Attempt,
    ) -> Result<sqlx::pool::PoolConnection<Postgres>, sqlx::Error> {
        self.0.database.acquire_for_transaction(attempt).await
    }

    pub async fn finish<T>(
        &self,
        mode: Mode,
        transaction: Transaction<'_, Postgres>,
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
    pub async fn acquire_for_transaction(
        &self,
        attempt: &Attempt,
    ) -> Result<sqlx::pool::PoolConnection<Postgres>, sqlx::Error> {
        self.0.database.acquire_for_stream(attempt).await
    }

    pub async fn finish<T>(
        &self,
        mode: Mode,
        transaction: Transaction<'_, Postgres>,
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
