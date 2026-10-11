//! The state of the application: the client core, what the person selected, and the work
//! that keeps the window current (ADR 0730).
//!
//! One entity holds all of it. It wakes on every change of the client core and of the
//! event stream of each server, and tells the window to draw again. The window reads plain
//! data through [`AppState::view`] and calls the methods here for what a person does.

pub mod view;

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use futures_channel::oneshot;
use gpui::{Context, Task};
use junto_client_core::api::ApiError;
use junto_client_core::book::ConnectionMode;
use junto_client_core::flow::FlowError;
use junto_client_core::flow::access::{AccessAction, AccessState};
use junto_client_core::protocol::ids::{AccountId, ChannelId, RecoveryId, ServerId, WorkspaceId};
use junto_client_core::settings::InterfaceSettings;
use junto_client_core::{Client, ServerHandle};

use crate::text;
use crate::theme;
use view::{Known, Selection, ServerSnapshot, ShellView, Shown};

/// The times of the work that the application does by itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timing {
    /// How often a request that waits for an administrator, or a device link, is checked.
    pub decision_interval: Duration,
    /// How often a server that could not be reached is contacted again.
    pub retry_interval: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            decision_interval: Duration::from_secs(5),
            retry_interval: Duration::from_secs(20),
        }
    }
}

/// The writes of files that a window started, in the order in which they were asked for.
/// The queue belongs to the application and not to a window: a write that was accepted is
/// done after its window has closed, and the application waits for the queue before it
/// ends (see [`finish_writes`]). The writes run on background threads, so they go on
/// while the interface thread waits for them at the end.
#[derive(Default)]
struct Writes {
    last: Option<Task<()>>,
}

impl gpui::Global for Writes {}

/// How long the application waits at its end for the writes that were accepted. A write
/// takes this long only when another program holds the lock of the data directory.
const WRITE_WAIT: Duration = Duration::from_secs(5);

/// Waits for the writes that still wait or run. The application calls this when it quits,
/// on the interface thread.
pub fn finish_writes(cx: &mut gpui::App) {
    let Some(last) = cx.default_global::<Writes>().last.take() else {
        return;
    };
    if cx
        .foreground_executor()
        .block_with_timeout(WRITE_WAIT, last)
        .is_err()
    {
        tracing::error!("a write of a file had not ended when the application ended");
    }
}

/// A setting of the interface, for the failure of its write.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Setting {
    Scale,
    ServerBook,
}

/// How often the name of one account is asked for when no answer arrives.
const NAME_TRIES: u8 = 3;

/// Where the read of the name of one account stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NameRead {
    /// A request is on its way. The number counts the requests so far.
    Asking(u8),
    /// No request is on its way, and this number of requests got no answer.
    Unanswered(u8),
    /// The name was read, or the server refused: it is not asked for again.
    Settled,
}

/// What the person asked for and that has not ended yet, or has failed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Pending {
    /// The recoveries whose acknowledgement is on its way.
    pub acknowledging: HashSet<RecoveryId>,
    /// Why the last acknowledgement of a recovery failed, per server.
    pub acknowledge_failed: HashMap<ServerId, String>,
    /// Why a server could not be removed.
    pub remove_failed: HashMap<ServerId, String>,
    /// The servers whose removal is on its way. A second removal of one of them is not
    /// started: it would be decided against the server book of a later moment.
    pub removing: HashSet<ServerId>,
    /// The servers whose waiting request is being given up.
    pub cancelling: HashSet<ServerId>,
    /// The name of the profile preset is being written.
    pub saving_name: bool,
}

