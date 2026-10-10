//! Calls: the server's own record of who is allowed in each room of the media server, and
//! the check that makes the rooms agree with it (ADR 0501 to ADR 0514).

pub mod admission;
pub mod entry;
pub mod speaking;
pub mod store;

use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap};
use std::ops::Bound;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use junto_protocol::error::ErrorCode;
use junto_protocol::ids::ServerId;
use time::OffsetDateTime;
use tokio::sync::Notify;

use crate::config::Limits;
use crate::error::{ApiError, ApiResult};
use crate::media::{self, MediaServer, RoomName};
use crate::rate_limit::RateLimiter;
use crate::state::AppState;

/// The marked rooms that the server remembers at most. With more, the marks are dropped:
/// the full check reaches every room in its turn.
const MOST_MARKED: usize = 4096;

pub struct Calls {
    media: Option<Arc<dyn MediaServer>>,
    namespace: String,
    reachable: AtomicBool,
    failing_after: u32,
    wake: Notify,
    work: Mutex<Work>,
    /// The requests of a device that change a call, its reports about its own state and its
    /// reports about who speaks are limited per session, by the hash of its token.
    pub actions: RateLimiter<[u8; 32]>,
    pub state_reports: RateLimiter<[u8; 32]>,
    pub speaking_reports: RateLimiter<[u8; 32]>,
    /// The notifications of the media server that start a check, all together.
    pub notifications: RateLimiter<()>,
    seen_notifications: Mutex<SeenNotifications>,
    pub(crate) speaking: Mutex<speaking::Marks>,
}

/// What the admission check has to do and what it found.
#[derive(Default)]
struct Work {
    /// The rooms whose calls changed since their last check.
    marked: BTreeSet<String>,
    /// A notification that names no room asked for a part of the full check.
    everything: bool,
    /// The rooms whose last checks failed, each with the number of failures in a row.
    failing: HashMap<String, u32>,
    /// The number of the time in which the media server answers. It grows with every
    /// outage, and starts anew with the server.
    era: u64,
    /// The rooms that were observed, each with the era of its last observation.
    observed: HashMap<String, u64>,
    /// The last room with a call, and the last room without one, that the full check took.
    after_recorded: Option<String>,
    after_ended: Option<String>,
    /// The checks that run now.
    checking: u32,
}

/// The notifications of the media server that were processed and are still valid.
#[derive(Default)]
struct SeenNotifications {
    until: HashMap<[u8; 32], OffsetDateTime>,
}

/// What the server did with a notification before.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Notified {
    First,
    /// The same notification was processed before.
    Repeated,
    /// The server remembers as many notifications as it may.
    Full,
}

/// The rooms that the work of one request, or of one step of a task, states as changed.
/// They are marked when the work has ended, which is after its commit: a check never looks
/// at a room for a change that is not yet in the database.
struct Changed {
    app: AppState,
    rooms: Vec<String>,
    everything: bool,
}

impl Drop for Changed {
    fn drop(&mut self) {
        let calls = &self.app.calls;
        {
            let mut work = calls.work();
            if work.marked.len() + self.rooms.len() > MOST_MARKED {
                work.marked.clear();
            } else {
                work.marked.extend(self.rooms.drain(..));
            }
            work.everything |= self.everything;
        }
        calls.poke();
    }
}

tokio::task_local! {
    static CHANGED: RefCell<Changed>;
}

/// Runs `work`, and afterwards marks the rooms that it stated as changed and wakes the
/// check of the changed rooms. This also happens when `work` is dropped before its end.
pub async fn following<T>(app: &AppState, work: impl Future<Output = T>) -> T {
    let changed = Changed {
        app: app.clone(),
        rooms: Vec::new(),
        everything: false,
    };
    CHANGED.scope(RefCell::new(changed), work).await
}

/// States that the call of a room changed, for the work that runs inside [`following`].
/// Returns false outside of it: the full check then finds the change at the turn of the
/// room.
pub fn changed(room: &str) -> bool {
    CHANGED
        .try_with(|changed| changed.borrow_mut().rooms.push(room.to_owned()))
        .is_ok()
}

/// One check that runs. The server is not settled while one exists.
pub struct Round<'a>(&'a Calls);

impl Drop for Round<'_> {
    fn drop(&mut self) {
        let mut work = self.0.work();
        work.checking = work.checking.saturating_sub(1);
    }
}

/// The next `most` names after `after` in the order of the names, continued from the first
/// name when the end is reached. Each name is taken once. `after` becomes the last one taken.
fn turn(names: &BTreeSet<String>, after: &mut Option<String>, most: usize) -> Vec<String> {
    let mut taken: Vec<String> = match after.as_ref() {
        Some(after) => names
            .range::<String, _>((Bound::Excluded(after), Bound::Unbounded))
            .take(most)
            .cloned()
            .collect(),
        None => names.iter().take(most).cloned().collect(),
    };
    if let Some(before) = after.as_ref()
        && taken.len() < most
    {
        let rest = most - taken.len();
        taken.extend(names.range::<String, _>(..=before).take(rest).cloned());
    }
    *after = taken.last().cloned();
    taken
}

