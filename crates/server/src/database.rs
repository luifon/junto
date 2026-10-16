use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use sqlx::migrate::{MigrateError, Migrator};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::{Connection, PgPool, Postgres, Transaction};
use tokio::sync::{Notify, Semaphore};

const BEGIN_READ_WRITE: &str = "begin isolation level serializable read write";
const BEGIN_READ_ONLY: &str = "begin isolation level serializable read only";
const DEFAULT_ISOLATION: &str = "default_transaction_isolation";
const ATTEMPTS: u32 = 8;
const FIRST_BACKOFF_CEILING: Duration = Duration::from_millis(4);
const SERIALIZATION_FAILURE: &str = "40001";
const DEADLOCK_DETECTED: &str = "40P01";
const TIME_LIMIT_STATES: [&str; 3] = ["55P03", "57014", "25P03"];

/// An error type that a transaction can end with.
pub trait TransactionError: From<sqlx::Error> {
    /// True when the transaction lost against a concurrent one and can run again.
    fn is_conflict(&self) -> bool;

    /// The error after the last attempt lost as well.
    fn exhausted() -> Self;
}

/// True when PostgreSQL ended a statement because of one of the time limits of [`Timeouts`].
pub fn is_timeout(error: &sqlx::Error) -> bool {
    error
        .as_database_error()
        .and_then(|database| database.code())
        .is_some_and(|code| TIME_LIMIT_STATES.contains(&code.as_ref()))
}

pub fn is_unavailable(error: &sqlx::Error) -> bool {
    matches!(
        error,
        sqlx::Error::Io(_) | sqlx::Error::Tls(_) | sqlx::Error::PoolClosed
    ) || error
        .as_database_error()
        .and_then(|database| database.code())
        .is_some_and(|code| {
            code.starts_with("08") || matches!(code.as_ref(), "57P01" | "57P02" | "57P03")
        })
}

pub fn is_conflict(error: &sqlx::Error) -> bool {
    error
        .as_database_error()
        .and_then(|database| database.code())
        .is_some_and(|code| code == SERIALIZATION_FAILURE || code == DEADLOCK_DETECTED)
}

/// The time limits of the database work of a request: the wait for a free connection of the
/// pool, which the server enforces, and three limits that PostgreSQL enforces on every
/// connection of the server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timeouts {
    /// A request that gets no connection of the pool within this time is answered with
    /// `busy` (ADR 0009). The wait includes the check of an idle connection and the opening
    /// of a new one.
    pub acquire: time::Duration,
    pub statement: time::Duration,
    pub lock: time::Duration,
    pub idle_in_transaction: time::Duration,
}

/// The only way the server reads and writes PostgreSQL. Every transaction is serializable
/// and is run again, a bounded number of times, when it loses against a concurrent one
/// (ADR 0009).
/// The connections that the event stream reads with, apart from the connections of the
/// requests (ADR 0011).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamPool {
    pub connections: u32,
    /// PostgreSQL ends a statement and a wait for a lock of the event stream after this time.
    pub time_limit: time::Duration,
}

pub struct Database {
    pool: PgPool,
    stream_pool: PgPool,
    commit_gate: CommitGate,
    written: Notify,
}

impl Database {
    pub async fn connect(
        options: PgConnectOptions,
        connections: u32,
        timeouts: &Timeouts,
        stream: &StreamPool,
    ) -> Result<Self, sqlx::Error> {
        let milliseconds = |limit: time::Duration| limit.whole_milliseconds().max(1).to_string();
        let limited = |statement, lock, idle_in_transaction| {
            options.clone().options([
                (DEFAULT_ISOLATION, "serializable".to_owned()),
                ("statement_timeout", milliseconds(statement)),
                ("lock_timeout", milliseconds(lock)),
                (
                    "idle_in_transaction_session_timeout",
                    milliseconds(idle_in_transaction),
                ),
            ])
        };
        let pool = PgPoolOptions::new()
            .max_connections(connections)
            .acquire_timeout(timeouts.acquire.unsigned_abs())
            .connect_with(limited(
                timeouts.statement,
                timeouts.lock,
                timeouts.idle_in_transaction,
            ))
            .await?;
        let stream_pool = PgPoolOptions::new()
            .max_connections(stream.connections.max(1))
            .acquire_timeout(stream.time_limit.unsigned_abs())
            .connect_lazy_with(limited(
                stream.time_limit,
                stream.time_limit,
                stream.time_limit,
            ));
        Ok(Self {
            pool,
            stream_pool,
            commit_gate: CommitGate::default(),
            written: Notify::new(),
        })
    }

