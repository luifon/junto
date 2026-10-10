//! The connections of this server process: their ceilings, their wake-ups and the accounts
//! that are in a call (ADR 0116). Nothing here decides what a connection may receive.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use junto_protocol::error::ErrorCode;
use junto_protocol::events::Level;
use junto_protocol::ids::{AccountId, ChannelId, DeviceId};
use tokio::sync::{Notify, Semaphore, watch};
use tokio::task::AbortHandle;

use crate::config::StreamLimits;
use crate::error::ApiError;

const NOT_SUBSCRIBED: u8 = 0;
const FULL: u8 = 1;
const BACKGROUND: u8 = 2;
const RELEASE_CHECK: Duration = Duration::from_millis(10);
const AFTER_ABORT: Duration = Duration::from_secs(1);

/// What the registry knows about one connection.
pub struct Link {
    id: u64,
    account_id: AccountId,
    device_id: DeviceId,
    level: AtomicU8,
    news: Notify,
    attention: Notify,
    replaced: AtomicBool,
    stopping: AtomicBool,
    task: Mutex<Option<AbortHandle>>,
}

impl Link {
    /// Resolves when the log can hold events that the connection has not read.
    pub async fn news(&self) {
        self.news.notified().await;
    }

    /// Resolves when the connection must stop what it does and look at its state: it was
    /// replaced, the server stops, or its session can have ended.
    pub async fn attention(&self) {
        self.attention.notified().await;
    }

    pub fn is_replaced(&self) -> bool {
        self.replaced.load(Ordering::SeqCst)
    }

    pub fn is_stopping(&self) -> bool {
        self.stopping.load(Ordering::SeqCst)
    }

    pub fn set_level(&self, level: Option<Level>) {
        let stored = match level {
            None => NOT_SUBSCRIBED,
            Some(Level::Full) => FULL,
            Some(Level::Background) => BACKGROUND,
        };
        self.level.store(stored, Ordering::SeqCst);
    }

    /// States the task that serves the connection, so that a stop of the server can end it.
    pub fn served_by(&self, task: AbortHandle) {
        *lock(&self.task) = Some(task);
    }
}

/// The new events that the watcher found, reduced to what decides a wake-up.
#[derive(Debug, Default)]
pub struct Arrivals {
    /// An event of the general class that is not addressed to one account.
    pub general: bool,
    /// An event of another class that is not addressed to one account.
    pub other: bool,
    pub accounts: HashSet<AccountId>,
    pub revalidate: HashSet<AccountId>,
}

impl Arrivals {
    pub fn everything() -> Self {
        Self {
            general: true,
            other: true,
            ..Self::default()
        }
    }
}

#[derive(Default)]
struct Registry {
    next_id: u64,
    by_account: HashMap<AccountId, Vec<Arc<Link>>>,
    stopping: bool,
}

/// The place of a call, as the audience of an event names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CallKey {
    Channel(ChannelId),
    /// A direct-message call, the smaller account identifier first.
    Direct(AccountId, AccountId),
}

/// The accounts that are in a call and the place of each call. A connection of such an
/// account receives every event of that call at any level (ADR 0107). The call routes enter
/// and remove the accounts, and every admission check replaces the list with the content of
/// the database (ADR 0509). The list widens the classes that a connection reads; it never
/// decides whether an account may receive an event.
#[derive(Default)]
pub struct CallRegistry {
    inner: Mutex<InCalls>,
}

#[derive(Default)]
struct InCalls {
    by_account: HashMap<AccountId, CallKey>,
    generation: u64,
}

impl CallRegistry {
    pub fn enter(&self, account_id: AccountId, key: CallKey) {
        let mut inner = lock(&self.inner);
        inner.by_account.insert(account_id, key);
        inner.generation += 1;
    }

    pub fn leave(&self, account_id: AccountId) {
        let mut inner = lock(&self.inner);
        inner.by_account.remove(&account_id);
        inner.generation += 1;
    }