impl Calls {
    pub fn new(media: Option<Arc<dyn MediaServer>>, server_id: ServerId, limits: &Limits) -> Self {
        let entries = limits.limiter_entries;
        Self {
            media,
            namespace: media::namespace(server_id),
            reachable: AtomicBool::new(false),
            failing_after: limits.calls.failing_after.max(1),
            wake: Notify::new(),
            work: Mutex::new(Work {
                era: 1,
                ..Work::default()
            }),
            actions: RateLimiter::with_capacity(limits.calls.actions, entries),
            state_reports: RateLimiter::with_capacity(limits.calls.state_reports, entries),
            speaking_reports: RateLimiter::with_capacity(limits.calls.speaking_reports, entries),
            notifications: RateLimiter::with_capacity(limits.calls.notifications, 1),
            seen_notifications: Mutex::new(SeenNotifications::default()),
            speaking: Mutex::new(speaking::Marks::default()),
        }
    }

    fn work(&self) -> MutexGuard<'_, Work> {
        self.work.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn media(&self) -> Option<&Arc<dyn MediaServer>> {
        self.media.as_ref()
    }

    /// The part of every room name of this server.
    pub fn namespace(&self) -> &str {
        &self.namespace
    }

    /// Whether the last full check reached the media server. `None` on a server without a
    /// media server.
    pub fn reachable(&self) -> Option<bool> {
        self.media
            .as_ref()
            .map(|_| self.reachable.load(Ordering::SeqCst))
    }

    /// Records whether the media server answers and returns what was recorded before. An
    /// outage starts a new era: what the server saw of a room before does not continue.
    pub(crate) fn set_reachable(&self, reachable: bool) -> bool {
        let before = self.reachable.swap(reachable, Ordering::SeqCst);
        if before && !reachable {
            self.work().era += 1;
        }
        before
    }

    fn unavailable() -> ApiError {
        ApiError::new(
            ErrorCode::MediaUnavailable,
            "the server does not reach its media server; no call can be joined now",
        )
        .retry_after(time::Duration::seconds(2))
    }

    /// The media server, for a request that lets a person into a room. While the admission
    /// check does not reach the media server, nobody is let in: the server could not remove
    /// a person who may not stay (ADR 0509).
    pub fn admitting(&self) -> ApiResult<&Arc<dyn MediaServer>> {
        match &self.media {
            Some(media) if self.reachable.load(Ordering::SeqCst) => Ok(media),
            _ => Err(Self::unavailable()),
        }
    }

    /// Refuses the entry into a room whose last check failed, for the same reason.
    pub fn admitting_into(&self, room: &RoomName) -> ApiResult<()> {
        if self.is_failing(room.as_str()) {
            Err(Self::unavailable())
        } else {
            Ok(())
        }
    }

    /// States that the call of a room changed. Inside [`following`] the room is marked when
    /// the work has ended; outside it, at once, and the caller wakes the check with
    /// [`Calls::poke`] after its commit.
    pub fn mark(&self, room: &str) {
        if !changed(room) {
            self.work().marked.insert(room.to_owned());
        }
    }

    /// Asks for the next part of the full check now, for a notification that names no room.
    pub fn mark_everything(&self) {
        let scoped = CHANGED.try_with(|changed| changed.borrow_mut().everything = true);
        if scoped.is_err() {
            self.work().everything = true;
        }
    }

    /// Wakes the check of the changed rooms. Without a marked room it sends no request to
    /// the media server.
    pub fn poke(&self) {
        let work = self.work();
        if work.everything || !work.marked.is_empty() {
            self.wake.notify_one();
        }
    }

    pub(crate) async fn poked(&self) {
        self.wake.notified().await;
    }

    /// What the woken task of the check does after a turn of the kind `last`: the part of
    /// the full check that a notification asked for, or a run of the targeted check. Marked
    /// rooms that waited during a part are checked before the next part.
    pub(crate) fn asked_for(&self, last: admission::Turn) -> admission::Turn {
        let work = self.work();
        let rooms_first = last == admission::Turn::Part && !work.marked.is_empty();
        if work.everything && !rooms_first {
            admission::Turn::Part
        } else {
            admission::Turn::Marked
        }
    }

    /// Takes at most `most` marked rooms.
    pub(crate) fn take_marked(&self, most: usize) -> Vec<String> {
        let mut work = self.work();
        let taken: Vec<String> = work.marked.iter().take(most).cloned().collect();
        for room in &taken {
            work.marked.remove(room);
        }
        taken
    }

    /// A check of `rooms` starts now: their marks are met by it.
    pub(crate) fn unmark(&self, rooms: &[String]) {
        let mut work = self.work();
        for room in rooms {
            work.marked.remove(room);
        }
    }

