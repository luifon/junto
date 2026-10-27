//! The notices of a server, the notification settings of the account, and the rules of a
//! system notification (ADR 0742).
//!
//! Everything here is derived without side effects from the state of a server (ADR 0730).
//! A notice names a message and a kind; its place and its text come from the message that
//! the client core read for it (ADR 0722).

use std::collections::HashMap;

use junto_client_core::api::SettingsScope;
use junto_client_core::model::{AdminPart, CollectionState, Named, PlaceId, ServerModel};
use junto_client_core::protocol::account::Departure;
use junto_client_core::protocol::call::CallPlace;
use junto_client_core::protocol::discovery::{
    DailyQuiet, NotificationKind, NotificationLevel, NotificationSettings,
};
use junto_client_core::protocol::ids::{
    AccountId, CallInvitationId, ChannelId, EntryRequestId, JoinRequestId, MessageId, ServerId,
    WorkspaceId,
};
use junto_client_core::protocol::message::{Message, MessagePlace};
use junto_client_core::protocol::structure::ChannelKind;
use junto_media::audio::sounds::Sound;
use time::OffsetDateTime;
use uuid::Uuid;

use super::feed::{self, Names, Zone};
use super::receipts::Awaited;
use super::view::{Known, Shown};
use crate::{text, zones};

/// The characters of a message that a notice, a search result and a system notification
/// show.
pub const EXCERPT_CHARACTERS: usize = 160;

/// Where the window goes for a message: its place, and the thread of a reply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Jump {
    pub server_id: ServerId,
    /// The workspace of the channel. Absent for a direct conversation.
    pub workspace_id: Option<WorkspaceId>,
    /// The channel or the direct conversation of the message.
    pub place: PlaceId,
    /// The root of the thread, for a reply of a thread.
    pub thread_root: Option<MessageId>,
    pub message_id: MessageId,
    pub position: u64,
}

impl Jump {
    /// The jump to a message of a server.
    pub fn to(server_id: ServerId, message: &Message) -> Self {
        Self {
            server_id,
            workspace_id: match message.place {
                MessagePlace::Channel { workspace_id, .. } => Some(workspace_id),
                MessagePlace::Direct { .. } => None,
            },
            place: PlaceId::of(&message.place),
            thread_root: message.thread_root,
            message_id: message.id,
            position: message.position,
        }
    }
}

/// The place of a message as a row names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Whereabouts {
    Channel {
        name: String,
        kind: ChannelKind,
    },
    /// A direct conversation, by the name of the other person.
    Direct {
        name: Option<String>,
    },
    /// The state holds no such place.
    Unknown,
}

/// One notice of the account, as the list shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoticeRow {
    pub id: Uuid,
    pub kind: NotificationKind,
    pub read: bool,
    pub place: Whereabouts,
    /// The day of the message in words.
    pub day: String,
    /// The time of day of the message.
    pub time: String,
    pub author_id: Option<AccountId>,
    pub author: String,
    pub text: String,
    /// Where the row leads. Absent when the message could not be read.
    pub jump: Option<Jump>,
}

/// The notices of the shown server, as the side panel lists them.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NoticesPanel {
    pub rows: Vec<NoticeRow>,
    /// The notices of the state that the person has not read, also those that the list
    /// does not show while their messages are read.
    pub unread: usize,
    /// The server may hold notices older than the listed ones.
    pub more: bool,
    /// The messages of some notices are not read yet.
    pub reading: bool,
    /// The workspaces whose archived list could not be read: their notices of archived
    /// channels are not listed until a read succeeds.
    pub archive_failed: Vec<WorkspaceId>,
    /// "Mark all as read" runs.
    pub marking: bool,
    /// Why the last mark did not reach the server.
    pub failed: Option<String>,
}

/// The names of the accounts of a server that the client holds: the members of the
/// workspaces that were read, the names that were read alone, and the account itself.
pub fn account_names(
    server_id: ServerId,
    model: &ServerModel,
    known: &Known,
) -> HashMap<AccountId, String> {
    let mut names: HashMap<AccountId, String> = known.people.names(server_id).collect();
    for workspace in &model.workspaces {
        if let Some(members) = workspace.members.loaded() {
            names.extend(
                members
                    .iter()
                    .map(|member| (member.account_id, crate::text::label(&member.name))),
            );
        }
    }
    names.insert(model.account.id, crate::text::label(&model.account.name));
    names
}

/// The names that the text of a message of this place can mention.
fn names_of(
    place: &MessagePlace,
    model: &ServerModel,
    accounts: &HashMap<AccountId, String>,
) -> Names {
    let roles = match place {
        MessagePlace::Channel { workspace_id, .. } => model
            .workspace(*workspace_id)
            .map(|workspace| {
                workspace
                    .roles
                    .iter()
                    .map(|role| (role.id, crate::text::label(&role.name)))
                    .collect()
            })
            .unwrap_or_default(),
        MessagePlace::Direct { .. } => HashMap::new(),
    };
    Names {
        accounts: accounts.clone(),
        roles,
        departed: HashMap::new(),
    }
}

/// The place of a message as the state names it now.
pub fn whereabouts(
    place: &MessagePlace,
    model: &ServerModel,
    accounts: &HashMap<AccountId, String>,
) -> Whereabouts {
    match place {
        // A channel of the lists, or an archived channel that the state knows (ADR 0762).
        MessagePlace::Channel { channel_id, .. } => match model.readable_channel(*channel_id) {
            Some(channel) => Whereabouts::Channel {
                name: channel.name.clone(),
                kind: channel.kind,
            },
            None => Whereabouts::Unknown,
        },
        MessagePlace::Direct {
            conversation_id,
            low_account_id,
            high_account_id,
        } => {
            if !model
                .conversations
                .iter()
                .any(|held| held.id == *conversation_id)
            {
                return Whereabouts::Unknown;
            }
            let other = if *low_account_id == model.account.id {
                *high_account_id
            } else {
                *low_account_id
            };
            Whereabouts::Direct {
                name: accounts.get(&other).cloned(),
            }
        }
    }
}