pub struct AppState {
    client: Client,
    timing: Timing,
    settings: InterfaceSettings,
    /// The interface scale of this start, from a start argument. It is not stored.
    scale_of_this_start: Option<u32>,
    /// The result of the last change of the profile name, until the window has taken it.
    name_result: Option<(String, Result<(), String>)>,
    /// Why a setting could not be written. The failure of one setting stays until that
    /// setting is in the file.
    settings_failed: HashMap<Setting, String>,
    selection: Selection,
    known: Known,
    pending: Pending,
    /// What the window showed after the last change. Side effects follow a change of it.
    shown: Shown,
    /// The accounts whose name was asked for from their profile route.
    asked_names: HashMap<(ServerId, AccountId), NameRead>,
    /// The workspace whose members the window needs, and the task that reads them.
    members: Option<((ServerId, WorkspaceId), Task<()>)>,
    /// One task per saved server that wakes on each change of its event stream.
    watchers: HashMap<ServerId, Task<()>>,
    /// One task per server that waits for a decision of another person.
    waiting: HashMap<ServerId, Task<()>>,
    /// The servers that the view is derived from in place of the client core.
    #[cfg(feature = "test-support")]
    fixture: Option<Vec<ServerSnapshot>>,
    _changes: Task<()>,
    _retry: Task<()>,
}

