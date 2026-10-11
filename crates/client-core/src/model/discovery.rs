//! What the state of a server holds of threads, pins and notices (ADR 0722): the threads
//! with something unread, the pin lists that a reader has open, the notices of the account
//! with its notification settings, and the messages that those lists name.

use std::sync::Arc;

use junto_protocol::discovery::{DailyQuiet, Notification, NotificationPage, NotificationSettings};
use junto_protocol::ids::{ChannelId, MessageId};
use junto_protocol::message::{Message, ThreadState, UnreadThreads};
use time::OffsetDateTime;
use uuid::Uuid;

use super::{Loadable, PlaceId, ReadFailure, ServerModel};
use crate::api::SettingsScope;

/// How many unread threads and how many notices the state holds at most. A page that
/// passes the bound is cut, so a server cannot make the state grow without a bound
/// (ADR 0714). The server keeps 500 notices of an account (ADR 0409).
pub const MOST_THREADS: usize = 2_000;
pub const MOST_NOTICES: usize = 500;
/// How many pinned messages of one channel the state holds at most. The server allows 100.
pub const MOST_PINS: usize = 1_000;
/// How many fork channels of one thread the state holds at most. The server allows one.
const MOST_FORKS: usize = 16;

/// A message that a list of the state names by its identifier: the root of an unread
/// thread, or the message of a notice. The lists carry no place and no text (ADR 0409).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Named {
    Read(Arc<Message>),
    /// The server has no such message for the account.
    Gone,
    /// The read failed. It is tried again after a reload of the state, and when an event
    /// names the message.
    Failed,
}

/// The pinned messages of one channel that a reader has open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PinList {
    pub channel_id: ChannelId,
    /// In the order in which the messages were pinned.
    pub messages: Loadable<Vec<Arc<Message>>>,
    /// Why the last read of the list failed.
    pub failed: Option<ReadFailure>,
}

/// What a `thread_read` did to the list of unread threads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ThreadMoved {
    /// The thread is read: it left the list.
    Read,
    /// The list holds the thread and something of it is unread still, or the list does
    /// not hold the thread. The list is read again.
    Unsettled,
}

/// True at `now` inside a daily quiet time. The two minutes are minutes of a day at the
/// stated distance from UTC; a time that starts later than it ends passes midnight.
pub fn in_daily_quiet(quiet: &DailyQuiet, now: OffsetDateTime) -> bool {
    let local = now + time::Duration::minutes(i64::from(quiet.utc_offset_minutes));
    let minute = u16::from(local.hour()) * 60 + u16::from(local.minute());
    if quiet.start_minute < quiet.end_minute {
        (quiet.start_minute..quiet.end_minute).contains(&minute)
    } else {
        minute >= quiet.start_minute || minute < quiet.end_minute
    }
}

impl ServerModel {
    // Unread threads.

    /// What the account has not read in a thread. Absent for a thread that is read, for
    /// one that the account does not follow, and for one beyond the loaded pages.
    pub fn thread_state(&self, root_id: MessageId) -> Option<&ThreadState> {
        self.threads.iter().find(|held| held.root_id == root_id)
    }

    /// The continuation of the list of unread threads, while the server has more.
    pub(crate) fn threads_continuation(&self) -> Option<&str> {
        self.threads_next.as_deref().filter(|_| self.threads_more)
    }

    /// Replaces the list of unread threads by its first page.
    pub(crate) fn set_threads(&mut self, page: UnreadThreads) {
        self.threads = page.threads;
        self.threads.truncate(MOST_THREADS);
        self.threads_more = page.more;
        self.threads_next = page.next;
        self.prune_named();
    }

    /// Adds the page after the loaded ones.
    pub(crate) fn add_threads(&mut self, page: UnreadThreads) {
        for state in page.threads {
            if self.thread_state(state.root_id).is_none() && self.threads.len() < MOST_THREADS {
                self.threads.push(state);
            }
        }
        self.threads_more = page.more && self.threads.len() < MOST_THREADS;
        self.threads_next = page.next;
    }