/// The workspace whose archived list names a channel that is in no channel list: the
/// channel can be an archived one (ADR 0762). Absent for a channel of the lists and for
/// a direct conversation.
pub fn archived_list_for(place: &MessagePlace, model: &ServerModel) -> Option<WorkspaceId> {
    match place {
        MessagePlace::Channel {
            workspace_id,
            channel_id,
        } if model.channel(*channel_id).is_none() => Some(*workspace_id),
        _ => None,
    }
}

/// Where the archived list of a workspace stands for an entry whose channel is in no
/// channel list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchivedList {
    /// The list has not answered: the place of the entry is not known yet.
    Pending,
    /// The read of the list failed: the place of the entry is not known, and the panel
    /// says so with a retry.
    Failed,
    /// The list answered, or the state holds no such workspace: the place is what the
    /// state knows, and an entry of no known place is not listed.
    Settled,
}

/// The state of the archived list of a workspace. A workspace that the state does not
/// hold has no list to wait for: it was deleted, or the account left it.
pub fn archived_list(model: &ServerModel, workspace_id: WorkspaceId) -> ArchivedList {
    if model.workspace(workspace_id).is_none() {
        return ArchivedList::Settled;
    }
    match model.admin.state(AdminPart::Archived(workspace_id)) {
        CollectionState::NotLoaded | CollectionState::Loading => ArchivedList::Pending,
        CollectionState::Failed(_) => ArchivedList::Failed,
        CollectionState::Loaded => ArchivedList::Settled,
    }
}

/// The workspaces whose archived lists the notices of a server need: those of the notices
/// whose channel is in no channel list, in a workspace that the state holds.
pub fn archived_lists_of_notices(model: &ServerModel) -> Vec<WorkspaceId> {
    let mut wanted = Vec::new();
    for notice in &model.notices {
        if let Some(Named::Read(message)) = model.named(notice.message_id)
            && let Some(workspace_id) = archived_list_for(&message.place, model)
            && model.workspace(workspace_id).is_some()
            && !wanted.contains(&workspace_id)
        {
            wanted.push(workspace_id);
        }
    }
    wanted
}

/// The text of a message on one line: its words, or the name of its first file.
pub fn excerpt(message: &Message, names: &Names) -> String {
    let words = message
        .document
        .as_ref()
        .map(|document| feed::plain_text(document, names, EXCERPT_CHARACTERS))
        .unwrap_or_default();
    if !words.is_empty() {
        return words;
    }
    message
        .attachments
        .first()
        .map(|file| file.name.clone())
        .unwrap_or_default()
}

/// The author of a message, with the mark of a departed account (ADR 0121).
fn author_of(
    message: &Message,
    accounts: &HashMap<AccountId, String>,
    departed: &HashMap<AccountId, Departure>,
) -> String {
    let departure = message.author_id.and_then(|id| departed.get(&id).copied());
    message
        .author_name
        .clone()
        .map(|name| match departure {
            Some(departure) => text::departed_name(&name, departure),
            None => name,
        })
        .or_else(|| message.author_id.and_then(|id| accounts.get(&id).cloned()))
        .unwrap_or_else(|| text::UNKNOWN_PERSON.to_owned())
}

/// The notices of a server as the side panel lists them: the oldest first and the newest
/// last, as a message list orders its messages. A notice whose message the server no longer
/// has for the account is not listed, and a notice whose place the state does not hold is
/// not listed either. A notice of a channel that is in no channel list waits for the
/// archived list of its workspace, and so do the older notices: they are added before the
/// listed rows when the list answers. A list that failed is named in the panel, and its
/// notices are not listed.
pub fn notices_panel(server_id: ServerId, model: &ServerModel, known: &Known) -> NoticesPanel {
    let accounts = account_names(server_id, model, known);
    let departed = known.people.departures(server_id);
    let mut rows = Vec::with_capacity(model.notices.len());
    let mut reading = false;
    let mut placing = false;
    let mut archive_failed: Vec<WorkspaceId> = Vec::new();
    // The names for the mentions of a text, once for each workspace of the notices.
    let mut names: HashMap<Option<WorkspaceId>, Names> = HashMap::new();
    for notice in &model.notices {
        let row = match model.named(notice.message_id) {
            Some(Named::Read(message)) => {
                if let Some(workspace_id) = archived_list_for(&message.place, model) {
                    match archived_list(model, workspace_id) {
                        ArchivedList::Pending => {
                            reading = true;
                            placing = true;
                            break;
                        }
                        ArchivedList::Failed => {
                            if !archive_failed.contains(&workspace_id) {
                                archive_failed.push(workspace_id);
                            }
                            continue;
                        }
                        ArchivedList::Settled => {}
                    }
                }
                let place = whereabouts(&message.place, model, &accounts);
                if place == Whereabouts::Unknown {
                    continue;
                }
                let local = known.zone.local(message.created_at);
                NoticeRow {
                    id: notice.id,
                    kind: notice.kind,
                    read: notice.read_at.is_some(),
                    place,
                    day: text::day_or_today(local.date(), known.today),
                    time: format!("{:02}:{:02}", local.hour(), local.minute()),
                    author_id: message.author_id,
                    author: author_of(message, &accounts, &departed),
                    text: excerpt(
                        message,
                        names
                            .entry(match message.place {
                                MessagePlace::Channel { workspace_id, .. } => Some(workspace_id),
                                MessagePlace::Direct { .. } => None,
                            })
                            .or_insert_with(|| names_of(&message.place, model, &accounts)),
                    ),
                    jump: Some(Jump::to(server_id, message)),
                }
            }
            Some(Named::Gone) => continue,
            Some(Named::Failed) => {
                let local = known.zone.local(notice.created_at);
                NoticeRow {
                    id: notice.id,
                    kind: notice.kind,
                    read: notice.read_at.is_some(),
                    place: Whereabouts::Unknown,
                    day: text::day_or_today(local.date(), known.today),
                    time: format!("{:02}:{:02}", local.hour(), local.minute()),
                    author_id: None,
                    author: String::new(),
                    text: text::NOTICE_NOT_READ.to_owned(),
                    jump: None,
                }
            }
            None => {
                reading = true;
                continue;
            }
        };
        rows.push(row);
    }
    rows.reverse();
    NoticesPanel {
        // The notices of the state count, also those whose messages are being read: the
        // panel offers "Mark all as read" for them (ADR 0742).
        unread: unread_notices(model),
        rows,
        // The control after the rows is not drawn while rows wait to be added before it,
        // and not while no notice is held to continue from.
        more: model.notices_more && !placing && !model.notices.is_empty(),
        // A list without a notice that the server says goes on is read again from its
        // first page.
        reading: reading || (model.notices.is_empty() && model.notices_more),
        archive_failed,
        marking: known.marking_all.contains_key(&server_id),
        failed: known.notices_failed.get(&server_id).cloned(),
    }
}