impl AppState {
    /// Starts the event streams, contacts every saved server and follows their changes.
    pub fn new(client: Client, timing: Timing, cx: &mut Context<Self>) -> Self {
        let settings = match client.settings().load() {
            Ok(settings) => settings,
            Err(error) => {
                tracing::warn!(%error, "the interface settings were not read");
                InterfaceSettings::default()
            }
        };
        if let Err(error) = client.start_event_streams() {
            tracing::error!(%error, "the event streams did not start");
        }
        let connecting = client.connect_all();
        cx.spawn(async move |_, _| {
            connecting.await;
        })
        .detach();

        let mut changes = client.changes();
        let follow = cx.spawn(async move |this, cx| {
            while changes.changed().await {
                if this.update(cx, |this, cx| this.changed(cx)).is_err() {
                    break;
                }
            }
        });
        let retry_interval = timing.retry_interval;
        let retry = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(retry_interval).await;
                if this
                    .update(cx, |this, cx| this.retry_unreached(cx))
                    .is_err()
                {
                    break;
                }
            }
        });
        let known = Known {
            preset_name: client
                .profile()
                .load()
                .map(|preset| preset.name)
                .unwrap_or_default(),
            ..Known::default()
        };
        let mut state = Self {
            client,
            timing,
            settings,
            scale_of_this_start: None,
            name_result: None,
            settings_failed: HashMap::new(),
            selection: Selection::default(),
            known,
            pending: Pending::default(),
            shown: Shown::default(),
            asked_names: HashMap::new(),
            members: None,
            watchers: HashMap::new(),
            waiting: HashMap::new(),
            #[cfg(feature = "test-support")]
            fixture: None,
            _changes: follow,
            _retry: retry,
        };
        state.changed(cx);
        state
    }

    pub fn client(&self) -> &Client {
        &self.client
    }

    /// The settings as the window uses them: the stored ones, with the interface scale of
    /// this start when a start argument gave one.
    pub fn settings(&self) -> InterfaceSettings {
        InterfaceSettings {
            scale_percent: self
                .scale_of_this_start
                .unwrap_or(self.settings.scale_percent),
            ..self.settings
        }
    }

    /// Uses an interface scale for this start only. The stored setting stays.
    pub fn use_scale_for_this_start(&mut self, scale_percent: u32, cx: &mut Context<Self>) {
        self.scale_of_this_start = Some(theme::clamp_scale(scale_percent));
        cx.notify();
    }

    /// Why a setting that the window shows is not in the file.
    pub fn settings_error(&self) -> Option<&str> {
        [Setting::Scale, Setting::ServerBook]
            .iter()
            .find_map(|setting| self.settings_failed.get(setting))
            .map(String::as_str)
    }

    /// Runs `work` on a background thread after every write that was asked for before,
    /// and then gives its result to `done`. The work is done also when this state has
    /// ended by then; `done` runs only while the state lives.
    fn queue_write<R: Send + 'static>(
        cx: &mut Context<Self>,
        work: impl FnOnce() -> R + Send + 'static,
        done: impl FnOnce(&mut Self, R, &mut Context<Self>) + 'static,
    ) {
        let before = cx.default_global::<Writes>().last.take();
        let (result, written) = oneshot::channel();
        let write = cx.background_executor().spawn(async move {
            if let Some(before) = before {
                before.await;
            }
            let _ = result.send(work());
        });
        cx.default_global::<Writes>().last = Some(write);
        cx.spawn(async move |this, cx| {
            if let Ok(result) = written.await {
                let _ = this.update(cx, |this, cx| done(this, result, cx));
            }
        })
        .detach();
    }

    pub fn pending(&self) -> &Pending {
        &self.pending
    }

    /// The name of the profile preset. Empty until the person has chosen one.
    pub fn preset_name(&self) -> &str {
        &self.known.preset_name
    }

    pub fn server(&self, server_id: ServerId) -> Option<ServerHandle> {
        self.client.server(server_id)
    }

    /// What the window shows now.
    pub fn view(&self) -> ShellView {
        view::derive(&self.snapshots(), &self.selection, &self.known)
    }

    fn snapshots(&self) -> Vec<ServerSnapshot> {
        #[cfg(feature = "test-support")]
        if let Some(fixture) = &self.fixture {
            return fixture.clone();
        }
        self.client
            .servers()
            .iter()
            .map(ServerSnapshot::of)
            .collect()
    }

    /// Draws these servers in place of the servers of the client core. The screen tool
    /// uses it to draw a state that no server of a test gives, such as a ban.
    #[cfg(feature = "test-support")]
    pub fn show_fixture(&mut self, servers: Vec<ServerSnapshot>, cx: &mut Context<Self>) {
        self.fixture = Some(servers);
        self.changed(cx);
    }

    /// Runs after every change of the client core and of an event stream: starts and ends
    /// the work that depends on what exists and on what is shown, and draws again.
    fn changed(&mut self, cx: &mut Context<Self>) {
        let servers = self.client.servers();
        let ids: HashSet<ServerId> = servers.iter().map(ServerHandle::id).collect();
        self.watchers.retain(|id, _| ids.contains(id));
        self.waiting.retain(|id, _| ids.contains(id));
        self.pending
            .acknowledge_failed
            .retain(|id, _| ids.contains(id));
        self.pending.remove_failed.retain(|id, _| ids.contains(id));
        for server in &servers {
            let id = server.id();
            self.watchers.entry(id).or_insert_with(|| {
                let mut watch = server.live().watch();
                cx.spawn(async move |this, cx| {
                    while watch.changed().await.is_some() {
                        if this.update(cx, |this, cx| this.changed(cx)).is_err() {
                            break;
                        }
                    }
                })
            });
            let waits = matches!(
                server.access().state,
                AccessState::WaitingForApproval { .. } | AccessState::WaitingForLink { .. }
            );
            if !waits {
                self.waiting.remove(&id);
            } else if !self.waiting.contains_key(&id) {
                let decision = server.wait_for_decision(self.timing.decision_interval);
                let task = cx.spawn(async move |this, cx| {
                    // A failure that ends the wait stays in the state of the flow. The
                    // next change of the server starts a new wait while it still waits.
                    let _ = decision.await;
                    let _ = this.update(cx, |this, cx| {
                        this.waiting.remove(&id);
                        cx.notify();
                    });
                });
                self.waiting.insert(id, task);
            }
        }

        let view = self.view();
        let shown = view.shown();
        if shown.server != self.shown.server {
            self.client.select(shown.server);
        }
        let wanted = shown.server.zip(shown.workspace);
        if self.members.as_ref().map(|(key, _)| *key) != wanted {
            if let Some(((server_id, workspace_id), _)) = self.members.take()
                && let Some(server) = self.client.server(server_id)
            {
                server.live().release_members(workspace_id);
            }
            if let Some(key) = wanted {
                self.load_members(key, cx);
            }
        }
        self.shown = shown;
        self.ask_missing_names(&view, cx);
        cx.notify();
    }

    fn load_members(&mut self, key: (ServerId, WorkspaceId), cx: &mut Context<Self>) {
        let Some(server) = self.client.server(key.0) else {
            return;
        };
        self.known.members_failed.retain(|failed| *failed != key);
        let loading = server.live().load_members(key.1);
        let task = cx.spawn(async move |this, cx| {
            let result = loading.await;
            let _ = this.update(cx, |this, cx| {
                if result.is_err() && !this.known.members_failed.contains(&key) {
                    this.known.members_failed.push(key);
                }
                cx.notify();
            });
        });
        self.members = Some((key, task));
    }

    /// Reads the members of the shown workspace again after a read that failed.
    pub fn reload_members(&mut self, cx: &mut Context<Self>) {
        if let Some((key, _)) = self.members.take() {
            self.load_members(key, cx);
            cx.notify();
        }
    }

    /// A person in a voice channel who is not in the member list of the workspace has no
    /// name there. The name is read once from the profile route.
    fn ask_missing_names(&mut self, view: &ShellView, cx: &mut Context<Self>) {
        let (Some(selected), Some(workspace)) = (&view.selected, view.workspace()) else {
            return;
        };
        if !matches!(workspace.members, view::Members::Loaded(_)) {
            return;
        }
        let server_id = selected.id;
        let Some(server) = self.client.server(server_id) else {
            return;
        };
        let missing: Vec<AccountId> = workspace
            .sections
            .iter()
            .flat_map(|section| section.rows.iter())
            .filter_map(|row| row.voice.as_ref())
            .flat_map(|voice| voice.occupants.iter())
            .filter(|occupant| occupant.name.is_none())
            .map(|occupant| occupant.account_id)
            .filter(|account_id| {
                let read = self
                    .asked_names
                    .entry((server_id, *account_id))
                    .or_insert(NameRead::Unanswered(0));
                match *read {
                    NameRead::Unanswered(tries) if tries < NAME_TRIES => {
                        *read = NameRead::Asking(tries + 1);
                        true
                    }
                    NameRead::Unanswered(_) | NameRead::Asking(_) | NameRead::Settled => false,
                }
            })
            .collect();
        for account_id in missing {
            let api = server.api();
            cx.spawn(async move |this, cx| {
                let read = api.profile(account_id).await;
                let _ = this.update(cx, |this, cx| {
                    let key = (server_id, account_id);
                    let tries = match this.asked_names.get(&key) {
                        Some(NameRead::Asking(tries)) => *tries,
                        _ => NAME_TRIES,
                    };
                    let next = match read {
                        Ok(profile) => {
                            this.known.names.insert(key, profile.name);
                            cx.notify();
                            NameRead::Settled
                        }
                        // No answer: the name is asked for again with a later change.
                        Err(error) if error.is_unavailable() => NameRead::Unanswered(tries),
                        // A refusal is final: the account may not see the profile.
                        Err(_) => NameRead::Settled,
                    };
                    this.asked_names.insert(key, next);
                });
            })
            .detach();
        }
    }

    /// Contacts again every server that this device is a member of and that has no
    /// session: the server could not be reached, or its login failed.
    fn retry_unreached(&mut self, cx: &mut Context<Self>) {
        for server in self.client.servers() {
            let view = server.access();
            if matches!(view.state, AccessState::LoggedOut { .. }) && view.in_progress.is_none() {
                Self::run(server.connect(), cx);
            }
        }
    }

    /// Runs an action of a flow. Its result is in the state of the flow.
    fn run(action: impl Future<Output = Result<(), FlowError>> + 'static, cx: &mut Context<Self>) {
        cx.spawn(async move |_, _| {
            let _ = action.await;
        })
        .detach();
    }

    pub fn select_server(&mut self, server_id: ServerId, cx: &mut Context<Self>) {
        self.selection.server = Some(server_id);
        self.changed(cx);
    }

    pub fn select_workspace(
        &mut self,
        server_id: ServerId,
        workspace_id: WorkspaceId,
        cx: &mut Context<Self>,
    ) {
        self.selection.server = Some(server_id);
        self.selection.workspaces.insert(server_id, workspace_id);
        self.changed(cx);
    }

    pub fn select_channel(
        &mut self,
        workspace_id: WorkspaceId,
        channel_id: ChannelId,
        cx: &mut Context<Self>,
    ) {
        self.selection.channels.insert(workspace_id, channel_id);
        self.changed(cx);
    }

    /// Asks the server what this device is now, or logs in.
    pub fn connect(&mut self, server_id: ServerId, cx: &mut Context<Self>) {
        if let Some(server) = self.client.server(server_id) {
            Self::run(server.connect(), cx);
        }
    }

    /// Deletes the key of this device for the server. The server stays in the book.
    pub fn forget_account(&mut self, server_id: ServerId, cx: &mut Context<Self>) {
        if let Some(server) = self.client.server(server_id) {
            Self::run(server.dispatch_when_free(AccessAction::ForgetAccount), cx);
        }
    }

    /// Removes the server from the book with the key of this device. The files are changed
    /// on a background thread; a failure is told in [`Pending::remove_failed`]. Returns
    /// false, and starts nothing, while a removal of this server runs.
    pub fn remove_server(&mut self, server_id: ServerId, cx: &mut Context<Self>) -> bool {
        if !self.pending.removing.insert(server_id) {
            return false;
        }
        let client = self.client.clone();
        Self::queue_write(
            cx,
            move || client.remove_server(server_id),
            move |this, removed, cx| this.removed(server_id, removed, cx),
        );
        cx.notify();
        true
    }

    fn removed(
        &mut self,
        server_id: ServerId,
        result: Result<(), junto_client_core::manager::RemoveError>,
        cx: &mut Context<Self>,
    ) {
        self.pending.removing.remove(&server_id);
        match result {
            Ok(()) => {
                self.pending.remove_failed.remove(&server_id);
            }
            Err(error) => {
                let failure = match error {
                    junto_client_core::manager::RemoveError::Busy => FlowError::Busy,
                    other => FlowError::Storage(other.to_string()),
                };
                self.pending
                    .remove_failed
                    .insert(server_id, text::flow_error(&failure));
            }
        }
        self.changed(cx);
    }

    /// Gives up a request that waits: forgets it, then removes the server from the book.
    /// When the request was approved meanwhile, the device has its account and nothing is
    /// removed. Every other failure is told in [`Pending::remove_failed`].
    pub fn cancel_request(&mut self, server_id: ServerId, cx: &mut Context<Self>) {
        let Some(server) = self.client.server(server_id) else {
            return;
        };
        if self.pending.removing.contains(&server_id) || !self.pending.cancelling.insert(server_id)
        {
            return;
        }
        self.pending.removing.insert(server_id);
        self.pending.remove_failed.remove(&server_id);
        self.waiting.remove(&server_id);
        let client = self.client.clone();
        let forgetting = server.dispatch_when_free(AccessAction::ForgetAccount);
        cx.spawn(async move |this, cx| {
            let forgotten = forgetting.await;
            let removed = match &forgotten {
                Ok(()) => Some(
                    cx.background_executor()
                        .spawn(async move { client.remove_server(server_id) })
                        .await,
                ),
                Err(_) => None,
            };
            let _ = this.update(cx, |this, cx| {
                this.pending.cancelling.remove(&server_id);
                this.pending.removing.remove(&server_id);
                match (forgotten, removed) {
                    (_, Some(removed)) => this.removed(server_id, removed, cx),
                    // The state no longer accepts the action: the request was decided
                    // while the person cancelled it. The state of the server shows how.
                    (Err(FlowError::NotAvailable), None) => this.changed(cx),
                    (Err(error), None) => {
                        this.pending
                            .remove_failed
                            .insert(server_id, text::flow_error(&error));
                        this.changed(cx);
                    }
                    (Ok(()), None) => this.changed(cx),
                }
            });
        })
        .detach();
        cx.notify();
    }

    pub fn set_keep_connected(&mut self, server_id: ServerId, keep: bool, cx: &mut Context<Self>) {
        let Some(server) = self.client.server(server_id) else {
            return;
        };
        let mode = if keep {
            ConnectionMode::Always
        } else {
            ConnectionMode::WhenSelected
        };
        // The writes of one server run in the order of the clicks. The client core tells
        // its change when the book is written.
        Self::queue_write(
            cx,
            move || server.set_connection_mode(mode),
            |_, written, _| {
                if let Err(error) = written {
                    tracing::warn!(%error, "the connection mode was not saved");
                }
            },
        );
    }

    /// States that the person has seen a recovery (ADR 0118).
    pub fn acknowledge_recovery(
        &mut self,
        server_id: ServerId,
        recovery_id: RecoveryId,
        cx: &mut Context<Self>,
    ) {
        let Some(server) = self.client.server(server_id) else {
            return;
        };
        if !self.pending.acknowledging.insert(recovery_id) {
            return;
        }
        self.pending.acknowledge_failed.remove(&server_id);
        let sending = server.acknowledge_recovery(recovery_id);
        cx.spawn(async move |this, cx| {
            let result = sending.await;
            let _ = this.update(cx, |this, cx| {
                this.pending.acknowledging.remove(&recovery_id);
                // The server holds no notice of this device any more: the recovery left
                // the state, and there is nothing to tell.
                let gone = matches!(
                    &result,
                    Err(ApiError::Rejected(rejection))
                        if rejection.code
                            == junto_client_core::protocol::error::ErrorCode::NotFound
                );
                if let (Err(error), false) = (&result, gone) {
                    this.pending
                        .acknowledge_failed
                        .insert(server_id, text::flow_error(&FlowError::from(error)));
                }
                this.changed(cx);
            });
        })
        .detach();
        cx.notify();
    }

    /// Sets the name of the profile preset. The name is checked at once; the file is
    /// written on a background thread. The window takes the result with
    /// [`AppState::take_name_result`] when this state tells a change.
    pub fn set_preset_name(&mut self, name: &str, cx: &mut Context<Self>) {
        if self.pending.saving_name {
            return;
        }
        let name = match junto_client_core::profile::account_name(name) {
            Ok(name) => name,
            Err(invalid) => {
                let failure = text::flow_error(&FlowError::from(invalid));
                self.name_result = Some((name.to_owned(), Err(failure)));
                cx.notify();
                return;
            }
        };
        self.pending.saving_name = true;
        let profile = self.client.profile().clone();
        let submitted = name.clone();
        Self::queue_write(
            cx,
            move || profile.set_name(&name),
            move |this, saved, cx| {
                this.pending.saving_name = false;
                let saved = match saved {
                    Ok(preset) => {
                        this.known.preset_name = preset.name;
                        Ok(())
                    }
                    Err(junto_client_core::profile::ProfileError::Name(invalid)) => {
                        Err(text::flow_error(&FlowError::from(invalid)))
                    }
                    Err(other) => Err(text::flow_error(&FlowError::Storage(other.to_string()))),
                };
                this.name_result = Some((submitted, saved));
                cx.notify();
            },
        );
        cx.notify();
    }

    /// The name that was submitted last, and whether it was stored. Each result is given
    /// once.
    pub fn take_name_result(&mut self) -> Option<(String, Result<(), String>)> {
        self.name_result.take()
    }

    /// Sets the interface scale and stores it.
    pub fn set_scale(&mut self, scale_percent: u32, cx: &mut Context<Self>) {
        self.scale_of_this_start = None;
        self.settings.scale_percent = theme::clamp_scale(scale_percent);
        self.write_setting(Setting::Scale, cx);
    }

    pub fn set_server_book_collapsed(&mut self, collapsed: bool, cx: &mut Context<Self>) {
        self.settings.server_book_collapsed = collapsed;
        self.write_setting(Setting::ServerBook, cx);
    }

    /// Writes one changed setting on a background thread, in the order of the changes. The
    /// window uses the new value at once. Only the changed setting is written, so that a
    /// second program that changed another one keeps its value. A setting whose write
    /// failed before is written again with this one, and its failure is told in
    /// [`AppState::settings_error`] until it is in the file.
    fn write_setting(&mut self, changed: Setting, cx: &mut Context<Self>) {
        let mut writing: Vec<Setting> = self.settings_failed.keys().copied().collect();
        if !writing.contains(&changed) {
            writing.push(changed);
        }
        let (store, wanted, written) = (self.client.settings(), self.settings, writing.clone());
        Self::queue_write(
            cx,
            move || {
                store.update(|stored| {
                    for setting in &writing {
                        match setting {
                            Setting::Scale => stored.scale_percent = wanted.scale_percent,
                            Setting::ServerBook => {
                                stored.server_book_collapsed = wanted.server_book_collapsed;
                            }
                        }
                    }
                })
            },
            move |this, result, cx| {
                match result {
                    Ok(_) => {
                        for setting in &written {
                            this.settings_failed.remove(setting);
                        }
                    }
                    Err(error) => {
                        let failure = text::flow_error(&FlowError::Storage(error.to_string()));
                        for setting in written {
                            this.settings_failed.insert(setting, failure.clone());
                        }
                    }
                }
                cx.notify();
            },
        );
        cx.notify();
    }
}