    pub fn key_of(&self, account_id: AccountId) -> Option<CallKey> {
        lock(&self.inner).by_account.get(&account_id).copied()
    }

    /// A number that changes with every entry and every leave.
    pub fn generation(&self) -> u64 {
        lock(&self.inner).generation
    }

    /// Replaces the list with `members`, which were read from the database after
    /// `generation` was taken. When a request changed the list in between, the list stays
    /// as it is and the next admission check replaces it.
    pub fn replace_since(&self, generation: u64, members: HashMap<AccountId, CallKey>) -> bool {
        let mut inner = lock(&self.inner);
        if inner.generation != generation {
            return false;
        }
        inner.by_account = members;
        true
    }
}

/// The kinds of reads of the database that a connection makes. Each kind has its own places,
/// so that slow batches do not delay the checks of the sessions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lane {
    /// A batch of events, or the answer to a `subscribe`.
    Events,
    /// A check of the session.
    Session,
}

pub struct Streams {
    limits: Mutex<Arc<StreamLimits>>,
    registry: Mutex<Registry>,
    event_reads: Arc<Semaphore>,
    session_reads: Arc<Semaphore>,
    held: watch::Sender<bool>,
    calls: CallRegistry,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Streams {
    pub fn new(limits: &StreamLimits) -> Self {
        Self {
            limits: Mutex::new(Arc::new(limits.clone())),
            registry: Mutex::new(Registry::default()),
            event_reads: Arc::new(Semaphore::new(limits.concurrent_deliveries.max(1))),
            session_reads: Arc::new(Semaphore::new(limits.concurrent_session_checks.max(1))),
            held: watch::Sender::new(false),
            calls: CallRegistry::default(),
        }
    }

    pub fn calls(&self) -> &CallRegistry {
        &self.calls
    }

    /// The limits of the event stream. A connection reads its heartbeat and its idle time
    /// when it opens, and the other time limits whenever it uses them.
    pub fn limits(&self) -> Arc<StreamLimits> {
        lock(&self.limits).clone()
    }

    /// Replaces the limits of the event stream while the server runs. Only tests do this:
    /// a test shortens a time limit for the step that it examines, after its preparation
    /// ran under the normal limits (ADR 0904).
    pub fn set_limits(&self, limits: StreamLimits) {
        *lock(&self.limits) = Arc::new(limits);
    }

    /// Admits a connection of a device. A device at its ceiling loses its oldest connection;
    /// an account at its ceiling is refused. A connection that was told to close counts for
    /// its account until its task has ended.
    pub fn open(
        self: &Arc<Self>,
        account_id: AccountId,
        device_id: DeviceId,
    ) -> Result<Registration, ApiError> {
        let limits = self.limits();
        let mut registry = lock(&self.registry);
        if registry.stopping {
            return Err(ApiError::new(
                ErrorCode::Unavailable,
                "the server is stopping",
            ));
        }
        let connections = registry.by_account.entry(account_id).or_default();
        if connections.len() >= limits.connections_per_account.max(1) {
            return Err(ApiError::new(
                ErrorCode::TooManyConnections,
                "the account has too many connections to the event stream",
            ));
        }
        let in_use = |link: &&Arc<Link>| link.device_id == device_id && !link.is_replaced();
        if connections.iter().filter(in_use).count() >= limits.connections_per_device.max(1)
            && let Some(oldest) = connections.iter().find(in_use)
        {
            oldest.replaced.store(true, Ordering::SeqCst);
            oldest.attention.notify_one();
        }
        registry.next_id += 1;
        let link = Arc::new(Link {
            id: registry.next_id,
            account_id,
            device_id,
            level: AtomicU8::new(NOT_SUBSCRIBED),
            news: Notify::new(),
            attention: Notify::new(),
            replaced: AtomicBool::new(false),
            stopping: AtomicBool::new(false),
            task: Mutex::new(None),
        });
        registry
            .by_account
            .entry(account_id)
            .or_default()
            .push(link.clone());
        Ok(Registration {
            streams: self.clone(),
            link,
        })
    }

    fn close(&self, link: &Link) {
        let mut registry = lock(&self.registry);
        if let Some(connections) = registry.by_account.get_mut(&link.account_id) {
            connections.retain(|other| other.id != link.id);
            if connections.is_empty() {
                registry.by_account.remove(&link.account_id);
            }
        }
    }

    /// Wakes the connections that the new events can concern and makes the connections of
    /// the accounts in `revalidate` check their session.
    pub fn wake(&self, arrivals: &Arrivals) {
        let registry = lock(&self.registry);
        for account_id in &arrivals.revalidate {
            for link in registry.by_account.get(account_id).into_iter().flatten() {
                link.attention.notify_one();
            }
        }
        for (account_id, connections) in &registry.by_account {
            let addressed = arrivals.accounts.contains(account_id);
            if !(addressed || arrivals.other || arrivals.general) {
                continue;
            }
            let in_call = self.calls.key_of(*account_id).is_some();
            for link in connections {
                let wanted = match link.level.load(Ordering::SeqCst) {
                    FULL => true,
                    BACKGROUND => addressed || arrivals.other || in_call,
                    _ => false,
                };
                if wanted {
                    link.news.notify_one();
                }
            }
        }
    }

    /// Makes every connection close and refuses new ones.
    pub fn shut_down(&self) {
        let mut registry = lock(&self.registry);
        registry.stopping = true;
        for link in registry.by_account.values().flatten() {
            link.stopping.store(true, Ordering::SeqCst);
            link.attention.notify_one();
        }
    }

    /// Closes every connection and waits until the task of each has ended, its close
    /// handshake included. The tasks that have not ended after `limit` are aborted.
    pub async fn stop(&self, limit: Duration) {
        self.shut_down();
        if tokio::time::timeout(limit, self.released()).await.is_ok() {
            return;
        }
        let aborted = self.abort();
        tracing::warn!(
            aborted,
            "connections of the event stream did not close in time"
        );
        let _ = tokio::time::timeout(AFTER_ABORT, self.released()).await;
    }

    async fn released(&self) {
        while self.connections() > 0 {
            tokio::time::sleep(RELEASE_CHECK).await;
        }
    }

    fn abort(&self) -> usize {
        let registry = lock(&self.registry);
        let mut aborted = 0;
        for link in registry.by_account.values().flatten() {
            if let Some(task) = lock(&link.task).as_ref() {
                task.abort();
                aborted += 1;
            }
        }
        aborted
    }

    /// The connections whose tasks have not ended, the ones that are closing included.
    pub fn connections(&self) -> usize {
        lock(&self.registry).by_account.values().map(Vec::len).sum()
    }

    pub fn connections_of(&self, account_id: AccountId) -> usize {
        lock(&self.registry)
            .by_account
            .get(&account_id)
            .map_or(0, Vec::len)
    }

    /// The places of the reads of one kind. A read holds its place until its work in the
    /// database has ended, also when the connection no longer waits for it (ADR 0011).
    pub fn places(&self, lane: Lane) -> Arc<Semaphore> {
        match lane {
            Lane::Events => self.event_reads.clone(),
            Lane::Session => self.session_reads.clone(),
        }
    }

    /// Stops the delivery of events on every connection until [`Streams::release`]. Only
    /// tests hold deliveries (ADR 0904).
    pub fn hold(&self) {
        self.held.send_replace(true);
    }

    pub fn release(&self) {
        self.held.send_replace(false);
    }

    pub fn held(&self) -> watch::Receiver<bool> {
        self.held.subscribe()
    }
}

/// The place of one connection in the registry. It is held until the task of the connection
/// ends, and dropping it frees the place.
pub struct Registration {
    streams: Arc<Streams>,
    link: Arc<Link>,
}

impl Registration {
    pub fn link(&self) -> &Arc<Link> {
        &self.link
    }
}

impl Drop for Registration {
    fn drop(&mut self) {
        self.streams.close(&self.link);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn streams(per_device: usize, per_account: usize) -> Arc<Streams> {
        Arc::new(Streams::new(&StreamLimits {
            connections_per_device: per_device,
            connections_per_account: per_account,
            ..StreamLimits::default()
        }))
    }

    #[test]
    fn an_account_is_in_at_most_one_call() {
        let calls = CallRegistry::default();
        let account_id = AccountId::generate();
        let first = CallKey::Channel(ChannelId::generate());
        let second = CallKey::Direct(account_id, AccountId::generate());
        assert_eq!(calls.key_of(account_id), None);
        calls.enter(account_id, first);
        calls.enter(account_id, second);
        assert_eq!(calls.key_of(account_id), Some(second));
        calls.leave(account_id);
        assert_eq!(calls.key_of(account_id), None);
        let read_at = calls.generation();
        assert!(calls.replace_since(read_at, HashMap::from([(account_id, first)])));
        assert_eq!(calls.key_of(account_id), Some(first));
        let read_at = calls.generation();
        calls.enter(account_id, second);
        assert!(!calls.replace_since(read_at, HashMap::new()));
        assert_eq!(calls.key_of(account_id), Some(second));
        assert!(calls.replace_since(calls.generation(), HashMap::new()));
        assert_eq!(calls.key_of(account_id), None);
    }

    #[test]
    fn a_replaced_connection_counts_for_its_account_until_it_is_released() {
        let streams = streams(1, 3);
        let account_id = AccountId::generate();
        let device_id = DeviceId::generate();

        let first = streams.open(account_id, device_id).unwrap();
        let second = streams.open(account_id, device_id).unwrap();
        assert!(first.link().is_replaced());
        assert!(!second.link().is_replaced());
        assert_eq!(streams.connections_of(account_id), 2);

        let third = streams.open(account_id, device_id).unwrap();
        assert!(second.link().is_replaced());
        let refused = streams.open(account_id, device_id).err().unwrap();
        assert_eq!(refused.code(), ErrorCode::TooManyConnections);
        assert!(!third.link().is_replaced());

        drop(first);
        assert_eq!(streams.connections_of(account_id), 2);
        let fourth = streams.open(account_id, device_id).unwrap();
        assert!(third.link().is_replaced());
        drop((second, third, fourth));
        assert_eq!(streams.connections(), 0);
    }

    #[test]
    fn another_device_is_refused_at_the_ceiling_of_the_account_and_closes_nothing() {
        let streams = streams(2, 2);
        let account_id = AccountId::generate();
        let first = streams.open(account_id, DeviceId::generate()).unwrap();
        let second = streams.open(account_id, DeviceId::generate()).unwrap();
        assert!(streams.open(account_id, DeviceId::generate()).is_err());
        assert!(!first.link().is_replaced() && !second.link().is_replaced());
        let other_account = streams.open(AccountId::generate(), DeviceId::generate());
        assert!(other_account.is_ok());
    }

    #[tokio::test]
    async fn a_stop_aborts_the_tasks_that_do_not_end() {
        let streams = streams(2, 2);
        let registration = streams
            .open(AccountId::generate(), DeviceId::generate())
            .unwrap();
        let link = registration.link().clone();
        let task = tokio::spawn(async move {
            let _registration = registration;
            std::future::pending::<()>().await;
        });
        link.served_by(task.abort_handle());

        streams.stop(Duration::from_millis(50)).await;
        assert!(link.is_stopping());
        assert_eq!(streams.connections(), 0);
        assert!(task.await.unwrap_err().is_cancelled());
        let after_the_stop = streams.open(AccountId::generate(), DeviceId::generate());
        assert!(after_the_stop.is_err());
    }
}