/// How many notices of a server the person has not read. A notice whose message is gone
/// does not count.
pub fn unread_notices(model: &ServerModel) -> usize {
    model
        .unread_notices()
        .filter(|notice| !matches!(model.named(notice.message_id), Some(Named::Gone)))
        .count()
}

// Notification settings.

/// The level of a scope: what the account stored, and what holds without it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LevelView {
    /// Absent: the scope takes the level of the scope above it.
    pub stored: Option<NotificationLevel>,
    /// The level that holds while nothing is stored. Absent when the client cannot tell
    /// it: the server does not tell whether a workspace has a default of its
    /// administrators.
    pub inherited: Option<NotificationLevel>,
}

/// How long a mute lasts (ADR 0404).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MuteFor {
    OneHour,
    EightHours,
    /// Until the next day begins in the time zone of the person.
    UntilTomorrow,
    UntilTurnedOff,
}

impl MuteFor {
    pub const ALL: [Self; 4] = [
        Self::OneHour,
        Self::EightHours,
        Self::UntilTomorrow,
        Self::UntilTurnedOff,
    ];

    /// The end of a mute that starts at `now`. Absent: it lasts until it is turned off.
    pub fn until(self, now: OffsetDateTime, zone: Zone) -> Option<OffsetDateTime> {
        match self {
            Self::OneHour => Some(now + time::Duration::hours(1)),
            Self::EightHours => Some(now + time::Duration::hours(8)),
            Self::UntilTomorrow => {
                let local = zone.local(now);
                let midnight = local.replace_time(time::Time::MIDNIGHT) + time::Duration::days(1);
                Some(crate::moment::in_utc(midnight))
            }
            Self::UntilTurnedOff => None,
        }
    }
}

/// The mute of a place as the person set it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Mute {
    #[default]
    Off,
    /// Until this time, in words.
    Until(String),
    UntilTurnedOff,
}

impl Mute {
    pub const fn is_on(&self) -> bool {
        !matches!(self, Self::Off)
    }

    fn of(muted: bool, until: Option<OffsetDateTime>, known: &Known) -> Self {
        match (muted, until, known.clock) {
            (false, _, _) => Self::Off,
            (true, None, _) => Self::UntilTurnedOff,
            (true, Some(until), Some(now)) if until <= now => Self::Off,
            (true, Some(until), _) => {
                let local = known.zone.local(until);
                Self::Until(text::time_and_day(local, known.today))
            }
        }
    }

    /// The mute that the account stored for a channel.
    pub fn of_channel(channel_id: ChannelId, model: &ServerModel, known: &Known) -> Self {
        match model.notification_settings(SettingsScope::Channel(channel_id)) {
            Some(settings) => Self::of(settings.mute, settings.muted_until, known),
            None => Self::Off,
        }
    }

    /// The mute of the direct conversation with an account (ADR 0515).
    pub fn of_direct(account_id: AccountId, model: &ServerModel, known: &Known) -> Self {
        match model
            .direct_mutes
            .iter()
            .find(|mute| mute.account_id == account_id)
        {
            Some(mute) => Self::of(true, mute.until, known),
            None => Self::Off,
        }
    }
}

/// What the dialog of a channel shows beside its level.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelNotify {
    pub mute: Mute,
    /// A mention of a role of the person counts as a mention.
    pub role_mentions: bool,
    /// A mention of everyone counts as a mention.
    pub everyone_mentions: bool,
}

/// The level that the administrators set for the members of a workspace, as the state
/// holds it, or as a change that the server took and the state does not hold yet asked
/// for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkspaceDefault {
    /// The level, or none while the workspace sets no level (ADR 0412).
    pub level: Option<NotificationLevel>,
    /// The account administers the server and can set the level.
    pub may_set: bool,
    /// A change of the level is on its way, or waits for the state: the control takes no
    /// further choice.
    pub changing: bool,
}

/// A change of the level of a workspace that the state does not hold yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SentDefault {
    pub level: Option<NotificationLevel>,
    /// The receipt of the answer. Absent while the request is on its way.
    pub answered: Option<Awaited>,
}

/// A change of the notification settings of a scope that the server took and the state
/// does not hold yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SentSettings {
    pub settings: NotificationSettings,
    pub answered: Awaited,
}

/// The level of a workspace as the window shows it: the level of a change that waits for
/// the state, and otherwise the level of the state.
pub fn shown_default(
    server_id: ServerId,
    workspace: &junto_client_core::model::WorkspaceModel,
    known: &Known,
) -> Option<NotificationLevel> {
    known
        .defaults_sent
        .get(&(server_id, workspace.workspace.id))
        .map_or(workspace.notification_default, |sent| sent.level)
}

/// The notification settings of one scope, as their dialog shows them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotifyPanel {
    pub server_id: ServerId,
    pub scope: SettingsScope,
    /// The name of the server, of the workspace or of the channel.
    pub name: String,
    pub level: LevelView,
    pub hide_text: Option<bool>,
    /// Present for a channel.
    pub channel: Option<ChannelNotify>,
    /// Present for a workspace.
    pub workspace_default: Option<WorkspaceDefault>,
    /// A change is on its way to the server.
    pub busy: bool,
    /// Why the server did not take the last change.
    pub refused: Option<String>,
}

/// The settings that the account stored for a scope, with a change that the server took
/// and the state does not hold yet.
pub fn stored_settings(
    server_id: ServerId,
    scope: SettingsScope,
    model: &ServerModel,
    known: &Known,
) -> NotificationSettings {
    known
        .notify_sent
        .get(&(server_id, scope.id()))
        .map(|sent| sent.settings.clone())
        .or_else(|| model.notification_settings(scope).cloned())
        .unwrap_or_default()
}

