//! What the application does for notices and notification settings (ADR 0742): the list of
//! the side panel, the marks, the jump to a message, the settings of a scope, the quiet
//! times, and the notifications of the system.

use std::rc::Rc;

use gpui::Context;
use junto_client_core::ServerHandle;
use junto_client_core::api::{ApiError, SettingsScope};
use junto_client_core::flow::FlowError;
use junto_client_core::flow::access::AccessState;
use junto_client_core::model::PlaceId;
use junto_client_core::protocol::discovery::{DailyQuiet, NotificationLevel, NotificationSettings};
use junto_client_core::protocol::ids::{AccountId, ChannelId, ServerId, WorkspaceId};
use time::OffsetDateTime;
use uuid::Uuid;

use super::AppState;
use super::notices::{
    AlertScene, AlertTarget, DefaultRead, Jump, MuteFor, alerts, stored_settings,
};
use super::view::{ServerSnapshot, Shown};
use crate::notify::{Kept, SystemNotifier};
use crate::{text, zones};

/// One of the two mention switches of a channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MentionSwitch {
    Roles,
    Everyone,
}

fn refusal(error: &ApiError) -> String {
    text::flow_error(&FlowError::from(error))
}

impl AppState {
    // The list of notices.

    /// Shows the notices of a server in the side panel.
    pub fn open_notices(&mut self, server_id: ServerId, cx: &mut Context<Self>) {
        self.selection.notices = Some(server_id);
        self.selection.thread = None;
        self.selection.pins = None;
        self.selection.transcript = None;
        self.known.transcript_line = None;
        self.known.fork = None;
        // The side panel shows one list: a search gives its place to the notices.
        if let Some(search) = &mut self.known.search {
            search.open = false;
        }
        self.changed(cx);
    }

    pub fn close_notices(&mut self, cx: &mut Context<Self>) {
        self.selection.notices = None;
        self.changed(cx);
    }

    /// Reads the notices after the listed ones.
    pub fn read_more_notices(&mut self, server_id: ServerId) {
        if let Some(server) = self.client.server(server_id) {
            server.live().read_more_notices();
        }
    }

    /// Marks one notice as read on every device of the account.
    pub fn mark_notice_read(
        &mut self,
        server_id: ServerId,
        notice_id: Uuid,
        cx: &mut Context<Self>,
    ) {
        self.mark_notices(server_id, vec![notice_id], cx);
    }

    /// Marks every listed notice that the person has not read. The server has a route
    /// for one notice: the requests follow each other, and the first one that fails ends
    /// them with its reason.
    pub fn mark_all_notices_read(&mut self, server_id: ServerId, cx: &mut Context<Self>) {
        let unread: Vec<Uuid> = self
            .client
            .server(server_id)
            .and_then(|server| server.live().view().model)
            .map(|model| model.unread_notices().map(|notice| notice.id).collect())
            .unwrap_or_default();
        self.mark_notices(server_id, unread, cx);
    }

    /// Marks notices as read, one request after the other. Marks that are asked for
    /// while requests of the server are on their way follow them.
    fn mark_notices(&mut self, server_id: ServerId, notices: Vec<Uuid>, cx: &mut Context<Self>) {
        if notices.is_empty() || self.client.server(server_id).is_none() {
            return;
        }
        // A mark that waits or is on its way is not asked for a second time.
        let sending = self
            .marks_sending
            .get(&server_id)
            .cloned()
            .unwrap_or_default();
        let waiting = self.marks_waiting.entry(server_id).or_default();
        for notice_id in notices {
            if !waiting.contains(&notice_id) && !sending.contains(&notice_id) {
                waiting.push(notice_id);
            }
        }
        self.known.notices_failed.remove(&server_id);
        self.send_marks(server_id, cx);
        self.changed(cx);
    }