    pub(crate) fn round(&self) -> Round<'_> {
        self.work().checking += 1;
        Round(self)
    }

    /// A part of the full check starts: it meets the request of a notification for one.
    pub(crate) fn round_of_a_part(&self) -> Round<'_> {
        let mut work = self.work();
        work.checking += 1;
        work.everything = false;
        Round(self)
    }

    /// True when no room waits for a check and no check runs.
    pub fn settled(&self) -> bool {
        let work = self.work();
        work.checking == 0 && work.marked.is_empty() && !work.everything
    }

    /// The rooms with a call that the next part of the full check takes, in turn.
    pub(crate) fn next_recorded(&self, recorded: &BTreeSet<String>, most: usize) -> Vec<String> {
        turn(recorded, &mut self.work().after_recorded, most)
    }

    /// The rooms without a call that the next part of the full check takes, in turn.
    pub(crate) fn next_ended(&self, ended: &BTreeSet<String>, most: usize) -> Vec<String> {
        turn(ended, &mut self.work().after_ended, most)
    }

    pub(crate) fn era(&self) -> u64 {
        self.work().era
    }

    /// True when the room was observed before in this era, and no observation of it failed
    /// since. Only then does the time between two observations count as observed.
    pub(crate) fn continues(&self, room: &str, era: u64) -> bool {
        self.work().observed.get(room) == Some(&era)
    }

    /// The record of the room agrees with what the media server listed in `era`.
    pub(crate) fn observed(&self, room: &str, era: u64) {
        self.work().observed.insert(room.to_owned(), era);
    }

    /// An observation of the room failed: what the server saw of it before does not continue.
    pub(crate) fn unobserved(&self, room: &str) {
        self.work().observed.remove(room);
    }

    /// Counts one more failed check of a room and returns the number of failures in a row.
    /// A check that could not observe the room ends what the server saw of it before.
    pub(crate) fn room_failed(&self, room: &str, observed: bool) -> u32 {
        let mut work = self.work();
        if !observed {
            work.observed.remove(room);
        }
        let times = work.failing.entry(room.to_owned()).or_insert(0);
        *times = times.saturating_add(1);
        *times
    }

    /// States that a check of a room succeeded. Returns true when the room had failed before.
    pub(crate) fn room_served(&self, room: &str) -> bool {
        self.work().failing.remove(room).is_some()
    }

    pub fn is_failing(&self, room: &str) -> bool {
        self.work().failing.contains_key(room)
    }

    /// The number of rooms that keep failing: their check failed several times in a row.
    pub fn failing_rooms(&self) -> u32 {
        let work = self.work();
        let failing = work
            .failing
            .values()
            .filter(|times| **times >= self.failing_after)
            .count();
        u32::try_from(failing).unwrap_or(u32::MAX)
    }

    pub(crate) fn forget_rooms(&self, exists: impl Fn(&str) -> bool) {
        let mut work = self.work();
        work.failing.retain(|room, _| exists(room));
        work.observed.retain(|room, _| exists(room));
    }

    /// Remembers a verified notification until its validity ends, so that the same one is
    /// processed once. At most `most` are remembered.
    pub fn notified(
        &self,
        key: [u8; 32],
        valid_until: OffsetDateTime,
        now: OffsetDateTime,
        most: usize,
    ) -> Notified {
        let mut seen = self
            .seen_notifications
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        seen.until.retain(|_, until| *until > now);
        if seen.until.contains_key(&key) {
            return Notified::Repeated;
        }
        if seen.until.len() >= most.max(1) {
            return Notified::Full;
        }
        seen.until.insert(key, valid_until);
        Notified::First
    }

    /// Forgets a notification that was not processed, so that the media server can send it
    /// again.
    pub fn not_notified(&self, key: [u8; 32]) {
        self.seen_notifications
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .until
            .remove(&key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(names: &[&str]) -> BTreeSet<String> {
        names.iter().map(|name| (*name).to_owned()).collect()
    }

    #[test]
    fn every_name_gets_its_turn_also_while_names_come_and_go() {
        let mut after = None;
        let five = names(&["a", "b", "c", "d", "e"]);
        assert_eq!(turn(&five, &mut after, 2), ["a", "b"]);
        assert_eq!(turn(&five, &mut after, 2), ["c", "d"]);
        assert_eq!(turn(&five, &mut after, 2), ["e", "a"]);
        assert_eq!(turn(&five, &mut after, 2), ["b", "c"]);

        let changed = names(&["a", "b", "bb", "e"]);
        assert_eq!(turn(&changed, &mut after, 2), ["e", "a"]);
        assert_eq!(turn(&changed, &mut after, 9), ["b", "bb", "e", "a"]);
        assert_eq!(after.as_deref(), Some("a"));

        assert!(turn(&names(&[]), &mut after, 3).is_empty());
        assert_eq!(after, None);
        assert_eq!(turn(&names(&["z"]), &mut after, 3), ["z"]);
        assert_eq!(turn(&names(&["z"]), &mut after, 3), ["z"]);
    }
}