/// The level of a channel for the person: its own, or the level that it inherits.
pub(super) fn channel_level(
    server_id: ServerId,
    channel_id: ChannelId,
    model: &ServerModel,
    known: &Known,
) -> NotificationLevel {
    stored_settings(server_id, SettingsScope::Channel(channel_id), model, known)
        .level
        .or_else(|| inherited_level(server_id, SettingsScope::Channel(channel_id), model, known))
        .unwrap_or(NotificationLevel::Mentions)
}

/// The level that holds for a scope while the account stored none for it (ADR 0409): the
/// level of the workspace for a channel, then the level of the administrators, then the
/// level of the account on the server, then "mentions only".
fn inherited_level(
    server_id: ServerId,
    scope: SettingsScope,
    model: &ServerModel,
    known: &Known,
) -> Option<NotificationLevel> {
    let stored = |scope| stored_settings(server_id, scope, model, known).level;
    let account = stored(SettingsScope::Account).unwrap_or(NotificationLevel::Mentions);
    let below_workspace = |workspace_id| {
        let workspace = model.workspace(workspace_id)?;
        Some(shown_default(server_id, workspace, known).unwrap_or(account))
    };
    match scope {
        SettingsScope::Account => Some(NotificationLevel::Mentions),
        SettingsScope::Workspace(workspace_id) => below_workspace(workspace_id),
        SettingsScope::Channel(channel_id) => {
            let workspace_id = model.channel(channel_id)?.channel.workspace_id;
            match stored(SettingsScope::Workspace(workspace_id)) {
                Some(level) => Some(level),
                None => below_workspace(workspace_id),
            }
        }
    }
}

/// The settings of a scope as their dialog shows them. Absent when the state does not
/// hold the workspace or the channel.
pub fn notify_panel(
    server_id: ServerId,
    server_name: &str,
    scope: SettingsScope,
    model: &ServerModel,
    known: &Known,
) -> Option<NotifyPanel> {
    let settings = stored_settings(server_id, scope, model, known);
    let (name, channel, workspace_default) = match scope {
        SettingsScope::Account => (server_name.to_owned(), None, None),
        SettingsScope::Workspace(workspace_id) => {
            let workspace = model.workspace(workspace_id)?;
            (
                workspace.workspace.name.clone(),
                None,
                Some(WorkspaceDefault {
                    level: shown_default(server_id, workspace, known),
                    may_set: model.is_administrator(),
                    changing: known.defaults_sent.contains_key(&(server_id, workspace_id)),
                }),
            )
        }
        SettingsScope::Channel(channel_id) => {
            let channel = model.channel(channel_id)?;
            (
                channel.channel.name.clone(),
                Some(ChannelNotify {
                    mute: Mute::of(settings.mute, settings.muted_until, known),
                    role_mentions: settings.role_mentions.unwrap_or(true),
                    everyone_mentions: settings.everyone_mentions.unwrap_or(true),
                }),
                None,
            )
        }
    };
    let key = (server_id, scope.id());
    Some(NotifyPanel {
        server_id,
        scope,
        name,
        level: LevelView {
            stored: settings.level,
            inherited: inherited_level(server_id, scope, model, known),
        },
        hide_text: (scope == SettingsScope::Account).then_some(settings.hide_text),
        channel,
        workspace_default,
        busy: known.notify_busy.contains(&key),
        refused: known
            .notify_refused
            .as_ref()
            .filter(|(server, scope_id, _)| (*server, *scope_id) == key)
            .map(|(_, _, reason)| reason.clone()),
    })
}

// Quiet times.

/// "Do not disturb" and the daily quiet time of the application. Each is data of the
/// account on one server (ADR 0409); the application shows and sets them for every server
/// on which the device has a session (ADR 0404).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct QuietView {
    /// Absent: the device has a session on no server.
    pub do_not_disturb: Option<Agreement<bool>>,
    pub daily: Option<Agreement<Option<DailyQuiet>>>,
    /// The system notifications of messages leave out their text (ADR 0742).
    pub hide_text: Option<Agreement<bool>>,
    /// System notifications and the sounds of messages wait while the person shares a
    /// screen or presents (ADR 0742).
    pub hold_while_presenting: Option<Agreement<bool>>,
    /// The account of the shown server is in its quiet time now.
    pub quiet_now: bool,
    pub busy: bool,
    /// The servers that did not take the last change, by name, with the reason.
    pub failed: Vec<(String, String)>,
}

/// A value that every server holds alike, or that differs between them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Agreement<T> {
    All(T),
    Differs,
}

fn agreement<T: PartialEq + Clone>(values: &[T]) -> Option<Agreement<T>> {
    let first = values.first()?;
    Some(if values.iter().all(|value| value == first) {
        Agreement::All(first.clone())
    } else {
        Agreement::Differs
    })
}

/// The quiet times as the settings show them. `servers` are the servers on which the
/// device has a session and whose state it holds; `names` are the names of every saved
/// server, for a server that did not take a change.
pub fn quiet_view(
    servers: &[(ServerId, &str, &ServerModel)],
    names: &[(ServerId, &str)],
    shown: Option<ServerId>,
    known: &Known,
) -> QuietView {
    let account = |server_id: ServerId, model: &ServerModel| {
        stored_settings(server_id, SettingsScope::Account, model, known)
    };
    let dnd: Vec<bool> = servers
        .iter()
        .map(|(server_id, _, model)| account(*server_id, model).do_not_disturb)
        .collect();
    let daily: Vec<Option<DailyQuiet>> = servers
        .iter()
        .map(|(server_id, _, model)| account(*server_id, model).daily_quiet)
        .collect();
    let hide_text: Vec<bool> = servers
        .iter()
        .map(|(server_id, _, model)| account(*server_id, model).hide_text)
        .collect();
    let hold: Vec<bool> = servers
        .iter()
        .map(|(server_id, _, model)| holds_while_presenting(&account(*server_id, model)))
        .collect();
    QuietView {
        do_not_disturb: agreement(&dnd),
        daily: agreement(&daily),
        hide_text: agreement(&hide_text),
        hold_while_presenting: agreement(&hold),
        quiet_now: known.clock.is_some_and(|now| {
            servers
                .iter()
                .any(|(server_id, _, model)| Some(*server_id) == shown && quiet_at(model, now))
        }),
        busy: known.quiet_busy,
        failed: known
            .quiet_failed
            .iter()
            .filter_map(|(server_id, reason)| {
                let (_, name) = names.iter().find(|(held, _)| held == server_id)?;
                Some(((*name).to_owned(), reason.clone()))
            })
            .collect(),
    }
}