    fn send_marks(&mut self, server_id: ServerId, cx: &mut Context<Self>) {
        if self.known.notices_marking.contains(&server_id) {
            return;
        }
        let notices = self.marks_waiting.remove(&server_id).unwrap_or_default();
        let Some(server) = self
            .client
            .server(server_id)
            .filter(|_| !notices.is_empty())
        else {
            return;
        };
        self.known.notices_marking.insert(server_id);
        self.marks_sending.insert(server_id, notices.clone());
        let api = server.api();
        cx.spawn(async move |this, cx| {
            let mut failed = None;
            for notice_id in notices {
                match api.mark_notification_read(notice_id).await {
                    Ok(_) => {}
                    // The notice is gone: its message was deleted, or the access to
                    // its place ended.
                    Err(error)
                        if error.category()
                            == Some(junto_client_core::api::ErrorCategory::NotFound) => {}
                    Err(error) => {
                        failed = Some(refusal(&error));
                        break;
                    }
                }
            }
            let _ = this.update(cx, |this, cx| {
                this.known.notices_marking.remove(&server_id);
                this.marks_sending.remove(&server_id);
                // The first request that fails ends the marks that were sent with it.
                if let Some(failed) = failed {
                    this.known.notices_failed.insert(server_id, failed);
                }
                // The marks that were asked for meanwhile are sent by themselves.
                this.send_marks(server_id, cx);
                this.changed(cx);
            });
        })
        .detach();
    }

    pub fn dismiss_notices_failure(&mut self, server_id: ServerId, cx: &mut Context<Self>) {
        self.known.notices_failed.remove(&server_id);
        self.changed(cx);
    }

    // The jump to a message.

    /// Shows a message in its place: the channel or the conversation is selected, the
    /// thread of a reply is opened, and the messages around the message are read when
    /// the list does not hold it. The list marks the message.
    pub fn go_to(&mut self, jump: Jump, cx: &mut Context<Self>) {
        let server_id = jump.server_id;
        self.navigations += 1;
        if let Some(search) = &mut self.known.search {
            search.going = None;
        }
        self.selection.server = Some(server_id);
        match jump.place {
            PlaceId::Channel(channel_id) => {
                if let Some(workspace_id) = jump.workspace_id {
                    self.selection.workspaces.insert(server_id, workspace_id);
                    self.selection
                        .channels
                        .insert((server_id, workspace_id), channel_id);
                }
                self.selection.conversations.remove(&server_id);
                if let Some(root_id) = jump.thread_root {
                    self.selection.thread = Some((server_id, channel_id, root_id));
                    self.selection.pins = None;
                    self.known.fork = None;
                }
            }
            PlaceId::Direct(conversation_id) => {
                self.selection
                    .conversations
                    .insert(server_id, conversation_id);
                self.known.conversation_refused = None;
            }
            PlaceId::Thread { .. } => return,
        }
        self.jump = Some(jump);
        self.known.target = Some((server_id, jump.message_id, self.navigations));
        self.changed(cx);
    }

    /// The place whose list shows the message of a jump.
    fn jump_place(jump: &Jump) -> PlaceId {
        match (jump.place, jump.thread_root) {
            (PlaceId::Channel(channel_id), Some(root_id)) => PlaceId::Thread {
                channel_id,
                root_id,
            },
            (place, _) => place,
        }
    }

    /// Reads the messages around the message of the jump that waits, once the client core
    /// holds the list of its place. A jump whose place the window does not show any more
    /// is given up.
    pub(super) fn follow_jump(&mut self, shown: Shown) {
        let Some(jump) = self.jump else {
            return;
        };
        let place = Self::jump_place(&jump);
        let wanted = shown.server == Some(jump.server_id)
            && (shown.place() == Some(place) || shown.thread_place() == Some(place));
        let Some(server) = self.client.server(jump.server_id).filter(|_| wanted) else {
            self.jump = None;
            return;
        };
        let model = server.live().view().model;
        let Some(timeline) = model.as_ref().and_then(|model| model.timeline(place)) else {
            return;
        };
        if timeline.stale || !timeline.loaded {
            return;
        }
        self.jump = None;
        if timeline.message(jump.message_id).is_none() {
            self.anchors
                .insert((jump.server_id, place), Some(jump.position));
            server.live().read_messages_around(place, jump.position);
        }
    }

    /// The person went to another place: no jump waits, and no message is marked.
    pub(super) fn end_jump(&mut self) {
        self.jump = None;
        self.known.target = None;
        // The answer for a result of a search that is being read leads nowhere then.
        if let Some(search) = &mut self.known.search {
            search.going = None;
        }
    }

    // Notification settings.

    /// Shows the notification settings of a scope in their dialog. The level that the
    /// administrators set for the workspace is read for the dialog of a workspace and of
    /// a channel.
    pub fn open_notify_settings(
        &mut self,
        server_id: ServerId,
        scope: SettingsScope,
        cx: &mut Context<Self>,
    ) {
        self.selection.notify = Some((server_id, scope));
        self.known.notify_refused = None;
        self.read_default_of(server_id, scope, false, cx);
        self.changed(cx);
    }