    /// Puts the answer to a read position of a thread into the list. A thread with
    /// nothing unread leaves the list. A thread that the list does not hold is not added:
    /// its place in the order of the server is not known.
    pub(crate) fn set_thread_state(&mut self, state: ThreadState) -> bool {
        let Some(index) = self
            .threads
            .iter()
            .position(|held| held.root_id == state.root_id)
        else {
            return false;
        };
        if state.unread == 0 && state.mentions == 0 {
            self.threads.remove(index);
            self.prune_named();
        } else if self.threads[index] == state {
            return false;
        } else {
            self.threads[index] = state;
        }
        true
    }

    /// Moves the read position of a thread forward, as `thread_read` tells it.
    pub(crate) fn move_thread_read(&mut self, root_id: MessageId, position: u64) -> ThreadMoved {
        let Some(index) = self.threads.iter().position(|held| held.root_id == root_id) else {
            return ThreadMoved::Unsettled;
        };
        let held = &mut self.threads[index];
        held.last_read = held.last_read.max(position);
        if held.last_read >= held.latest {
            self.threads.remove(index);
            self.prune_named();
            ThreadMoved::Read
        } else {
            ThreadMoved::Unsettled
        }
    }

    /// The highest position of a place that the client knows: of its summary, of its
    /// events, and for a thread of its loaded replies and of its read state.
    pub(crate) fn latest_known(&self, place: PlaceId) -> u64 {
        match place {
            PlaceId::Thread { root_id, .. } => {
                let loaded = self
                    .timeline(place)
                    .and_then(|timeline| timeline.messages.last())
                    .map_or(0, |last| last.position);
                let listed = self.thread_state(root_id).map_or(0, |state| state.latest);
                loaded.max(listed)
            }
            PlaceId::Channel(_) | PlaceId::Direct(_) => {
                let summary = self.message_state(place).map_or(0, |state| state.latest);
                let seen = self.seen_positions.get(&place).copied().unwrap_or(0);
                summary.max(seen)
            }
        }
    }

    /// Takes the position of an event of a place. True when the position is not above
    /// the highest one that the client knew: the event tells no new message. The notice
    /// of the root of a thread follows the notice of its reply (ADR 0409), and a root is
    /// older than its reply, so such an event can be the notice of a root.
    pub(crate) fn see_position(&mut self, place: PlaceId, position: u64) -> bool {
        let known = self.latest_known(place);
        if position > known {
            self.seen_positions.insert(place, position);
            false
        } else {
            true
        }
    }

    /// Puts the root and the fork channels that a page of a thread carried into the
    /// timeline of the thread. A root that is not the message that the thread was opened
    /// for, or that lies in another channel, is not taken: a server is not trusted to
    /// answer for the thread that was asked for (ADR 0714).
    pub(crate) fn set_thread_head(
        &mut self,
        place: PlaceId,
        root: Message,
        mut forks: Vec<ChannelId>,
    ) -> bool {
        if place.thread_root() != Some(root.id)
            || PlaceId::of(&root.place) != place.container()
            || root.thread_root.is_some()
        {
            return false;
        }
        forks.truncate(MOST_FORKS);
        let Some(timeline) = self.timeline_mut(place) else {
            return false;
        };
        timeline.root = Some(Arc::new(root));
        timeline.forks = forks;
        true
    }

    /// Puts the answer of a read of the fork channels of a thread into its timeline. The
    /// root that the timeline holds stays: it follows the events of its message, and this
    /// answer can be older than one of them.
    pub(crate) fn set_thread_forks(
        &mut self,
        place: PlaceId,
        root: Message,
        forks: Vec<ChannelId>,
    ) -> bool {
        if place.thread_root() != Some(root.id) {
            return false;
        }
        let held = self
            .timeline(place)
            .and_then(|timeline| timeline.root.clone());
        match held {
            Some(held) => self.set_thread_head(place, held.as_ref().clone(), forks),
            None => self.set_thread_head(place, root, forks),
        }
    }

    // Named messages.

    /// True when a list of the state names the message.
    pub(crate) fn names(&self, message_id: MessageId) -> bool {
        self.threads.iter().any(|held| held.root_id == message_id)
            || self
                .notices
                .iter()
                .any(|held| held.message_id == message_id)
    }