/// True while the account of a server wants no alert at an instant. The clock of the zone
/// that the daily quiet time names decides, as on the server (ADR 0412, ADR 0798).
/// The account holds its notifications while the person shares a screen or presents; on
/// unless the account turned it off (ADR 0742).
pub fn holds_while_presenting(settings: &NotificationSettings) -> bool {
    settings.hold_while_presenting != Some(false)
}

pub(super) fn quiet_at(model: &ServerModel, at: OffsetDateTime) -> bool {
    model.is_quiet(|zone| zones::minute_in(zone, at))
}

/// The minutes of a day of a time that a person typed as `22:00` or `7:30`.
pub fn parse_clock(text: &str) -> Option<u16> {
    let (hours, minutes) = text.trim().split_once(':')?;
    let digits = |part: &str| part.bytes().all(|byte| byte.is_ascii_digit());
    if !(1..=2).contains(&hours.len()) || minutes.len() != 2 || !digits(hours) || !digits(minutes) {
        return None;
    }
    let (hours, minutes) = (hours.parse::<u16>().ok()?, minutes.parse::<u16>().ok()?);
    (hours < 24 && minutes < 60).then_some(hours * 60 + minutes)
}

/// The minutes of a day as a clock shows them.
pub fn clock_text(minutes: u16) -> String {
    format!("{:02}:{:02}", minutes / 60, minutes % 60)
}

// System notifications.

/// A notification of the operating system.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Alert {
    /// The name of the notification for the system. A later one with the same tag
    /// replaces it, and a press on it is told with the tag.
    pub tag: String,
    pub title: String,
    pub body: String,
}

/// How many system notifications one server shows at most within [`ALERT_SPAN`].
pub const MOST_ALERTS: usize = 3;
/// The time in which one server shows at most [`MOST_ALERTS`].
pub const ALERT_SPAN: time::Duration = time::Duration::seconds(10);
/// The longest title of a system notification in characters.
pub const ALERT_TITLE_CHARACTERS: usize = 120;
/// How many new notices wait for the read of their message at most.
const MOST_WAITING: usize = 32;
/// How many shown system notifications the application remembers, to take one away when
/// its notice was read elsewhere.
const MOST_SHOWN: usize = 64;

/// What the application remembers of the notices and the invitations of one server, to
/// tell which are new.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Seen {
    /// The first state of the server in this run was seen.
    started: bool,
    /// The newest notice that was seen, by its time and its identifier.
    newest: Option<(OffsetDateTime, Uuid)>,
    /// The new notices whose message is not read yet.
    waiting: Vec<Uuid>,
    /// The notices that have a system notification.
    shown: Vec<ShownAlert>,
    /// The invitations that were seen.
    invitations: Vec<CallInvitationId>,
    /// The join requests that were seen, from the first list of them that the window held
    /// while the person may decide them (ADR 0761).
    join_requests: Option<Vec<JoinRequestId>>,
    /// The requests of people outside to enter the call of the account that were seen.
    entry_requests: Vec<EntryRequestId>,
    /// "Do not disturb" was on in the state that was seen last.
    do_not_disturb: bool,
    /// When the last system notifications of this server were shown.
    recent: Vec<OffsetDateTime>,
    /// The system notifications of single events that are shown, with what a press on
    /// each leads to (ADR 0742).
    events: Vec<(String, AlertTarget)>,
    /// The recordings, transcripts and exports whose end was told.
    told: Vec<String>,
}

/// A system notification of a notice that is shown.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ShownAlert {
    notice_id: Uuid,
    tag: String,
    /// The place of the message, when the message was read. The notification is taken
    /// away with that place also while the state does not hold the message.
    place: Option<MessagePlace>,
    /// The way to the message, for a press on the notification.
    jump: Option<Jump>,
}

impl Seen {
    pub(super) fn forget_outcome(&mut self, key: &str) {
        self.told.retain(|held| held != key);
    }

    /// True the first time a key is told: an end of a recording, of a transcript or of an
    /// export is told once.
    pub(super) fn tell_once(&mut self, key: String) -> bool {
        if self.told.contains(&key) {
            return false;
        }
        if self.told.len() >= MOST_SHOWN {
            self.told.remove(0);
        }
        self.told.push(key);
        true
    }

    /// True while this server has shown fewer than [`MOST_ALERTS`] system notifications
    /// within [`ALERT_SPAN`].
    pub(super) fn room_for_alert(&mut self, now: OffsetDateTime) -> bool {
        self.recent
            .retain(|at| *at <= now && now - *at < ALERT_SPAN);
        self.recent.len() < MOST_ALERTS
    }

    /// A system notification of a single event is shown. Returns the tag of the one that
    /// the application forgets for it.
    pub(super) fn event_shown(
        &mut self,
        tag: String,
        target: AlertTarget,
        now: OffsetDateTime,
    ) -> Option<String> {
        self.recent.push(now);
        self.events.push((tag, target));
        (self.events.len() > MOST_SHOWN).then(|| self.events.remove(0).0)
    }

    /// What a press on a system notification of this server leads to, while the
    /// notification is shown.
    pub fn target(&self, server_id: ServerId, tag: &str) -> Option<AlertTarget> {
        if let Some((_, target)) = self.events.iter().find(|(shown, _)| shown == tag) {
            return Some(*target);
        }
        if let Some(shown) = self.shown.iter().find(|shown| shown.tag == tag) {
            return Some(AlertTarget::Notice {
                server_id,
                notice_id: shown.notice_id,
                jump: shown.jump,
            });
        }
        if self
            .entry_requests
            .iter()
            .any(|request_id| entry_request_tag(server_id, *request_id) == tag)
        {
            return Some(AlertTarget::Call(server_id));
        }
        if self
            .join_requests
            .iter()
            .flatten()
            .any(|request_id| join_request_tag(server_id, *request_id) == tag)
        {
            return Some(AlertTarget::JoinRequests(server_id));
        }
        self.invitations
            .iter()
            .any(|invitation_id| invitation_tag(server_id, *invitation_id) == tag)
            .then_some(AlertTarget::Invitation(server_id))
    }