    /// Reads the level that the administrators set for the workspace of a scope.
    fn read_default_of(
        &mut self,
        server_id: ServerId,
        scope: SettingsScope,
        again: bool,
        cx: &mut Context<Self>,
    ) {
        let workspace_id = match scope {
            SettingsScope::Account => None,
            SettingsScope::Workspace(workspace_id) => Some(workspace_id),
            SettingsScope::Channel(channel_id) => self
                .client
                .server(server_id)
                .and_then(|server| server.live().view().model)
                .and_then(|model| {
                    model
                        .channel(channel_id)
                        .map(|channel| channel.channel.workspace_id)
                }),
        };
        if let Some(workspace_id) = workspace_id {
            self.read_workspace_default(server_id, workspace_id, again, cx);
        }
    }

    pub fn close_notify_settings(&mut self, cx: &mut Context<Self>) {
        self.selection.notify = None;
        self.known.notify_refused = None;
        self.changed(cx);
    }

    fn read_workspace_default(
        &mut self,
        server_id: ServerId,
        workspace_id: WorkspaceId,
        again: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(server) = self.client.server(server_id) else {
            return;
        };
        let key = (server_id, workspace_id);
        // A read after the state was replaced takes the place of one that is on its
        // way: that one can carry the level of before.
        if !again && self.known.workspace_defaults.get(&key) == Some(&DefaultRead::Reading) {
            return;
        }
        self.known
            .workspace_defaults
            .insert(key, DefaultRead::Reading);
        let number = self.next_default_ask(key);
        let api = server.api();
        cx.spawn(async move |this, cx| {
            let read = api.notification_default(workspace_id).await;
            let _ = this.update(cx, |this, cx| {
                // A later read or change of this level decides what is shown.
                if this.default_asks.get(&key) != Some(&number) {
                    return;
                }
                this.known.workspace_defaults.insert(
                    key,
                    match read {
                        Ok(level) => DefaultRead::Read(level),
                        Err(_) => DefaultRead::Failed,
                    },
                );
                this.changed(cx);
            });
        })
        .detach();
    }

    /// The number of a new read or change of the level of a workspace. The answer of
    /// an earlier one is not shown.
    fn next_default_ask(&mut self, key: (ServerId, WorkspaceId)) -> u64 {
        let number = self.default_asks.entry(key).or_default();
        *number += 1;
        *number
    }

    /// The state of a server was replaced: the level of the administrators that an open
    /// dialog shows is read again. A change of that level replaces the state (ADR 0792).
    pub(super) fn read_default_after_reload(
        &mut self,
        server_id: ServerId,
        cx: &mut Context<Self>,
    ) {
        if let Some((of, scope)) = self.selection.notify
            && of == server_id
        {
            self.read_default_of(server_id, scope, true, cx);
        }
    }

    /// Sets the level of the members of a workspace who chose none. Only an
    /// administrator of the server can do this.
    pub fn set_workspace_default(
        &mut self,
        server_id: ServerId,
        workspace_id: WorkspaceId,
        level: NotificationLevel,
        cx: &mut Context<Self>,
    ) {
        let Some(server) = self.client.server(server_id) else {
            return;
        };
        let key = (server_id, workspace_id.as_uuid());
        if !self.known.notify_busy.insert(key) {
            return;
        }
        self.known.notify_refused = None;
        let number = self.next_default_ask((server_id, workspace_id));
        let api = server.api();
        cx.spawn(async move |this, cx| {
            let set = api.set_notification_default(workspace_id, level).await;
            let _ = this.update(cx, |this, cx| {
                this.known.notify_busy.remove(&key);
                match set {
                    Ok(level) => {
                        if this.default_asks.get(&(server_id, workspace_id)) == Some(&number) {
                            this.known
                                .workspace_defaults
                                .insert((server_id, workspace_id), DefaultRead::Read(level));
                        }
                    }
                    Err(error) => {
                        this.known.notify_refused = Some((key.0, key.1, refusal(&error)));
                    }
                }
                this.changed(cx);
            });
        })
        .detach();
        self.changed(cx);
    }

