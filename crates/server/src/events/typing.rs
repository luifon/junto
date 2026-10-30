//! Bounded activity that never enters the event log or a snapshot (ADR 0401).
use super::delivery::Opened;
use crate::{
    access::Access,
    error::{ApiError, ApiResult},
    messages::{Place, Purpose},
    read,
    state::AppState,
};
use junto_protocol::{
    ids::{AccountId, DeviceId},
    typing::{MOST_TYPING_PLACES, TYPING_LIFETIME_MS, Typing, TypingPlace},
};
use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};
use tokio::sync::broadcast;
use uuid::Uuid;

const CAPACITY: usize = 8192;
const LIFETIME: Duration = Duration::from_millis(TYPING_LIFETIME_MS as u64);
#[derive(Clone, Copy)]
pub struct Source {
    pub account: AccountId,
    pub device: DeviceId,
    pub session: Uuid,
    pub connection: u64,
}
#[derive(Clone)]
pub struct Signal {
    source: Source,
    pub place: TypingPlace,
    name: String,
    active: bool,
    until: Instant,
    serial: u64,
}
#[derive(Default)]
struct Registry {
    next: u64,
    entries: HashMap<(AccountId, DeviceId, TypingPlace), Signal>,
    latest: HashMap<(AccountId, TypingPlace), (u64, Instant)>,
}
pub struct Activity {
    registry: Mutex<Registry>,
    send: broadcast::Sender<Signal>,
    rate: crate::rate_limit::RateLimiter<AccountId>,
}
impl Default for Activity {
    fn default() -> Self {
        Self {
            registry: Mutex::new(Registry::default()),
            send: broadcast::Sender::new(256),
            rate: crate::rate_limit::RateLimiter::with_capacity(
                crate::config::RateLimit {
                    attempts: 8,
                    window: time::Duration::seconds(1),
                },
                CAPACITY,
            ),
        }
    }
}
impl Activity {
    pub fn subscribe(&self) -> broadcast::Receiver<Signal> {
        self.send.subscribe()
    }
    pub fn allowed(&self, account: AccountId, now: time::OffsetDateTime) -> bool {
        self.rate.attempt_of(account, now).is_ok()
    }
    pub fn current(&self, signal: &Signal) -> bool {
        self.registry
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .latest
            .get(&(signal.source.account, signal.place))
            .is_some_and(|(serial, until)| *serial == signal.serial && *until > Instant::now())
    }
    pub fn update(
        &self,
        source: Source,
        place: TypingPlace,
        name: String,
        active: bool,
        lifetime_ms: u32,
        now: Instant,
    ) {
        let mut registry = self.registry.lock().unwrap_or_else(PoisonError::into_inner);
        registry.entries.retain(|_, entry| entry.until > now);
        registry.latest.retain(|_, (_, until)| *until > now);
        let key = (source.account, source.device, place);
        if active {
            if !registry.entries.contains_key(&key)
                && (registry.entries.len() >= CAPACITY
                    || registry
                        .entries
                        .values()
                        .filter(|entry| entry.source.connection == source.connection)
                        .count()
                        >= MOST_TYPING_PLACES)
            {
                return;
            }
            registry.entries.insert(
                key,
                Signal {
                    source,
                    place,
                    name,
                    active: true,
                    until: now
                        + Duration::from_millis(u64::from(lifetime_ms.min(TYPING_LIFETIME_MS))),
                    serial: 0,
                },
            );
        } else if registry
            .entries
            .get(&key)
            .is_some_and(|entry| entry.source.connection == source.connection)
        {
            registry.entries.remove(&key);
        } else {
            return;
        }
        self.tell(&mut registry, source, place, now);
    }
    fn tell(&self, registry: &mut Registry, source: Source, place: TypingPlace, now: Instant) {
        let mut signal = registry
            .entries
            .values()
            .filter(|entry| entry.source.account == source.account && entry.place == place)
            .max_by_key(|entry| entry.until)
            .cloned()
            .unwrap_or(Signal {
                source,
                place,
                name: String::new(),
                active: false,
                until: now,
                serial: 0,
            });
        registry.next = registry.next.wrapping_add(1);
        signal.serial = registry.next;
        if registry.latest.len() >= CAPACITY
            && !registry.latest.contains_key(&(source.account, place))
            && let Some(oldest) = registry
                .latest
                .iter()
                .min_by_key(|(_, (_, until))| *until)
                .map(|(key, _)| *key)
        {
            registry.latest.remove(&oldest);
        }
        registry
            .latest
            .insert((source.account, place), (signal.serial, now + LIFETIME));
        let _ = self.send.send(signal);
    }
    pub fn close(&self, connection: u64) {
        let now = Instant::now();
        let mut registry = self.registry.lock().unwrap_or_else(PoisonError::into_inner);
        let removed: Vec<_> = registry
            .entries
            .values()
            .filter(|entry| entry.source.connection == connection)
            .map(|entry| (entry.source, entry.place))
            .collect();
        registry
            .entries
            .retain(|_, entry| entry.source.connection != connection && entry.until > now);
        let mut told = HashSet::new();
        for (source, place) in removed {
            if told.insert((source.account, place)) {
                self.tell(&mut registry, source, place, now);
            }
        }
    }
}
async fn place(
    connection: &mut sqlx::PgConnection,
    access: &Access,
    target: TypingPlace,
    purpose: Purpose,
) -> ApiResult<Place> {
    match target {
        TypingPlace::Channel { channel_id } => Place::channel(connection, access, channel_id).await,
        TypingPlace::Direct { conversation_id } => {
            Place::direct(connection, access, conversation_id, purpose).await
        }
        TypingPlace::Thread {
            channel_id,
            root_id,
        } => {
            let (place, _) =
                crate::messages::threads::root(connection, access, root_id, purpose).await?;
            if !matches!(place.value, junto_protocol::message::MessagePlace::Channel { channel_id: actual, .. } if actual == channel_id)
            {
                return Err(ApiError::not_found());
            }
            Ok(place)
        }
    }
}
pub async fn changed(
    app: &AppState,
    who: Opened,
    connection_id: u64,
    target: TypingPlace,
    lifetime_ms: u32,
) -> ApiResult<()> {
    let active = lifetime_ms > 0;
    if lifetime_ms > TYPING_LIFETIME_MS {
        return Ok(());
    }
    if !app.streams.typing().allowed(who.account_id, app.now()) {
        return Ok(());
    }
    let source = Source {
        account: who.account_id,
        device: who.device_id,
        session: who.session_id,
        connection: connection_id,
    };
    if !active {
        app.streams
            .typing_changed(source, target, String::new(), false, 0);
        return Ok(());
    }
    let name = read!(app, |connection| {
        let Some(access) = Access::of_session(
            connection,
            who.account_id,
            who.device_id,
            who.session_id,
            app.now(),
        )
        .await?
        else {
            return Ok(None);
        };
        let Ok(place) = place(connection, &access, target, Purpose::Change).await else {
            return Ok(None);
        };
        if place.require_open().is_err()
            || place
                .require_send(&access, matches!(target, TypingPlace::Thread { .. }))
                .is_err()
        {
            return Ok(None);
        }
        if let TypingPlace::Thread { root_id, .. } = target {
            let exists: bool = sqlx::query_scalar("select exists(select 1 from messages where thread_root=$1) or exists(select 1 from thread_reads where root_id=$1 and writer)").bind(root_id).fetch_one(&mut *connection).await?;
            if !exists
                && place
                    .require(&access, junto_protocol::WorkspacePermission::CreateThreads)
                    .is_err()
            {
                return Ok(None);
            }
        }
        let name: String = sqlx::query_scalar("select name from accounts where id=$1")
            .bind(who.account_id)
            .fetch_one(connection)
            .await?;
        Ok(Some(name))
    })?;
    if let Some(name) = name {
        app.streams
            .typing_changed(source, target, name, true, lifetime_ms);
    }
    Ok(())
}
pub async fn delivered(app: &AppState, who: Opened, signal: Signal) -> ApiResult<Option<Typing>> {
    if who.account_id == signal.source.account || !app.streams.typing().current(&signal) {
        return Ok(None);
    }
    let allowed = read!(app, |connection| {
        let Some(access) = Access::of_session(
            connection,
            who.account_id,
            who.device_id,
            who.session_id,
            app.now(),
        )
        .await?
        else {
            return Ok(false);
        };
        let since = app
            .streams
            .live_since(who.account_id, who.device_id, who.session_id);
        if place(connection, &access, signal.place, Purpose::Live(since))
            .await
            .is_err()
        {
            return Ok(false);
        }
        if signal.active {
            let Some(source) = Access::of_session(
                connection,
                signal.source.account,
                signal.source.device,
                signal.source.session,
                app.now(),
            )
            .await?
            else {
                return Ok(false);
            };
            let Ok(place) = place(connection, &source, signal.place, Purpose::Change).await else {
                return Ok(false);
            };
            if place.require_open().is_err()
                || place
                    .require_send(&source, matches!(signal.place, TypingPlace::Thread { .. }))
                    .is_err()
            {
                return Ok(false);
            }
        }
        Ok(true)
    })?;
    let expires = signal
        .until
        .saturating_duration_since(Instant::now())
        .as_millis()
        .min(u128::from(TYPING_LIFETIME_MS)) as u32;
    Ok(
        (allowed && app.streams.typing().current(&signal) && (!signal.active || expires > 0))
            .then_some(Typing {
                place: signal.place,
                account_id: signal.source.account,
                name: signal.name,
                active: signal.active,
                expires_in_ms: expires,
            }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    fn source(connection: u64) -> Source {
        Source {
            account: AccountId::generate(),
            device: DeviceId::generate(),
            session: Uuid::now_v7(),
            connection,
        }
    }
    fn place() -> TypingPlace {
        TypingPlace::Channel {
            channel_id: junto_protocol::ids::ChannelId::generate(),
        }
    }
    #[test]
    fn devices_aggregate_and_closing_an_old_connection_cannot_restart_activity() {
        let activity = Activity::default();
        let mut receiver = activity.subscribe();
        let first = source(1);
        let second = Source {
            device: DeviceId::generate(),
            connection: 2,
            ..first
        };
        let place = place();
        let now = Instant::now();
        activity.update(first, place, "Ada".to_owned(), true, 8000, now);
        assert!(receiver.try_recv().unwrap().active);
        activity.update(second, place, "Ada".to_owned(), true, 8000, now);
        receiver.try_recv().unwrap();
        activity.close(1);
        assert!(receiver.try_recv().unwrap().active);
        activity.close(2);
        assert!(!receiver.try_recv().unwrap().active);
        let streams = std::sync::Arc::new(super::super::hub::Streams::new(
            &crate::config::StreamLimits::default(),
        ));
        let registration = streams
            .open(
                first.account,
                first.device,
                first.session,
                time::OffsetDateTime::now_utc(),
            )
            .unwrap();
        registration
            .link()
            .set_level(Some(junto_protocol::events::Level::Full));
        let from = Source {
            connection: registration.link().id(),
            ..first
        };
        let mut events = streams.typing().subscribe();
        streams.typing_changed(from, place, "Ada".to_owned(), true, 8000);
        assert!(events.try_recv().unwrap().active);
        drop(registration);
        assert!(!events.try_recv().unwrap().active);
        streams.typing_changed(from, place, "Ada".to_owned(), true, 8000);
        assert!(events.try_recv().is_err());
    }
    #[test]
    fn expired_activity_has_no_replay_and_sources_have_rate_and_place_bounds() {
        let activity = Activity::default();
        let from = source(1);
        let mut receiver = activity.subscribe();
        activity.update(
            from,
            place(),
            "Ada".to_owned(),
            true,
            8000,
            Instant::now() - Duration::from_secs(9),
        );
        assert!(!activity.current(&receiver.try_recv().unwrap()));
        assert!(activity.subscribe().try_recv().is_err());
        for _ in 0..MOST_TYPING_PLACES {
            activity.update(from, place(), "Ada".to_owned(), true, 8000, Instant::now());
            receiver.try_recv().unwrap();
        }
        activity.update(from, place(), "Ada".to_owned(), true, 8000, Instant::now());
        assert!(receiver.try_recv().is_err());
        let now = time::OffsetDateTime::now_utc();
        for _ in 0..8 {
            assert!(activity.allowed(from.account, now));
        }
        assert!(!activity.allowed(from.account, now));
        assert!(activity.allowed(from.account, now + time::Duration::seconds(1)));
    }
}