    /// The tags of the system notifications of this server that can be on screen.
    pub fn tags(&self, server_id: ServerId) -> Vec<String> {
        let notices = self.shown.iter().map(|shown| shown.tag.clone());
        let invitations = self
            .invitations
            .iter()
            .map(|invitation_id| invitation_tag(server_id, *invitation_id));
        let requests = self
            .join_requests
            .iter()
            .flatten()
            .map(|request_id| join_request_tag(server_id, *request_id));
        let entries = self
            .entry_requests
            .iter()
            .map(|request_id| entry_request_tag(server_id, *request_id));
        let events = self.events.iter().map(|(tag, _)| tag.clone());
        notices
            .chain(invitations)
            .chain(requests)
            .chain(entries)
            .chain(events)
            .collect()
    }
}

/// What the rules of a system notification read beside the state of the server.
pub struct AlertScene<'a> {
    pub server_id: ServerId,
    pub server_name: &'a str,
    /// The device has more than one saved server: a notification names its server.
    pub several_servers: bool,
    pub model: &'a ServerModel,
    pub known: &'a Known,
    pub now: OffsetDateTime,
    /// The window is the active window of the system.
    pub active: bool,
    /// What the window shows.
    pub shown: Shown,
    /// The person shares a screen or presents in meeting mode, and the account holds its
    /// notifications meanwhile: nothing is shown and no sound of messages plays (ADR 0742).
    pub presenting: bool,
}

/// The system notifications to show and the tags to take away after a change, and the
/// invitations into a voice channel that the person got with it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Alerts {
    pub show: Vec<(Alert, AlertTarget)>,
    pub dismiss: Vec<String>,
    /// The invitations into a voice channel that were not seen before, whatever their
    /// system notification, outside the quiet times of the account. A direct-message call
    /// rings with its tone (ADR 0515).
    pub arrived: Vec<CallInvitationId>,
    /// The sounds of the change (ADR 0707): one for the new notices that pass the rules of
    /// a system notification, whatever the bound of the time span, the mention sound when
    /// one of them is for the person alone; and the request sound for a new join request.
    pub sounds: Vec<Sound>,
}

/// The sound of a new notice: a message for the person alone has a sound of its own.
fn notice_sound(kind: NotificationKind) -> Sound {
    match kind {
        NotificationKind::Mention | NotificationKind::Direct | NotificationKind::MissedCall => {
            Sound::Mention
        }
        NotificationKind::Message | NotificationKind::Thread => Sound::Message,
    }
}

/// What a press on a system notification leads to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlertTarget {
    /// A notice, with the way to its message when the message could be read.
    Notice {
        server_id: ServerId,
        notice_id: Uuid,
        jump: Option<Jump>,
    },
    /// The server of an invitation into a call.
    Invitation(ServerId),
    /// The page of the join requests of a server.
    JoinRequests(ServerId),
    /// The call of this device, on a server.
    Call(ServerId),
    /// A server, whatever its state.
    Server(ServerId),
    /// A channel of a workspace.
    Channel {
        server_id: ServerId,
        workspace_id: WorkspaceId,
        channel_id: ChannelId,
    },
    /// A workspace.
    Workspace {
        server_id: ServerId,
        workspace_id: WorkspaceId,
    },
    /// A page of the administration of a server.
    Admin(ServerId, super::admin::AdminPage),
    /// A page of the account on a server: its devices, its recoveries.
    Account(ServerId, super::account::AccountPage),
}

pub fn notice_tag(server_id: ServerId, notice_id: Uuid) -> String {
    format!("notice:{server_id}:{notice_id}")
}

pub fn invitation_tag(server_id: ServerId, invitation_id: CallInvitationId) -> String {
    format!("invitation:{server_id}:{invitation_id}")
}

pub fn join_request_tag(server_id: ServerId, request_id: JoinRequestId) -> String {
    format!("join-request:{server_id}:{request_id}")
}

pub fn entry_request_tag(server_id: ServerId, request_id: EntryRequestId) -> String {
    format!("entry-request:{server_id}:{request_id}")
}

/// A text of a server as a system notification takes it: one line without control
/// characters, of at most `most` characters.
pub(super) fn one_line(text: &str, most: usize) -> String {
    let mut line: String = text
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .take(most)
        .collect();
    if text.chars().nth(most).is_some() {
        line.pop();
        line.push('…');
    }
    line
}

/// True when the state of the server holds the place of a message.
fn place_held(place: &MessagePlace, model: &ServerModel) -> bool {
    match place {
        MessagePlace::Channel { channel_id, .. } => model.channel(*channel_id).is_some(),
        MessagePlace::Direct {
            conversation_id, ..
        } => model
            .conversations
            .iter()
            .any(|held| held.id == *conversation_id),
    }
}

/// True when the window shows the place of a message to a person who looks at it.
fn in_view(scene: &AlertScene<'_>, message: &Message) -> bool {
    if !scene.active || scene.shown.server != Some(scene.server_id) {
        return false;
    }
    let place = PlaceId::of(&message.place);
    match message.thread_root {
        Some(root_id) => {
            scene.shown.thread.map(|(_, root)| root) == Some(root_id)
                && scene.shown.place() == Some(place)
        }
        None => scene.shown.place() == Some(place),
    }
}

/// True when the person muted the place of a message.
fn muted(scene: &AlertScene<'_>, message: &Message) -> bool {
    let model = scene.model;
    match message.place {
        MessagePlace::Channel { channel_id, .. } => {
            model
                .message_state(PlaceId::Channel(channel_id))
                .is_some_and(|state| state.muted)
                || Mute::of_channel(channel_id, model, scene.known).is_on()
        }
        MessagePlace::Direct {
            low_account_id,
            high_account_id,
            ..
        } => {
            let other = if low_account_id == model.account.id {
                high_account_id
            } else {
                low_account_id
            };
            Mute::of_direct(other, model, scene.known).is_on()
        }
    }
}

