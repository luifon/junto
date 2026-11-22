use std::{
    collections::{BTreeSet, HashMap},
    sync::Arc,
    time::{Duration, Instant},
};

use gpui::{Context, Task};
use junto_client_core::{
    Owner, ServerHandle,
    activity::{ActivityState, SystemActivity},
    flow::FlowError,
    live::StreamStatus,
    protocol::{
        ids::{AccountId, ServerId},
        presence::{
            PRESENCE_PAGE, Presence, PresenceChoice, ReadPresence, SetPresence, SetStatusText,
            StatusText,
        },
    },
};

use super::{AppState, people::Wanted, view::shown_model};
use crate::text;

const READS: usize = 4;

/// When a status text ends (ADR 0401).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StatusTextEnd {
    #[default]
    Never,
    HalfHour,
    Hour,
    FourHours,
    /// At the end of the day on the clock of the person.
    Today,
    /// At the end of Sunday on the clock of the person.
    ThisWeek,
    /// At a date and a time that the person types, on the clock of the person.
    Custom,
}

impl StatusTextEnd {
    /// The near ends, and the later ones with the choice of a date and a time.
    pub const NEAR: [Self; 4] = [Self::Never, Self::HalfHour, Self::Hour, Self::FourHours];
    pub const LATER: [Self; 3] = [Self::Today, Self::ThisWeek, Self::Custom];

    /// The end of a status text that is set at `now`. Absent: it stays until it is changed,
    /// and for `Custom`, whose end the person types.
    pub fn until(
        self,
        now: time::OffsetDateTime,
        zone: super::feed::Zone,
    ) -> Option<time::OffsetDateTime> {
        let midnight_after = |days: i64| {
            let local = zone.local(now);
            let midnight = local.replace_time(time::Time::MIDNIGHT) + time::Duration::days(days);
            crate::moment::in_utc(midnight)
        };
        match self {
            Self::Never | Self::Custom => None,
            Self::HalfHour => Some(now + time::Duration::minutes(30)),
            Self::Hour => Some(now + time::Duration::hours(1)),
            Self::FourHours => Some(now + time::Duration::hours(4)),
            Self::Today => Some(midnight_after(1)),
            Self::ThisWeek => {
                let weekday = zone.local(now).weekday().number_days_from_monday();
                Some(midnight_after(7 - i64::from(weekday)))
            }
        }
    }
}
const REFRESH: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerPresence {
    owner: Owner,
    pub choice: Option<PresenceChoice>,
    pub connected: bool,
    pub changing: bool,
    pub failed: Option<String>,
    pub read_failed: Option<String>,
    pub reading: bool,
    values: Arc<HashMap<AccountId, Presence>>,
    /// The status texts of the accounts that the server returned (ADR 0401).
    texts: Arc<HashMap<AccountId, StatusText>>,
    /// The server takes a status text: `capabilities.status_text`.
    pub takes_text: bool,
    /// A change of the own status text is on its way.
    pub text_changing: bool,
    /// Why the last change of the own status text failed.
    pub text_failed: Option<String>,
}

impl ServerPresence {
    fn empty(owner: Owner, takes_text: bool) -> Self {
        Self {
            owner,
            choice: None,
            connected: true,
            changing: false,
            failed: None,
            read_failed: None,
            reading: false,
            values: Arc::default(),
            texts: Arc::default(),
            takes_text,
            text_changing: false,
            text_failed: None,
        }
    }

    /// The account of this device on the server whose presence this is.
    pub fn owner(&self) -> Owner {
        self.owner
    }

    /// The status text of the account of this device on the server.
    pub fn own_text(&self) -> Option<&StatusText> {
        self.texts.get(&self.owner.account_id)
    }
}
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct KnownPresence {
    servers: HashMap<ServerId, ServerPresence>,
}
impl KnownPresence {
    pub fn get(&self, server: ServerId, account: AccountId) -> Presence {
        self.servers
            .get(&server)
            .and_then(|server| server.values.get(&account))
            .copied()
            .unwrap_or_default()
    }
    pub fn server(&self, server: ServerId) -> Option<&ServerPresence> {
        self.servers.get(&server)
    }
    pub fn text(&self, server: ServerId, account: AccountId) -> Option<&StatusText> {
        self.servers
            .get(&server)
            .and_then(|server| server.texts.get(&account))
    }
    pub(crate) fn texts(&self, server: ServerId) -> Arc<HashMap<AccountId, StatusText>> {
        self.servers
            .get(&server)
            .map(|server| server.texts.clone())
            .unwrap_or_default()
    }
    pub(crate) fn values(&self, server: ServerId) -> Arc<HashMap<AccountId, Presence>> {
        self.servers
            .get(&server)
            .map(|server| server.values.clone())
            .unwrap_or_default()
    }
}