    /// The settings of a scope as the server holds them, for a change. A mute whose end
    /// has passed is not sent again: the server refuses an end in the past.
    fn settings_for_change(
        &self,
        server: &ServerHandle,
        scope: SettingsScope,
    ) -> Option<NotificationSettings> {
        let model = server.live().view().model?;
        let mut settings = stored_settings(server.id(), scope, &model, &self.known);
        if settings
            .muted_until
            .is_some_and(|until| until <= OffsetDateTime::now_utc())
            || !settings.mute
        {
            settings.mute = false;
            settings.muted_until = None;
        }
        Some(settings)
    }

    /// Replaces the settings of a scope by the stored ones with a change. One change of a
    /// scope is on its way at a time. A refusal is told in the dialog of the scope.
    fn change_settings(
        &mut self,
        server_id: ServerId,
        scope: SettingsScope,
        cx: &mut Context<Self>,
        change: impl FnOnce(&mut NotificationSettings),
    ) -> Option<gpui::Task<Result<(), String>>> {
        let server = self.client.server(server_id)?;
        let mut settings = self.settings_for_change(&server, scope)?;
        change(&mut settings);
        let key = (server_id, scope.id());
        if !self.known.notify_busy.insert(key) {
            return None;
        }
        if self
            .known
            .notify_refused
            .as_ref()
            .is_some_and(|(server, scope_id, _)| (*server, *scope_id) == key)
        {
            self.known.notify_refused = None;
        }
        let api = server.api();
        let reloads = self.reloads.get(&server_id).copied().unwrap_or(0);
        let task = cx.spawn(async move |this, cx| {
            let set = api.set_notification_settings(scope, &settings).await;
            this.update(cx, |this, cx| {
                this.known.notify_busy.remove(&key);
                let result = match set {
                    Ok(taken) => {
                        // The state holds the change after the event of the server was
                        // followed. Until then the dialog shows what the server took. A
                        // state that was replaced since the request holds the change, or
                        // a later one of another device.
                        if this.reloads.get(&server_id).copied().unwrap_or(0) == reloads {
                            this.known.notify_sent.insert(key, taken);
                        }
                        Ok(())
                    }
                    Err(error) => Err(refusal(&error)),
                };
                this.changed(cx);
                result
            })
            .unwrap_or(Ok(()))
        });
        self.changed(cx);
        Some(task)
    }

    /// A change of the settings of one scope that its dialog or its menu asked for. A
    /// refusal is told in the dialog of the scope, or above the composer when that
    /// dialog is not open.
    fn change_scope(
        &mut self,
        server_id: ServerId,
        scope: SettingsScope,
        cx: &mut Context<Self>,
        change: impl FnOnce(&mut NotificationSettings),
    ) {
        let Some(changing) = self.change_settings(server_id, scope, cx, change) else {
            return;
        };
        let (navigation, owner) = (self.navigations, self.owner(server_id));
        cx.spawn(async move |this, cx| {
            if let Err(reason) = changing.await {
                let _ = this.update(cx, |this, cx| {
                    let in_dialog = this
                        .selection
                        .notify
                        .is_some_and(|(of, shown)| of == server_id && shown.id() == scope.id());
                    if in_dialog {
                        this.known.notify_refused = Some((server_id, scope.id(), reason));
                    } else if this.navigations == navigation && this.owner(server_id) == owner {
                        // The change came from the menu of a row: no dialog is open
                        // that could tell the refusal.
                        this.refuse(reason);
                    }
                    this.changed(cx);
                });
            }
        })
        .detach();
    }

    /// Sets the level of a scope. `None` takes the level of the scope above it.
    pub fn set_notify_level(
        &mut self,
        server_id: ServerId,
        scope: SettingsScope,
        level: Option<NotificationLevel>,
        cx: &mut Context<Self>,
    ) {
        self.change_scope(server_id, scope, cx, move |settings| settings.level = level);
    }

    /// Mutes a channel for a time, or ends its mute with `None` (ADR 0404).
    pub fn set_channel_mute(
        &mut self,
        server_id: ServerId,
        channel_id: ChannelId,
        length: Option<MuteFor>,
        cx: &mut Context<Self>,
    ) {
        let zone = self.known.zone;
        let scope = SettingsScope::Channel(channel_id);
        self.change_scope(server_id, scope, cx, move |settings| {
            settings.mute = length.is_some();
            settings.muted_until =
                length.and_then(|length| length.until(OffsetDateTime::now_utc(), zone));
        });
    }