    /// The message that a list names, once it was read.
    pub fn named(&self, message_id: MessageId) -> Option<&Named> {
        self.named
            .iter()
            .find(|(held, _)| *held == message_id)
            .map(|(_, named)| named)
    }

    /// The messages that the lists name and that were not read yet, in the order of the
    /// lists.
    pub(crate) fn missing_named(&self) -> Vec<MessageId> {
        let mut missing = Vec::new();
        let roots = self.threads.iter().map(|held| held.root_id);
        let noticed = self.notices.iter().map(|held| held.message_id);
        for message_id in roots.chain(noticed) {
            if self.named(message_id).is_none() && !missing.contains(&message_id) {
                missing.push(message_id);
            }
        }
        missing
    }

    fn set_named(&mut self, message_id: MessageId, named: Named) -> bool {
        match self.named.iter_mut().find(|(held, _)| *held == message_id) {
            Some((_, held)) if *held == named => false,
            Some((_, held)) => {
                *held = named;
                true
            }
            None => {
                self.named.push((message_id, named));
                true
            }
        }
    }

    pub(super) fn put_named(&mut self, message: &Arc<Message>) -> bool {
        self.names(message.id) && self.set_named(message.id, Named::Read(message.clone()))
    }

    /// The server has no such message for the account.
    pub(super) fn lose_named(&mut self, message_id: MessageId) -> bool {
        self.names(message_id) && self.set_named(message_id, Named::Gone)
    }

    /// A read of a named message ended without the message: the server has none, or the
    /// read failed for good.
    pub(crate) fn settle_named(&mut self, message_id: MessageId, gone: bool) -> bool {
        let named = if gone { Named::Gone } else { Named::Failed };
        self.names(message_id) && self.set_named(message_id, named)
    }

    /// Drops the messages that no list names any more.
    fn prune_named(&mut self) {
        let named = std::mem::take(&mut self.named);
        self.named = named
            .into_iter()
            .filter(|(message_id, _)| self.names(*message_id))
            .collect();
    }

    /// A channel or a conversation is gone: the entries of the lists whose message is
    /// known to lie in it leave. The lists are read again for the entries whose place the
    /// client does not know.
    pub(super) fn forget_named_of(&mut self, place: PlaceId) {
        let inside: Vec<MessageId> = self
            .named
            .iter()
            .filter_map(|(message_id, named)| match named {
                Named::Read(message) if PlaceId::of(&message.place) == place => Some(*message_id),
                _ => None,
            })
            .collect();
        self.threads.retain(|held| !inside.contains(&held.root_id));
        self.notices
            .retain(|held| !inside.contains(&held.message_id));
        self.prune_named();
    }

    // Pins.

    /// The pinned messages of a channel, while a reader has them open.
    pub fn pin_list(&self, channel_id: ChannelId) -> Option<&PinList> {
        self.pins.iter().find(|held| held.channel_id == channel_id)
    }

    fn pin_list_mut(&mut self, channel_id: ChannelId) -> Option<&mut PinList> {
        self.pins
            .iter_mut()
            .find(|held| held.channel_id == channel_id)
    }

    /// Starts to hold the pin list of a channel for a reader.
    pub(crate) fn open_pins(&mut self, channel_id: ChannelId) {
        if self.pin_list(channel_id).is_none() {
            self.pins.push(PinList {
                channel_id,
                messages: Loadable::NotLoaded,
                failed: None,
            });
        }
    }

    pub(crate) fn close_pins(&mut self, channel_id: ChannelId) {
        self.pins.retain(|held| held.channel_id != channel_id);
    }

    /// Marks the pin list of a channel as on its way, or as failed.
    pub(crate) fn set_pins_reading(&mut self, channel_id: ChannelId, failed: Option<ReadFailure>) {
        if let Some(list) = self.pin_list_mut(channel_id) {
            if list.messages.loaded().is_none() {
                list.messages = if failed.is_some() {
                    Loadable::NotLoaded
                } else {
                    Loadable::Loading
                };
            }
            list.failed = failed;
        }
    }