struct Reader {
    owner: Owner,
    wanted: BTreeSet<AccountId>,
    stale: BTreeSet<AccountId>,
    last: Option<AccountId>,
    reading: Option<u64>,
    task: Option<Task<()>>,
    writing: Option<u64>,
    write_task: Option<Task<()>>,
    failed_read: bool,
    failed_choice: Option<PresenceChoice>,
    refresh_at: Instant,
}
impl Reader {
    fn new(owner: Owner) -> Self {
        Self {
            owner,
            wanted: BTreeSet::new(),
            stale: BTreeSet::new(),
            last: None,
            reading: None,
            task: None,
            writing: None,
            write_task: None,
            failed_read: false,
            failed_choice: None,
            refresh_at: Instant::now() + REFRESH,
        }
    }
    fn want(&mut self, mut wanted: BTreeSet<AccountId>) {
        wanted.insert(self.owner.account_id);
        self.stale.extend(wanted.difference(&self.wanted).copied());
        self.stale.retain(|id| wanted.contains(id));
        self.wanted = wanted;
    }
    fn next(&mut self) -> Vec<AccountId> {
        let mut rows: Vec<_> = self.stale.iter().copied().collect();
        if let Some(last) = self.last {
            let after = rows.partition_point(|id| *id <= last);
            rows.rotate_left(after);
        }
        rows.truncate(PRESENCE_PAGE);
        for id in &rows {
            self.stale.remove(id);
        }
        if let Some(last) = rows.last() {
            self.last = Some(*last);
        }
        rows
    }
}

type ActivityReader = Arc<dyn Fn() -> ActivityState + Send + Sync>;
#[derive(Default)]
pub(super) struct PresenceState {
    readers: HashMap<ServerId, Reader>,
    sequence: u64,
    cursor: usize,
    idle: bool,
    activity: Option<SystemActivity>,
    activity_reader: Option<ActivityReader>,
    activity_state: Option<ActivityState>,
    ticker: Option<Task<()>>,
    wake: Option<Task<()>>,
}

impl AppState {
    #[cfg(feature = "test-support")]
    pub fn show_presence_fixture(
        &mut self,
        owner: Owner,
        values: HashMap<AccountId, Presence>,
        cx: &mut Context<Self>,
    ) {
        let mut shown = ServerPresence::empty(owner, true);
        shown.choice = Some(PresenceChoice::Online);
        shown.values = Arc::new(values);
        self.known
            .presence
            .servers
            .insert(owner.entry.server_id, shown);
        cx.notify();
    }

    /// Shows these status texts on a server that a presence fixture shows.
    #[cfg(feature = "test-support")]
    pub fn show_status_text_fixture(
        &mut self,
        server: ServerId,
        texts: HashMap<AccountId, StatusText>,
        cx: &mut Context<Self>,
    ) {
        if let Some(shown) = self.known.presence.servers.get_mut(&server) {
            shown.texts = Arc::new(texts);
        }
        cx.notify();
    }

    /// The end of a status text that is set now.
    pub fn status_text_until(&self, end: StatusTextEnd) -> Option<time::OffsetDateTime> {
        end.until(time::OffsetDateTime::now_utc(), self.known.zone)
    }