    pub fn set_mention_switch(
        &mut self,
        server_id: ServerId,
        channel_id: ChannelId,
        switch: MentionSwitch,
        on: bool,
        cx: &mut Context<Self>,
    ) {
        let scope = SettingsScope::Channel(channel_id);
        self.change_scope(server_id, scope, cx, move |settings| match switch {
            MentionSwitch::Roles => settings.role_mentions = Some(on),
            MentionSwitch::Everyone => settings.everyone_mentions = Some(on),
        });
    }

    pub fn dismiss_notify_refusal(&mut self, cx: &mut Context<Self>) {
        self.known.notify_refused = None;
        self.changed(cx);
    }

    /// Mutes the direct conversation with an account for a time, or ends its mute with
    /// `None` (ADR 0515). A refusal is told above the composer.
    pub fn set_direct_mute(
        &mut self,
        server_id: ServerId,
        account_id: AccountId,
        length: Option<MuteFor>,
        cx: &mut Context<Self>,
    ) {
        let Some(server) = self.client.server(server_id) else {
            return;
        };
        let api = server.api();
        let until =
            length.and_then(|length| length.until(OffsetDateTime::now_utc(), self.known.zone));
        self.message_asks += 1;
        let (number, navigation) = (self.message_asks, self.navigations);
        let owner = self.owner(server_id);
        self.pending.message_refused = None;
        cx.spawn(async move |this, cx| {
            let asked = match length {
                Some(_) => api.mute_direct_conversation(account_id, until).await,
                None => api.unmute_direct_conversation(account_id).await,
            };
            let Err(error) = asked else {
                return;
            };
            let _ = this.update(cx, |this, cx| {
                if this.message_asks == number
                    && this.navigations == navigation
                    && this.owner(server_id) == owner
                {
                    this.refuse(refusal(&error));
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }

    // Quiet times.

    /// Changes the settings of the account on every server on which the device has a
    /// session (ADR 0404). Each server that does not take the change is told.
    fn change_quiet(
        &mut self,
        cx: &mut Context<Self>,
        change: impl Fn(&mut NotificationSettings) + Clone + 'static,
    ) {
        if self.known.quiet_busy {
            return;
        }
        // A server with a session whose state the device does not hold, or whose
        // settings of the account are being changed, does not get the change. It is
        // named with the reason.
        let mut left_out = Vec::new();
        let mut changes: Vec<(ServerId, gpui::Task<Result<(), String>>)> = Vec::new();
        for server in self.client.servers() {
            let server_id = server.id();
            if !matches!(server.access().state, AccessState::LoggedIn { .. }) {
                continue;
            }
            if server.live().view().model.is_none() {
                left_out.push((server_id, text::QUIET_NO_STATE.to_owned()));
                continue;
            }
            let change = change.clone();
            match self.change_settings(server_id, SettingsScope::Account, cx, change) {
                Some(task) => changes.push((server_id, task)),
                None => left_out.push((server_id, text::QUIET_OTHER_CHANGE.to_owned())),
            }
        }
        if changes.is_empty() {
            if !left_out.is_empty() {
                self.known.quiet_failed = left_out;
                self.changed(cx);
            }
            return;
        }
        self.known.quiet_busy = true;
        self.known.quiet_failed.clear();
        cx.spawn(async move |this, cx| {
            let mut failed = left_out;
            for (server_id, changing) in changes {
                if let Err(reason) = changing.await {
                    failed.push((server_id, reason));
                }
            }
            let _ = this.update(cx, |this, cx| {
                this.known.quiet_busy = false;
                this.known.quiet_failed = failed;
                this.changed(cx);
            });
        })
        .detach();
        self.changed(cx);
    }

    /// Turns "do not disturb" on or off (ADR 0404).
    pub fn set_do_not_disturb(&mut self, on: bool, cx: &mut Context<Self>) {
        self.change_quiet(cx, move |settings| settings.do_not_disturb = on);
    }

    /// Sets the daily quiet time as two minutes of a day on the clock of the time zone of
    /// the system, or ends it with `None`. The server keeps the name of the zone
    /// (ADR 0412).
    pub fn set_daily_quiet(&mut self, span: Option<(u16, u16)>, cx: &mut Context<Self>) {
        let daily = span.map(|(start_minute, end_minute)| DailyQuiet {
            start_minute,
            end_minute,
            time_zone: zones::system(),
        });
        self.change_quiet(cx, move |settings| settings.daily_quiet.clone_from(&daily));
    }

    /// A change of the settings that a server took is shown until the state of that
    /// server holds it, and no longer than until the state was replaced.
    pub(super) fn settle_sent_settings(&mut self, servers: &[ServerSnapshot]) {
        self.known
            .notify_sent
            .retain(|(server_id, scope_id), sent| {
                servers
                    .iter()
                    .find(|server| server.id == *server_id)
                    .and_then(|server| server.model.as_ref())
                    .is_some_and(|model| {
                        let held = model
                            .notification_settings
                            .iter()
                            .find(|held| held.scope_id == *scope_id)
                            .map(|held| &held.settings);
                        held != Some(sent)
                    })
            });
    }

    /// The state of a server was replaced: it holds every change that the server took
    /// before.
    pub(super) fn forget_sent_settings(&mut self, server_id: ServerId) {
        self.known.notify_sent.retain(|(of, _), _| *of != server_id);
    }

    /// Draws for this moment, and with this change of what the application holds beside
    /// the servers: the marks of the notices, the changes of settings that are on their
    /// way, the level of a workspace. The screen tool uses it.
    #[cfg(feature = "test-support")]
    pub fn show_notices_fixture(
        &mut self,
        now: OffsetDateTime,
        change: impl FnOnce(&mut super::view::Known),
        cx: &mut Context<Self>,
    ) {
        self.clock_fixture = Some(now);
        change(&mut self.known);
        self.changed(cx);
    }

    /// How often the state of a server was replaced since the start. A test waits for
    /// a reload with it.
    #[cfg(feature = "test-support")]
    pub fn reloads_of(&self, server_id: ServerId) -> u64 {
        self.reloads.get(&server_id).copied().unwrap_or(0)
    }

    // The notifications of the system.

    /// Uses this notifier for the notifications of the system.
    pub fn set_notifier(&mut self, notifier: Rc<dyn SystemNotifier>) {
        self.notifier = notifier;
    }

    /// What the notifier of a start without the notifications of the system was told.
    pub fn kept_notifications(&self) -> Kept {
        self.kept.kept()
    }

    /// The window became the active window of the system, or stopped being it.
    pub fn set_window_active(&mut self, active: bool, cx: &mut Context<Self>) {
        if self.window_active != active {
            self.window_active = active;
            self.changed(cx);
        }
    }

    /// Applies the rules of a system notification to every server after a change.
    pub(super) fn tell_alerts(
        &mut self,
        servers: &[ServerSnapshot],
        shown: Shown,
        cx: &mut Context<Self>,
    ) {
        let now = OffsetDateTime::now_utc();
        // A server that is gone, or whose session ended, takes its system notifications
        // with it. Its next state is a first state again.
        let gone: Vec<ServerId> = self
            .seen
            .keys()
            .filter(|server_id| {
                !servers.iter().any(|server| {
                    server.id == **server_id && super::view::shown_model(server).is_some()
                })
            })
            .copied()
            .collect();
        for server_id in gone {
            let Some(seen) = self.seen.remove(&server_id) else {
                continue;
            };
            for tag in seen.tags(server_id) {
                self.notifier.dismiss(&tag, cx);
            }
        }
        for server in servers {
            let Some(model) = super::view::shown_model(server) else {
                continue;
            };
            let seen = self.seen.entry(server.id).or_default();
            let told = alerts(
                seen,
                &AlertScene {
                    server_id: server.id,
                    server_name: &server.name,
                    several_servers: servers.len() > 1,
                    model,
                    known: &self.known,
                    now,
                    active: self.window_active,
                    shown,
                },
            );
            for tag in told.dismiss {
                self.notifier.dismiss(&tag, cx);
            }
            // What a press leads to is kept with what was seen of the server, for as
            // long as the notification is shown.
            for (alert, _) in told.show {
                self.notifier.show(&alert, cx);
            }
        }
    }

    /// The person pressed a notification of the system: the window shows what it was
    /// about, and its notice is read.
    pub fn alert_pressed(&mut self, tag: &str, cx: &mut Context<Self>) {
        let target = self
            .seen
            .iter()
            .find_map(|(server_id, seen)| seen.target(*server_id, tag));
        let Some(target) = target else {
            return;
        };
        match target {
            AlertTarget::Notice {
                server_id,
                notice_id,
                jump,
            } => {
                self.mark_notice_read(server_id, notice_id, cx);
                match jump {
                    Some(jump) => self.go_to(jump, cx),
                    None => self.select_server(server_id, cx),
                }
            }
            AlertTarget::Invitation(server_id) => self.select_server(server_id, cx),
        }
    }
}