    /// Applies the migrations on a connection of its own without the time limits of
    /// [`Timeouts`], because a migration can run longer than a request may.
    pub async fn migrate(&self, migrations: &Migrator) -> Result<(), MigrateError> {
        let mut connection = self.pool.acquire().await?.detach();
        for limit in [
            "set statement_timeout = 0",
            "set lock_timeout = 0",
            "set idle_in_transaction_session_timeout = 0",
        ] {
            sqlx::query(limit).execute(&mut connection).await?;
        }
        let migrated = migrations.run(&mut connection).await;
        connection.close().await?;
        migrated
    }

    pub async fn acquire_for_transaction(
        &self,
        attempt: &Attempt,
    ) -> Result<sqlx::pool::PoolConnection<Postgres>, sqlx::Error> {
        self.acquire_on(&self.pool, attempt).await
    }

    pub async fn acquire_for_stream(
        &self,
        attempt: &Attempt,
    ) -> Result<sqlx::pool::PoolConnection<Postgres>, sqlx::Error> {
        self.acquire_on(&self.stream_pool, attempt).await
    }

    async fn acquire_on(
        &self,
        pool: &PgPool,
        attempt: &Attempt,
    ) -> Result<sqlx::pool::PoolConnection<Postgres>, sqlx::Error> {
        if attempt.0 > 0 {
            tokio::time::sleep(backoff(attempt.0)).await;
        }
        pool.acquire().await
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    pub fn commit_gate(&self) -> &CommitGate {
        &self.commit_gate
    }

    pub async fn close(&self) {
        self.stream_pool.close().await;
        self.pool.close().await;
    }

    /// Resolves after a transaction that can write has committed in this process.
    pub async fn written(&self) {
        self.written.notified().await;
    }

    /// Ends one attempt: commits after success, and asks for another attempt when the
    /// transaction lost against a concurrent one.
    pub async fn finish<T, E>(
        &self,
        mode: Mode,
        transaction: Transaction<'_, Postgres>,
        outcome: Result<T, E>,
        attempt: &mut Attempt,
    ) -> Step<Result<T, E>>
    where
        E: TransactionError,
    {
        self.end(mode, Some(&self.commit_gate), transaction, outcome, attempt)
            .await
    }

    /// Like [`Database::finish`], for the work of the event stream that no request waits
    /// for: the commit gate of the tests does not count or hold the transaction.
    pub async fn finish_unattended<T, E>(
        &self,
        mode: Mode,
        transaction: Transaction<'_, Postgres>,
        outcome: Result<T, E>,
        attempt: &mut Attempt,
    ) -> Step<Result<T, E>>
    where
        E: TransactionError,
    {
        self.end(mode, None, transaction, outcome, attempt).await
    }

    async fn end<T, E>(
        &self,
        mode: Mode,
        gate: Option<&CommitGate>,
        transaction: Transaction<'_, Postgres>,
        outcome: Result<T, E>,
        attempt: &mut Attempt,
    ) -> Step<Result<T, E>>
    where
        E: TransactionError,
    {
        match outcome {
            Ok(_) if gate.is_some_and(CommitGate::loses) => rollback(transaction).await,
            Ok(value) => {
                if let Some(gate) = gate {
                    gate.pass().await;
                }
                match transaction.commit().await {
                    Ok(()) => {
                        if matches!(mode, Mode::ReadWrite) {
                            self.written.notify_one();
                        }
                        return Step::Done(Ok(value));
                    }
                    Err(error)
                        if is_conflict(&error)
                            || error
                                .as_database_error()
                                .is_some_and(|e| e.is_unique_violation()) => {}
                    Err(error) => return Step::Done(Err(error.into())),
                }
            }
            Err(error) => {
                rollback(transaction).await;
                if !error.is_conflict() {
                    return Step::Done(Err(error));
                }
            }
        }
        attempt.0 += 1;
        if attempt.0 < ATTEMPTS {
            Step::Again
        } else {
            Step::Done(Err(E::exhausted()))
        }
    }
}

pub async fn begin_on_connection(
    connection: &mut sqlx::PgConnection,
    mode: Mode,
) -> Result<Transaction<'_, Postgres>, sqlx::Error> {
    let mut transaction = connection
        .begin_with(match mode {
            Mode::ReadWrite => BEGIN_READ_WRITE,
            Mode::ReadOnly => BEGIN_READ_ONLY,
        })
        .await?;
    if matches!(mode, Mode::ReadWrite) && crate::receipts::requested() {
        sqlx::query("select set_config('junto.event_first', '', false), set_config('junto.event_cursor', '', false)")
            .execute(&mut *transaction)
            .await?;
    }
    Ok(transaction)
}