    /// The end that the person typed as a date (`2026-11-30`) and a time (`18:00` or
    /// `6:00 PM`) on the clock of the person. It must be later than now.
    pub fn status_text_custom_until(
        &self,
        date: &str,
        clock: &str,
    ) -> Result<time::OffsetDateTime, &'static str> {
        let format = time::macros::format_description!("[year]-[month]-[day]");
        let date =
            time::Date::parse(date.trim(), format).map_err(|_| text::presence::END_NOT_READ)?;
        let minute = super::notices::parse_clock(clock).ok_or(text::presence::END_NOT_READ)?;
        let until = self
            .known
            .zone
            .at(date, minute)
            .ok_or(text::presence::END_NOT_READ)?;
        if until <= time::OffsetDateTime::now_utc() {
            return Err(text::presence::END_PASSED);
        }
        Ok(crate::moment::in_utc(until))
    }

    /// A status text in words, with its end on the clock of the person.
    pub fn status_text_words(&self, status: &StatusText) -> String {
        match status.until {
            Some(until) => text::presence::with_end(
                &status.text,
                &text::time_and_day(
                    self.known.zone.local(until),
                    self.known.today,
                    self.known.hour_cycle,
                ),
            ),
            None => status.text.clone(),
        }
    }

    /// The status text of an account, as the server of the account returned it.
    pub fn status_text(&self, server: ServerId, account: AccountId) -> Option<&StatusText> {
        self.known.presence.text(server, account)
    }
    pub fn presence(&self, server: ServerId, account: AccountId) -> Presence {
        self.known.presence.get(server, account)
    }
    pub fn presence_server(&self, server: ServerId) -> Option<&ServerPresence> {
        self.known.presence.server(server)
    }
    pub fn presence_activity(&self) -> Option<ActivityState> {
        self.presence.activity_state
    }

    pub(super) fn start_presence(&mut self, cx: &mut Context<Self>) {
        self.presence.ticker = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(5)).await;
                if this.update(cx, |this, cx| this.presence_tick(cx)).is_err() {
                    break;
                }
            }
        }));
    }

    pub(super) fn read_system_idle_duration(
        &mut self,
    ) -> impl Future<Output = Option<Duration>> + use<> {
        self.presence
            .activity
            .get_or_insert_with(SystemActivity::new)
            .read_idle_duration()
    }
    fn presence_tick(&mut self, cx: &mut Context<Self>) {
        self.afk_tick(cx);
        if !self.presence.readers.is_empty() {
            let activity = if let Some(reader) = &self.presence.activity_reader {
                reader()
            } else {
                self.presence
                    .activity
                    .get_or_insert_with(SystemActivity::new)
                    .state()
            };
            let idle = activity == ActivityState::Idle;
            if self.presence.activity_state != Some(activity) {
                self.presence.activity_state = Some(activity);
                cx.notify();
            }
            if idle != self.presence.idle {
                self.presence.idle = idle;
                for reader in self.presence.readers.values_mut() {
                    reader.stale.extend(&reader.wanted);
                }
            }
        }
        for reader in self.presence.readers.values_mut() {
            if Instant::now() >= reader.refresh_at {
                reader.stale.extend(&reader.wanted);
                reader.failed_read = false;
                reader.refresh_at = Instant::now() + REFRESH;
            }
        }
        self.read_presence(cx);
    }

    fn presence_scopes(&self) -> Vec<(Owner, ServerHandle)> {
        let snapshots = self.snapshots();
        self.client
            .servers()
            .into_iter()
            .filter_map(|server| {
                let snapshot = snapshots
                    .iter()
                    .find(|snapshot| snapshot.id == server.id())?;
                if snapshot.stream != StreamStatus::Live
                    || !snapshot
                        .server_events
                        .is_some_and(|revision| revision >= 11)
                {
                    return None;
                }
                let model = shown_model(snapshot)?;
                let owner = server
                    .owner()
                    .filter(|owner| owner.account_id == model.account.id)?;
                Some((owner, server))
            })
            .collect()
    }

    pub(super) fn follow_presence_current(&mut self, cx: &mut Context<Self>) {
        let wanted = self.followed_accounts();
        self.follow_presence(&wanted, cx);
    }

    pub(super) fn follow_presence(
        &mut self,
        wanted: &HashMap<ServerId, Wanted>,
        cx: &mut Context<Self>,
    ) {
        #[cfg(feature = "test-support")]
        if self.fixture.is_some() {
            return;
        }
        for server in self.client.servers() {
            server.live().presence_notifications(true);
        }
        let scopes = self.presence_scopes();
        let owners: Vec<_> = self
            .client
            .servers()
            .iter()
            .filter_map(ServerHandle::owner)
            .collect();
        self.presence
            .readers
            .retain(|_, reader| owners.contains(&reader.owner));
        self.known
            .presence
            .servers
            .retain(|_, shown| owners.contains(&shown.owner));
        for (server, reader) in &mut self.presence.readers {
            if !scopes.iter().any(|(owner, _)| *owner == reader.owner) {
                reader.reading = None;
                reader.task = None;
                reader.failed_read = false;
                reader.stale.extend(&reader.wanted);
                if let Some(known) = self.known.presence.servers.get_mut(server) {
                    known.connected = false;
                    known.reading = false;
                    known.values = Arc::default();
                    known.texts = Arc::default();
                }
            }
        }
        let mut wanted: HashMap<_, BTreeSet<AccountId>> = wanted
            .iter()
            .map(|(server, wanted)| (*server, wanted.accounts.iter().copied().collect()))
            .collect();
        if let Some(admin) = self.admin_view() {
            wanted
                .entry(admin.server_id)
                .or_default()
                .extend(super::people::admin_people(&admin));
        }
        if let Some(contacts) = self.contacts_view() {
            wanted
                .entry(contacts.server_id)
                .or_default()
                .extend(contacts.rows.iter().flatten().map(|row| row.account_id));
        }
        for window in self.switchers.window_ids() {
            if let Some(view) = self.switcher_view(window) {
                for row in view.rows {
                    if let Some(account) = row.account_id {
                        wanted
                            .entry(row.destination.server())
                            .or_default()
                            .insert(account);
                    }
                }
            }
        }
        for (owner, _) in scopes {
            let server = owner.entry.server_id;
            let reader = self
                .presence
                .readers
                .entry(server)
                .or_insert_with(|| Reader::new(owner));
            reader.want(wanted.remove(&server).unwrap_or_default());
            let takes_text = self
                .client
                .server(server)
                .is_some_and(|handle| handle.entry().capabilities.status_text);
            let known = self
                .known
                .presence
                .servers
                .entry(server)
                .or_insert_with(|| ServerPresence::empty(owner, takes_text));
            known.connected = true;
            known.takes_text = takes_text;
            Arc::make_mut(&mut known.values).retain(|id, _| reader.wanted.contains(id));
            Arc::make_mut(&mut known.texts).retain(|id, _| reader.wanted.contains(id));
        }
        self.read_presence(cx);
    }

    pub(super) fn presence_invalidated(
        &mut self,
        server: ServerId,
        clear: bool,
        cx: &mut Context<Self>,
    ) {
        if let Some(reader) = self.presence.readers.get_mut(&server) {
            reader.stale.extend(&reader.wanted);
            if clear {
                reader.reading = None;
                reader.task = None;
                reader.failed_read = false;
                if let Some(known) = self.known.presence.servers.get_mut(&server) {
                    known.values = Arc::default();
                    known.texts = Arc::default();
                    known.reading = false;
                    known.read_failed = None;
                }
                cx.notify();
            }
        }
        if self.presence.wake.is_none() {
            self.presence.wake = Some(cx.spawn(async move |this, cx| {
                cx.background_executor()
                    .timer(Duration::from_millis(250))
                    .await;
                let _ = this.update(cx, |this, cx| {
                    this.presence.wake = None;
                    this.read_presence(cx);
                });
            }));
        }
    }

    fn read_presence(&mut self, cx: &mut Context<Self>) {
        let scopes = self.presence_scopes();
        if scopes.is_empty() {
            return;
        }
        let mut room = READS.saturating_sub(
            self.presence
                .readers
                .values()
                .filter(|reader| reader.reading.is_some())
                .count(),
        );
        let start = self.presence.cursor % scopes.len();
        for offset in 0..scopes.len() {
            if room == 0 {
                break;
            }
            let index = (start + offset) % scopes.len();
            let (owner, server) = &scopes[index];
            let Some(reader) = self
                .presence
                .readers
                .get_mut(&owner.entry.server_id)
                .filter(|reader| reader.owner == *owner)
            else {
                continue;
            };
            if reader.reading.is_some() || reader.writing.is_some() || reader.failed_read {
                continue;
            }
            let accounts = reader.next();
            if accounts.is_empty() {
                continue;
            }
            room -= 1;
            self.presence.cursor = index + 1;
            self.presence.sequence += 1;
            let number = self.presence.sequence;
            reader.reading = Some(number);
            if let Some(known) = self.known.presence.servers.get_mut(&owner.entry.server_id) {
                known.reading = true;
            }
            let body = ReadPresence {
                accounts,
                idle: self.presence.idle,
            };
            let api = server.api_for(owner.account_id);
            let owner = *owner;
            reader.task = Some(cx.spawn(async move |this, cx| {
                let answer = api
                    .presence(&body)
                    .await
                    .map_err(|error| text::flow_error(&FlowError::from(&error)));
                let _ = this.update(cx, |this, cx| {
                    let server = owner.entry.server_id;
                    if !this
                        .presence_scopes()
                        .iter()
                        .any(|(current, _)| *current == owner)
                    {
                        this.follow_presence_current(cx);
                        cx.notify();
                        return;
                    }
                    if let Some(reader) =
                        this.presence.readers.get_mut(&server).filter(|reader| {
                            reader.owner == owner && reader.reading == Some(number)
                        })
                    {
                        reader.reading = None;
                        reader.task = None;
                        if let Some(known) = this.known.presence.servers.get_mut(&server) {
                            known.reading = false;
                        }
                        match answer {
                            Ok(page) => {
                                if let Some(known) = this.known.presence.servers.get_mut(&server) {
                                    known.choice = Some(page.own);
                                    known.read_failed = None;
                                    let values = Arc::make_mut(&mut known.values);
                                    let texts = Arc::make_mut(&mut known.texts);
                                    for row in page.accounts.into_iter().take(PRESENCE_PAGE) {
                                        if body.accounts.contains(&row.account_id)
                                            && reader.wanted.contains(&row.account_id)
                                        {
                                            values.insert(row.account_id, row.presence);
                                            match row.status_text {
                                                Some(text) => texts.insert(row.account_id, text),
                                                None => texts.remove(&row.account_id),
                                            };
                                        }
                                    }
                                }
                            }
                            Err(reason) => {
                                reader.failed_read = true;
                                reader.stale.extend(body.accounts);
                                if let Some(known) = this.known.presence.servers.get_mut(&server) {
                                    known.read_failed = Some(reason);
                                }
                            }
                        }
                        this.read_presence(cx);
                        cx.notify();
                    }
                });
            }));
        }
    }

    pub fn set_presence_choice(
        &mut self,
        server: ServerId,
        choice: PresenceChoice,
        cx: &mut Context<Self>,
    ) {
        let Some((owner, handle)) = self
            .presence_scopes()
            .into_iter()
            .find(|(owner, _)| owner.entry.server_id == server)
        else {
            return;
        };
        let Some(reader) = self
            .presence
            .readers
            .get_mut(&server)
            .filter(|reader| reader.owner == owner && reader.writing.is_none())
        else {
            return;
        };
        reader.reading = None;
        reader.task = None;
        reader.failed_choice = None;
        self.presence.sequence += 1;
        let number = self.presence.sequence;
        reader.writing = Some(number);
        if let Some(known) = self.known.presence.servers.get_mut(&server) {
            known.changing = true;
            known.failed = None;
            known.reading = false;
        }
        let api = handle.api_for(owner.account_id);
        reader.write_task = Some(cx.spawn(async move |this, cx| {
            let answer = api
                .set_presence(&SetPresence { choice })
                .await
                .map_err(|error| text::flow_error(&FlowError::from(&error)));
            let _ = this.update(cx, |this, cx| {
                if this.owner(owner.entry.server_id) != Some(owner) {
                    this.follow_presence_current(cx);
                    cx.notify();
                    return;
                }
                if let Some(reader) = this
                    .presence
                    .readers
                    .get_mut(&server)
                    .filter(|reader| reader.owner == owner && reader.writing == Some(number))
                {
                    reader.writing = None;
                    reader.write_task = None;
                    if let Some(known) = this.known.presence.servers.get_mut(&server) {
                        known.changing = false;
                        match answer {
                            Ok(_) => {
                                reader.failed_read = false;
                                known.choice = Some(choice);
                                if known.connected {
                                    Arc::make_mut(&mut known.values).insert(
                                        owner.account_id,
                                        choice.shown(true, this.presence.idle, true),
                                    );
                                }
                            }
                            Err(reason) => {
                                known.failed = Some(reason);
                                reader.failed_choice = Some(choice);
                            }
                        }
                    }
                    reader.stale.extend(&reader.wanted);
                    this.read_presence(cx);
                    cx.notify();
                }
            });
        }));
        cx.notify();
    }

    /// Sets or removes the own status text on one server (ADR 0401). The people who watch
    /// the account read it again; this device shows it at once.
    pub fn set_status_text(
        &mut self,
        server: ServerId,
        text: Option<StatusText>,
        cx: &mut Context<Self>,
    ) {
        let Some((owner, handle)) = self
            .presence_scopes()
            .into_iter()
            .find(|(owner, _)| owner.entry.server_id == server)
        else {
            return;
        };
        let Some(known) = self
            .known
            .presence
            .servers
            .get_mut(&server)
            .filter(|known| known.takes_text && !known.text_changing)
        else {
            return;
        };
        known.text_changing = true;
        known.text_failed = None;
        let api = handle.api_for(owner.account_id);
        let body = SetStatusText { status_text: text };
        cx.spawn(async move |this, cx| {
            let answer = api
                .set_status_text(&body)
                .await
                .map_err(|error| text::flow_error(&FlowError::from(&error)));
            let _ = this.update(cx, |this, cx| {
                if this.owner(server) != Some(owner) {
                    return;
                }
                if let Some(known) = this.known.presence.servers.get_mut(&server) {
                    known.text_changing = false;
                    match answer {
                        Ok(stored) => {
                            let texts = Arc::make_mut(&mut known.texts);
                            match stored.status_text {
                                Some(text) => texts.insert(owner.account_id, text),
                                None => texts.remove(&owner.account_id),
                            };
                        }
                        Err(reason) => known.text_failed = Some(reason),
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub fn retry_presence(&mut self, server: ServerId, cx: &mut Context<Self>) {
        if let Some(reader) = self.presence.readers.get_mut(&server) {
            reader.failed_read = false;
            reader.stale.extend(&reader.wanted);
        }
        self.read_presence(cx);
        cx.notify();
    }

    pub fn retry_presence_choice(&mut self, server: ServerId, cx: &mut Context<Self>) {
        if let Some(choice) = self
            .presence
            .readers
            .get(&server)
            .and_then(|reader| reader.failed_choice)
        {
            self.set_presence_choice(server, choice, cx);
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn use_activity_reader(
        &mut self,
        reader: impl Fn() -> ActivityState + Send + Sync + 'static,
        cx: &mut Context<Self>,
    ) {
        self.presence.activity_reader = Some(Arc::new(reader));
        self.presence.activity = None;
        self.presence_tick(cx);
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn refresh_activity(&mut self, cx: &mut Context<Self>) {
        self.presence_tick(cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use junto_client_core::EntryKey;

    #[test]
    fn a_status_text_ends_at_its_time_on_the_clock_of_the_person() {
        use time::macros::datetime;
        // A Wednesday, 15:10 in UTC, on a clock two hours ahead.
        let now = datetime!(2026-10-07 15:10 UTC);
        let zone = super::super::feed::Zone::Fixed(time::UtcOffset::from_hms(2, 0, 0).unwrap());
        assert_eq!(StatusTextEnd::Never.until(now, zone), None);
        assert_eq!(
            StatusTextEnd::HalfHour.until(now, zone),
            Some(datetime!(2026-10-07 15:40 UTC))
        );
        assert_eq!(
            StatusTextEnd::FourHours.until(now, zone),
            Some(datetime!(2026-10-07 19:10 UTC))
        );
        // Midnight on that clock is 22:00 in UTC.
        assert_eq!(
            StatusTextEnd::Today.until(now, zone),
            Some(datetime!(2026-10-07 22:00 UTC))
        );
        assert_eq!(
            StatusTextEnd::ThisWeek.until(now, zone),
            Some(datetime!(2026-10-11 22:00 UTC))
        );
    }
    fn reader() -> Reader {
        Reader::new(Owner {
            entry: EntryKey {
                server_id: ServerId::generate(),
                generation: 1,
            },
            account_id: AccountId::generate(),
        })
    }
    #[test]
    fn batches_are_bounded_and_repeated_invalidations_do_not_starve_later_accounts() {
        let mut reader = reader();
        reader.want((0..301).map(|_| AccountId::generate()).collect());
        let wanted = reader.wanted.clone();
        let mut read = BTreeSet::new();
        for _ in 0..4 {
            reader.stale.extend(&reader.wanted);
            let batch = reader.next();
            assert!(batch.len() <= PRESENCE_PAGE);
            read.extend(batch);
        }
        assert_eq!(read, wanted);
    }
    #[test]
    fn changed_interest_prunes_pending_accounts_and_keeps_the_owner() {
        let mut reader = reader();
        let old = AccountId::generate();
        let new = AccountId::generate();
        reader.want(BTreeSet::from([old]));
        reader.want(BTreeSet::from([new]));
        let batch: BTreeSet<_> = reader.next().into_iter().collect();
        assert_eq!(batch, BTreeSet::from([new, reader.owner.account_id]));
        assert!(reader.next().is_empty());
    }
}