/// The rules of a system notification (ADR 0742), applied to the state of one server
/// after a change.
///
/// A notice gets a system notification when it appeared after the first state of its
/// server in this run, is unread and newer than every notice seen before, the account is
/// not in its quiet time, the person did not mute its place, and an active window does not
/// show its place. A notice of a quiet time is not shown later: the server tells no notice
/// in a quiet time, so the client learns of those notices when the quiet time has ended.
/// The notices of the state in which "do not disturb" ended are not shown, and neither is
/// a notice that was made inside the daily quiet time. An invitation into a call gets a
/// system notification unless the account is quiet or the window is active, and so does a
/// join request that a person who may decide it did not see before, and a request of a
/// person outside to enter the call of the account. One server
/// shows at most [`MOST_ALERTS`] within [`ALERT_SPAN`]; what passes that bound is not
/// shown later.
pub fn alerts(seen: &mut Seen, scene: &AlertScene<'_>) -> Alerts {
    let model = scene.model;
    let mut alerts = Alerts::default();
    let key = |notice: &junto_client_core::protocol::discovery::Notification| {
        (notice.created_at, notice.id)
    };
    let newest = model.notices.iter().map(key).max();
    let invitations: Vec<CallInvitationId> = model
        .incoming_invitations()
        .map(|invitation| invitation.id)
        .collect();
    let do_not_disturb = model
        .notification_settings(SettingsScope::Account)
        .is_some_and(|settings| settings.do_not_disturb);
    // The account keeps the text of messages out of the notifications of the system.
    let hide_text = model
        .notification_settings(SettingsScope::Account)
        .is_some_and(|settings| settings.hide_text);
    // The notices of the first state of a server are not new. An invitation of that
    // state waits for an answer now, and follows its rule.
    let first = !seen.started;
    if first {
        seen.started = true;
        seen.newest = newest;
        seen.do_not_disturb = do_not_disturb;
    }
    let quiet = quiet_at(model, scene.now) || scene.presenting;
    let held_back = seen.do_not_disturb && !do_not_disturb;
    seen.do_not_disturb = do_not_disturb;
    seen.recent
        .retain(|at| *at <= scene.now && scene.now - *at < ALERT_SPAN);

    // A system notification is taken away when its notice was read or is gone, when its
    // message is gone or deleted, and when the state no longer holds the place of its
    // message. The place is the one of the message as the state holds it, or the one
    // that the message had when the notification was shown.
    seen.shown.retain(|shown| {
        let stays = model
            .notices
            .iter()
            .find(|notice| notice.id == shown.notice_id && notice.read_at.is_none())
            .is_some_and(|notice| {
                let place = match model.named(notice.message_id) {
                    Some(Named::Gone) => return false,
                    Some(Named::Read(message)) if message.deleted => return false,
                    Some(Named::Read(message)) => Some(&message.place),
                    Some(Named::Failed) | None => shown.place.as_ref(),
                };
                place.is_none_or(|place| place_held(place, model))
            });
        if !stays {
            alerts.dismiss.push(shown.tag.clone());
        }
        stays
    });

    // The names are needed only when something can be shown.
    let accounts = std::cell::OnceCell::new();
    let accounts = || accounts.get_or_init(|| account_names(scene.server_id, model, scene.known));
    let server = scene.several_servers.then_some(scene.server_name);

    // The invitations into a call (ADR 0404). They come before the notices of the same
    // change.
    for held in std::mem::take(&mut seen.invitations) {
        if invitations.contains(&held) {
            seen.invitations.push(held);
        } else {
            alerts.dismiss.push(invitation_tag(scene.server_id, held));
        }
    }
    for invitation in model.incoming_invitations() {
        if seen.invitations.contains(&invitation.id) {
            continue;
        }
        if seen.invitations.len() >= MOST_SHOWN {
            break;
        }
        seen.invitations.push(invitation.id);
        // No sound during "do not disturb" or the daily quiet time, as for the ring of a
        // direct-message call (ADR 0515).
        if matches!(invitation.place, CallPlace::Channel { .. }) && !quiet {
            alerts.arrived.push(invitation.id);
        }
        if quiet || scene.active || seen.recent.len() >= MOST_ALERTS {
            continue;
        }
        let by = accounts().get(&invitation.invited_by).map(String::as_str);
        let channel = invitation
            .place
            .channel_id()
            .and_then(|channel_id| model.channel(channel_id))
            .map(|channel| channel.channel.name.as_str());
        seen.recent.push(scene.now);
        alerts.show.push((
            Alert {
                tag: invitation_tag(scene.server_id, invitation.id),
                title: one_line(&text::invitation_alert(by, channel), ALERT_TITLE_CHARACTERS),
                body: one_line(server.unwrap_or_default(), EXCERPT_CHARACTERS),
            },
            AlertTarget::Invitation(scene.server_id),
        ));
    }

    join_request_alerts(seen, scene, quiet, &mut alerts);
    entry_request_alerts(seen, scene, quiet, first, &mut alerts, &accounts);

    if first {
        return alerts;
    }
    let mut candidates: Vec<Uuid> = std::mem::take(&mut seen.waiting);
    for notice in model.notices.iter().rev() {
        if notice.read_at.is_none()
            && seen.newest.is_none_or(|newest| key(notice) > newest)
            && !candidates.contains(&notice.id)
        {
            candidates.push(notice.id);
        }
    }
    seen.newest = seen.newest.max(newest);
    for notice_id in candidates {
        let Some(notice) = model
            .notices
            .iter()
            .find(|notice| notice.id == notice_id && notice.read_at.is_none())
        else {
            continue;
        };
        if quiet || held_back || quiet_at(model, notice.created_at) {
            continue;
        }
        let tag = notice_tag(scene.server_id, notice.id);
        let (title, body, jump, place) = match model.named(notice.message_id) {
            None => {
                if seen.waiting.len() < MOST_WAITING {
                    seen.waiting.push(notice_id);
                }
                continue;
            }
            Some(Named::Gone) => continue,
            Some(Named::Failed) => (
                scene.server_name.to_owned(),
                text::ALERT_WITHOUT_TEXT.to_owned(),
                None,
                None,
            ),
            Some(Named::Read(message)) => {
                if message.deleted || muted(scene, message) || in_view(scene, message) {
                    continue;
                }
                let place = whereabouts(&message.place, model, accounts());
                if place == Whereabouts::Unknown {
                    continue;
                }
                let author = author_of(
                    message,
                    accounts(),
                    &scene.known.people.departures(scene.server_id),
                );
                let body = if hide_text {
                    text::ALERT_TEXT_HIDDEN.to_owned()
                } else {
                    excerpt(message, &names_of(&message.place, model, accounts()))
                };
                (
                    text::alert_title(notice.kind, &author, &place, server),
                    body,
                    Some(Jump::to(scene.server_id, message)),
                    Some(message.place),
                )
            }
        };
        let sound = notice_sound(notice.kind);
        if !alerts.sounds.contains(&sound) {
            alerts.sounds.push(sound);
        }
        // A notice above the bound of the time span is in the list only.
        if seen.recent.len() >= MOST_ALERTS {
            continue;
        }
        seen.recent.push(scene.now);
        seen.shown.push(ShownAlert {
            notice_id: notice.id,
            tag: tag.clone(),
            place,
            jump,
        });
        alerts.show.push((
            Alert {
                tag,
                title: one_line(&title, ALERT_TITLE_CHARACTERS),
                body: one_line(&body, EXCERPT_CHARACTERS),
            },
            AlertTarget::Notice {
                server_id: scene.server_id,
                notice_id: notice.id,
                jump,
            },
        ));
    }
    // One sound for the notices of one change: the mention sound covers the others.
    if alerts.sounds.contains(&Sound::Mention) {
        alerts.sounds.retain(|sound| *sound != Sound::Message);
    }
    // A notice that still waits for its message when the bound of the time span is
    // reached gets no system notification later.
    if seen.recent.len() >= MOST_ALERTS {
        seen.waiting.clear();
    }
    // The application remembers a bounded number of shown notifications. One that it
    // forgets is taken away, because nothing would take it away later.
    if seen.shown.len() > MOST_SHOWN {
        let excess = seen.shown.len() - MOST_SHOWN;
        alerts
            .dismiss
            .extend(seen.shown.drain(..excess).map(|shown| shown.tag));
    }
    alerts
}