    /// Puts the answer of the pin route into the state.
    /// A message of another place is not taken (ADR 0714).
    pub(crate) fn set_pins(&mut self, channel_id: ChannelId, mut messages: Vec<Message>) {
        messages.retain(|message| PlaceId::of(&message.place) == PlaceId::Channel(channel_id));
        messages.truncate(MOST_PINS);
        if let Some(list) = self.pin_list_mut(channel_id) {
            list.messages = Loadable::Loaded(messages.into_iter().map(Arc::new).collect());
            list.failed = None;
        }
    }

    /// Keeps an open pin list current from one message that the server returned: a pinned
    /// message is replaced, or added at the end, where the server lists the newest pin
    /// (ADR 0409); a message without the pin leaves the list.
    pub(super) fn put_pin(&mut self, message: &Arc<Message>) -> bool {
        let PlaceId::Channel(channel_id) = PlaceId::of(&message.place) else {
            return false;
        };
        let Some(pins) = self
            .pin_list_mut(channel_id)
            .and_then(|list| list.messages.loaded_mut())
        else {
            return false;
        };
        let held = pins.iter().position(|pin| pin.id == message.id);
        let pinned = message.pinned && !message.deleted;
        match (held, pinned) {
            (Some(index), true) if pins[index] == *message => false,
            (Some(index), true) => {
                pins[index] = message.clone();
                true
            }
            (Some(index), false) => {
                pins.remove(index);
                true
            }
            (None, true) if pins.len() < MOST_PINS => {
                pins.push(message.clone());
                true
            }
            (None, _) => false,
        }
    }

    pub(super) fn remove_pin(&mut self, place: PlaceId, message_id: MessageId) -> bool {
        let PlaceId::Channel(channel_id) = place else {
            return false;
        };
        let Some(pins) = self
            .pin_list_mut(channel_id)
            .and_then(|list| list.messages.loaded_mut())
        else {
            return false;
        };
        let before = pins.len();
        pins.retain(|pin| pin.id != message_id);
        pins.len() != before
    }

    // Notices and notification settings.

    /// Replaces the notices by their first page.
    pub(crate) fn set_notices(&mut self, page: NotificationPage) {
        self.notices = page.notifications;
        self.notices.truncate(MOST_NOTICES);
        self.notices_more = page.has_more && self.notices.len() < MOST_NOTICES;
        self.prune_named();
    }

    /// Adds the page after the notice `before`, when that notice is still the last one.
    pub(crate) fn add_notices(&mut self, before: Uuid, page: NotificationPage) -> bool {
        if self.notices.last().map(|last| last.id) != Some(before) {
            return false;
        }
        for notice in page.notifications {
            if self.notices.len() < MOST_NOTICES
                && self.notices.iter().all(|held| held.id != notice.id)
            {
                self.notices.push(notice);
            }
        }
        self.notices_more = page.has_more && self.notices.len() < MOST_NOTICES;
        true
    }

    /// True when a read position can have marked a notice of the state as read. The
    /// server marks the notices of the messages up to a read position in the request that
    /// moves it, and sends no event of the notices for that (ADR 0409). `inside` tells
    /// whether a message lies in the place of the position. A notice whose message is not
    /// read yet can lie anywhere.
    pub(crate) fn read_covers_notice(
        &self,
        inside: impl Fn(&Message) -> bool,
        position: u64,
    ) -> bool {
        self.unread_notices()
            .any(|notice| match self.named(notice.message_id) {
                Some(Named::Read(message)) => inside(message) && message.position <= position,
                Some(Named::Gone) => false,
                // The place of the message is not known.
                Some(Named::Failed) | None => true,
            })
    }

    /// The notices that the person has not read.
    pub fn unread_notices(&self) -> impl Iterator<Item = &Notification> {
        self.notices.iter().filter(|held| held.read_at.is_none())
    }