/// Ends the transaction before the connection goes back to the pool, so that no response is
/// sent while its backend is still inside a transaction.
async fn rollback(transaction: Transaction<'_, Postgres>) {
    if let Err(error) = transaction.rollback().await {
        tracing::warn!(%error, "a transaction could not be rolled back");
    }
}

#[derive(Debug, Clone, Copy)]
pub enum Mode {
    ReadWrite,
    ReadOnly,
}

/// Counts the attempts of one transaction.
#[derive(Debug, Default)]
pub struct Attempt(u32);

pub enum Step<R> {
    Done(R),
    Again,
}

/// Runs a block in one transaction that can write, and evaluates to its `Result`.
///
/// The block runs again from its start when the transaction loses against a concurrent one
/// (ADR 0009), so it must have no effect outside the database. `$database` is a
/// [`Database`] or the application state.
#[macro_export]
macro_rules! transact {
    ($database:expr, |$connection:ident| $body:block) => {
        $crate::in_transaction!(
            $database,
            $crate::database::Mode::ReadWrite,
            |$connection| $body
        )
    };
}

/// Like [`transact!`], in a transaction that cannot write.
#[macro_export]
macro_rules! read {
    ($database:expr, |$connection:ident| $body:block) => {
        $crate::in_transaction!($database, $crate::database::Mode::ReadOnly, |$connection| {
            $body
        })
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! in_transaction {
    ($database:expr, $mode:expr, |$connection:ident| $body:block) => {{
        let mut attempt = $crate::database::Attempt::default();
        loop {
            let mut held = $database.acquire_for_transaction(&attempt).await?;
            let mut transaction = $crate::database::begin_on_connection(&mut held, $mode).await?;
            let outcome = {
                let $connection: &mut ::sqlx::PgConnection = &mut transaction;
                async { $body }.await
            };
            match $database
                .finish($mode, transaction, outcome, &mut attempt)
                .await
            {
                $crate::database::Step::Done(result) => {
                    if result.is_ok() && matches!($mode, $crate::database::Mode::ReadWrite) {
                        $crate::receipts::committed(&mut held).await;
                    }
                    break result;
                }
                $crate::database::Step::Again => continue,
            }
        }
    }};
}

fn backoff(attempt: u32) -> Duration {
    let ceiling = FIRST_BACKOFF_CEILING * 2u32.pow(attempt.min(6));
    let random = getrandom::u64().unwrap_or(u64::MAX);
    let nanos = u64::try_from(ceiling.as_nanos()).unwrap_or(u64::MAX);
    Duration::from_nanos(random % nanos.max(1))
}

/// Lets a test hold one transaction after its work and before its commit. Nothing in the
/// server arms it.
#[derive(Default)]
pub struct CommitGate {
    losses: AtomicUsize,
    countdown: AtomicUsize,
    arrived: Signal,
    released: Signal,
}

struct Signal(Semaphore);

impl Default for Signal {
    fn default() -> Self {
        Self(Semaphore::new(0))
    }
}

impl CommitGate {
    /// Holds the transaction that reaches its commit after `skipped` others have passed.
    pub fn arm(&self, skipped: usize) {
        self.countdown.store(skipped + 1, Ordering::SeqCst);
    }

    /// Makes the next `count` transactions that reach their commit lose, as if each had met
    /// a serialization failure.
    pub fn lose_next(&self, count: usize) {
        self.losses.store(count, Ordering::SeqCst);
    }

    fn loses(&self) -> bool {
        self.losses
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |left| {
                left.checked_sub(1)
            })
            .is_ok()
    }

    /// Waits until the armed transaction is held.
    pub async fn arrived(&self) {
        if let Ok(permit) = self.arrived.0.acquire().await {
            permit.forget();
        }
    }

    pub fn release(&self) {
        self.released.0.add_permits(1);
    }

    async fn pass(&self) {
        let reached = self
            .countdown
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |left| {
                left.checked_sub(1)
            });
        if reached == Ok(1) {
            self.arrived.0.add_permits(1);
            if let Ok(permit) = self.released.0.acquire().await {
                permit.forget();
            }
        }
    }
}