/// The requests of people outside to enter the call of the account (ADR 0514). The state
/// holds those that the account may answer. The requests of the first state are not new.
fn entry_request_alerts<'a>(
    seen: &mut Seen,
    scene: &AlertScene<'_>,
    quiet: bool,
    first: bool,
    alerts: &mut Alerts,
    accounts: &dyn Fn() -> &'a HashMap<AccountId, String>,
) {
    let model = scene.model;
    seen.entry_requests.retain(|request_id| {
        let stays = model
            .entry_requests
            .iter()
            .any(|request| request.id == *request_id);
        if !stays {
            alerts
                .dismiss
                .push(entry_request_tag(scene.server_id, *request_id));
        }
        stays
    });
    for request in &model.entry_requests {
        if seen.entry_requests.contains(&request.id) || seen.entry_requests.len() >= MOST_SHOWN {
            continue;
        }
        seen.entry_requests.push(request.id);
        if first || quiet || scene.active || seen.recent.len() >= MOST_ALERTS {
            continue;
        }
        let name = accounts().get(&request.account_id).map(String::as_str);
        let channel = request
            .place
            .channel_id()
            .and_then(|channel_id| model.channel(channel_id))
            .map(|channel| channel.channel.name.as_str());
        seen.recent.push(scene.now);
        alerts.show.push((
            Alert {
                tag: entry_request_tag(scene.server_id, request.id),
                title: one_line(
                    &text::entry_request_alert(name, channel),
                    ALERT_TITLE_CHARACTERS,
                ),
                body: if scene.several_servers {
                    one_line(scene.server_name, EXCERPT_CHARACTERS)
                } else {
                    String::new()
                },
            },
            AlertTarget::Call(scene.server_id),
        ));
    }
}

/// The join requests that wait for a decision of the person (ADR 0761). The first list
/// that the window holds is not new: those requests waited before. A request that leaves
/// the list, decided or withdrawn, takes its notification away. While the list is not
/// held, what was seen stays, so that a request that arrives meanwhile is new when the
/// list is read again.
fn join_request_alerts(seen: &mut Seen, scene: &AlertScene<'_>, quiet: bool, alerts: &mut Alerts) {
    let model = scene.model;
    if !super::admin::view::holds_server(
        model,
        junto_client_core::protocol::ServerPermission::ApproveJoinRequests,
    ) {
        for request_id in seen.join_requests.take().into_iter().flatten() {
            alerts
                .dismiss
                .push(join_request_tag(scene.server_id, request_id));
        }
        return;
    }
    let Some(requests) = model.admin.join_requests.loaded() else {
        return;
    };
    let Some(held) = seen.join_requests.as_mut() else {
        seen.join_requests = Some(requests.iter().map(|request| request.id).collect());
        return;
    };
    held.retain(|request_id| {
        let stays = requests.iter().any(|request| request.id == *request_id);
        if !stays {
            alerts
                .dismiss
                .push(join_request_tag(scene.server_id, *request_id));
        }
        stays
    });
    for request in requests {
        if held.contains(&request.id) || held.len() >= MOST_SHOWN {
            continue;
        }
        held.push(request.id);
        // The request sound plays also while a window is active, as the sound of a request
        // to enter a call does (ADR 0707).
        if !quiet && !alerts.sounds.contains(&Sound::Request) {
            alerts.sounds.push(Sound::Request);
        }
        if quiet || scene.active || seen.recent.len() >= MOST_ALERTS {
            continue;
        }
        seen.recent.push(scene.now);
        alerts.show.push((
            Alert {
                tag: join_request_tag(scene.server_id, request.id),
                title: one_line(
                    &text::join_request_alert(&request.name),
                    ALERT_TITLE_CHARACTERS,
                ),
                body: one_line(scene.server_name, EXCERPT_CHARACTERS),
            },
            AlertTarget::JoinRequests(scene.server_id),
        ));
    }
}

#[cfg(test)]
mod tests;