    /// The settings that the account stored for a scope. Absent when it stored none: the
    /// scope inherits everything (ADR 0409).
    pub fn notification_settings(&self, scope: SettingsScope) -> Option<&NotificationSettings> {
        let id = scope.id();
        self.notification_settings
            .iter()
            .find(|held| held.scope_id == id)
            .map(|held| &held.settings)
    }

    /// True while the account wants no alert: "do not disturb" is on, or `now` lies in the
    /// daily quiet time. The server delivers no notice event during this time; a client
    /// applies the same rule to what it holds already (ADR 0409).
    pub fn is_quiet(&self, now: OffsetDateTime) -> bool {
        self.notification_settings(SettingsScope::Account)
            .is_some_and(|settings| {
                settings.do_not_disturb
                    || settings
                        .daily_quiet
                        .as_ref()
                        .is_some_and(|quiet| in_daily_quiet(quiet, now))
            })
    }

    /// The channels whose mute has a stated end and whose sign depends on it: the end
    /// has not passed at `now`, or the summary of the channel still says "muted". The
    /// clock of the server decides the end, and it can differ from the clock of `now`.
    pub(crate) fn mute_ends(&self, now: OffsetDateTime) -> Vec<(ChannelId, OffsetDateTime)> {
        self.notification_settings
            .iter()
            .filter(|held| held.settings.mute)
            .filter_map(|held| {
                let until = held.settings.muted_until?;
                let channel_id = ChannelId::from_uuid(held.scope_id);
                self.channel(channel_id)?;
                let muted = self
                    .message_state(PlaceId::Channel(channel_id))
                    .is_some_and(|state| state.muted);
                (until > now || muted).then_some((channel_id, until))
            })
            .collect()
    }

    /// The end that the settings state for the mute of a channel, while the summary of
    /// the channel says "muted".
    pub(crate) fn stated_mute_end(&self, channel_id: ChannelId) -> Option<OffsetDateTime> {
        let muted = self
            .message_state(PlaceId::Channel(channel_id))
            .is_some_and(|state| state.muted);
        self.notification_settings(SettingsScope::Channel(channel_id))
            .filter(|settings| settings.mute && muted)
            .and_then(|settings| settings.muted_until)
    }

    /// The channels whose summary shows mentions. A `thread_read` names no channel, and
    /// an unread mention of a thread counts in the summary of its channel (ADR 0409).
    pub(crate) fn places_with_mentions(&self) -> Vec<PlaceId> {
        self.message_states
            .iter()
            .filter(|held| held.mentions > 0 || held.notification_mentions > 0)
            .map(|held| PlaceId::of(&held.place))
            .filter(|place| matches!(place, PlaceId::Channel(_)))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(hour: u8, minute: u8) -> OffsetDateTime {
        OffsetDateTime::from_unix_timestamp(1_700_000_000)
            .unwrap()
            .replace_time(time::Time::from_hms(hour, minute, 0).unwrap())
    }

    #[test]
    fn a_daily_quiet_time_holds_between_its_minutes_in_its_zone() {
        let quiet = DailyQuiet {
            start_minute: 13 * 60,
            end_minute: 14 * 60,
            utc_offset_minutes: -180,
        };
        // 13:00 at three hours behind UTC is 16:00 UTC.
        assert!(!in_daily_quiet(&quiet, at(15, 59)));
        assert!(in_daily_quiet(&quiet, at(16, 0)));
        assert!(in_daily_quiet(&quiet, at(16, 59)));
        assert!(!in_daily_quiet(&quiet, at(17, 0)));
    }

    #[test]
    fn a_daily_quiet_time_can_pass_midnight() {
        let quiet = DailyQuiet {
            start_minute: 22 * 60,
            end_minute: 7 * 60,
            utc_offset_minutes: 0,
        };
        assert!(in_daily_quiet(&quiet, at(22, 0)));
        assert!(in_daily_quiet(&quiet, at(23, 59)));
        assert!(in_daily_quiet(&quiet, at(0, 0)));
        assert!(in_daily_quiet(&quiet, at(6, 59)));
        assert!(!in_daily_quiet(&quiet, at(7, 0)));
        assert!(!in_daily_quiet(&quiet, at(21, 59)));
    }
}
