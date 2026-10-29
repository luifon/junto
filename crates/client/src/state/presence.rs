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
        presence::{PRESENCE_PAGE, Presence, PresenceChoice, ReadPresence, SetPresence},
    },
};

use super::{AppState, people::Wanted, view::shown_model};
use crate::text;

const READS: usize = 4;
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
        self.known.presence.servers.insert(
            owner.entry.server_id,
            ServerPresence {
                owner,
                choice: None,
                connected: true,
                changing: false,
                failed: None,
                read_failed: None,
                reading: false,
                values: Arc::new(values),
            },
        );
        cx.notify();
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

    fn presence_tick(&mut self, cx: &mut Context<Self>) {
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
                    || !snapshot.server_events.is_some_and(|revision| revision >= 3)
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
            let known = self
                .known
                .presence
                .servers
                .entry(server)
                .or_insert_with(|| ServerPresence {
                    owner,
                    connected: true,
                    choice: None,
                    changing: false,
                    failed: None,
                    read_failed: None,
                    reading: false,
                    values: Arc::default(),
                });
            known.connected = true;
            Arc::make_mut(&mut known.values).retain(|id, _| reader.wanted.contains(id));
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
                                    for row in page.accounts.into_iter().take(PRESENCE_PAGE) {
                                        if body.accounts.contains(&row.account_id)
                                            && reader.wanted.contains(&row.account_id)
                                        {
                                            values.insert(row.account_id, row.presence);
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
