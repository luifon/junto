//! What the window shows, derived from the state of the client core (ADR 0730).
//!
//! [`derive`] is a function without side effects: it takes what the client core holds of
//! each saved server and what the person has selected, and returns plain data that the
//! drawing code turns into elements. The drawing code holds no rule about a server.

use std::collections::HashMap;
use std::sync::Arc;

use junto_client_core::flow::FlowError;
use junto_client_core::flow::access::{AccessState, ActionKind, Notice};
use junto_client_core::live::{StreamFault, StreamStatus};
use junto_client_core::model::{ChannelModel, Loadable, ServerModel, WorkspaceModel};
use junto_client_core::protocol::account::RecoveryNotice;
use junto_client_core::protocol::call::BlockReason;
use junto_client_core::protocol::ids::{AccountId, ChannelId, ServerId, WorkspaceId};
use junto_client_core::protocol::structure::ChannelKind;
use junto_client_core::version::Incompatibility;
use junto_client_core::{Reach, ServerHandle};

/// What the client core holds of one saved server at one moment.
#[derive(Debug, Clone)]
pub struct ServerSnapshot {
    pub id: ServerId,
    pub name: String,
    /// The address as the server book shows it: the origin without its scheme.
    pub address: String,
    /// The connection mode "always" (ADR 0107).
    pub keep_connected: bool,
    pub reach: Reach,
    pub access: AccessState,
    pub in_progress: Option<ActionKind>,
    pub error: Option<FlowError>,
    pub stream: StreamStatus,
    pub model: Option<Arc<ServerModel>>,
}

impl ServerSnapshot {
    pub fn of(server: &ServerHandle) -> Self {
        let entry = server.entry();
        let access = server.access();
        let live = server.live().view();
        Self {
            id: server.id(),
            name: entry.name,
            address: shown_address(entry.address.as_str()),
            keep_connected: entry.connection_mode
                == junto_client_core::book::ConnectionMode::Always,
            reach: server.reach(),
            access: access.state,
            in_progress: access.in_progress,
            error: access.error,
            stream: live.status,
            model: live.model,
        }
    }
}

/// The origin of a server without its scheme, as a person types and reads it.
pub fn shown_address(origin: &str) -> String {
    origin
        .strip_prefix("https://")
        .or_else(|| origin.strip_prefix("http://"))
        .unwrap_or(origin)
        .to_owned()
}

/// What the person has selected. A selection that the state no longer holds is replaced
/// by the first choice of its kind when the view is derived.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Selection {
    pub server: Option<ServerId>,
    pub workspaces: HashMap<ServerId, WorkspaceId>,
    pub channels: HashMap<WorkspaceId, ChannelId>,
}

/// The names of accounts that the members of a workspace do not give, and the workspaces
/// whose members could not be read.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Known {
    pub names: HashMap<(ServerId, AccountId), String>,
    pub members_failed: Vec<(ServerId, WorkspaceId)>,
    /// The name of the profile preset.
    pub preset_name: String,
}

/// What one saved server is to this device now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerStatus {
    /// Logged in, and the state follows the server.
    Ready,
    /// Logged in without a connection, by the mode "connect only when selected".
    Standby,
    /// The client logs in or reads the first state.
    Connecting,
    /// Logged in, and the connection is interrupted. The state is the last known one.
    Reconnecting,
    /// The device belongs to an account, the server answers, and the login did not succeed.
    LoggedOut,
    WaitingForApproval {
        name: String,
    },
    WaitingForLink {
        code: String,
    },
    NoAccount(Option<Notice>),
    Banned,
    Unreachable,
    Incompatible(Incompatibility),
    OtherServer,
}

impl ServerStatus {
    /// True when the lists of the server are shown.
    pub const fn shows_content(&self) -> bool {
        matches!(self, Self::Ready | Self::Reconnecting)
    }
}

/// The state of a server as the account of the session sees it. The event stream follows
/// the access flow with a delay (ADR 0718): for that time it can still hold the state of
/// the account before, or of a session that has ended. That state is not shown.
fn shown_model(server: &ServerSnapshot) -> Option<&ServerModel> {
    match &server.access {
        AccessState::LoggedIn { account_id, .. } => server
            .model
            .as_deref()
            .filter(|model| model.account.id == *account_id),
        _ => None,
    }
}

pub fn status(server: &ServerSnapshot) -> ServerStatus {
    match server.reach {
        Reach::Incompatible(reason) => return ServerStatus::Incompatible(reason),
        Reach::OtherServer { .. } => return ServerStatus::OtherServer,
        Reach::Unknown | Reach::Reachable { .. } | Reach::Unreachable => {}
    }
    match &server.access {
        AccessState::LoggedIn { .. } => match (shown_model(server), server.stream) {
            (Some(_), StreamStatus::Interrupted(_)) => ServerStatus::Reconnecting,
            (Some(_), _) => ServerStatus::Ready,
            (None, StreamStatus::Interrupted(StreamFault::SessionEnded(_))) => {
                ServerStatus::Connecting
            }
            (None, StreamStatus::Interrupted(_)) => ServerStatus::Unreachable,
            (None, StreamStatus::Off) => ServerStatus::Standby,
            (None, StreamStatus::Connecting | StreamStatus::Live) => ServerStatus::Connecting,
        },
        AccessState::LoggedOut { .. } => {
            if server.in_progress.is_some() || server.reach == Reach::Unknown {
                ServerStatus::Connecting
            } else if server.reach == Reach::Unreachable {
                ServerStatus::Unreachable
            } else {
                ServerStatus::LoggedOut
            }
        }
        AccessState::WaitingForApproval { name, .. } => {
            ServerStatus::WaitingForApproval { name: name.clone() }
        }
        AccessState::WaitingForLink { code, .. } => ServerStatus::WaitingForLink {
            code: code.expose().to_owned(),
        },
        AccessState::NoAccount { notice } => ServerStatus::NoAccount(*notice),
        AccessState::Banned { .. } => ServerStatus::Banned,
    }
}

/// One workspace of a server in the server book.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceRow {
    pub id: WorkspaceId,
    pub name: String,
    pub selected: bool,
}

/// One server of the server book.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerRow {
    pub id: ServerId,
    pub name: String,
    pub address: String,
    pub status: ServerStatus,
    pub selected: bool,
    /// The device has a recovery notice of this server that the person has not seen.
    pub has_recovery: bool,
    pub workspaces: Vec<WorkspaceRow>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SectionTitle {
    TextChannels,
    VoiceChannels,
    Category(String),
}

/// The microphone of a person in a voice channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Microphone {
    Open {
        speaking: bool,
    },
    /// The person closed it.
    Closed,
    /// A moderator or meeting mode closed it.
    Held(BlockReason),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Occupant {
    pub account_id: AccountId,
    /// Absent while the client has not read the name of the account.
    pub name: Option<String>,
    pub is_self: bool,
    pub microphone: Microphone,
    pub sharing: bool,
    pub hand_raised: bool,
    /// The person turned the sound off, or a moderator blocked it.
    pub sound_off: bool,
}

/// What a voice channel shows of its call.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct VoiceState {
    pub in_use: bool,
    /// The channel is in use and shows who is inside only to the people inside.
    pub hidden: bool,
    pub locked: bool,
    pub meeting: bool,
    pub recording: bool,
    pub user_limit: Option<u32>,
    pub full: bool,
    pub occupants: Vec<Occupant>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelRow {
    pub id: ChannelId,
    pub kind: ChannelKind,
    pub name: String,
    pub selected: bool,
    /// Present for a voice channel.
    pub voice: Option<VoiceState>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelSection {
    pub title: SectionTitle,
    pub rows: Vec<ChannelRow>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemberRow {
    pub account_id: AccountId,
    pub name: String,
    pub is_self: bool,
    /// The names of the roles of the member, without the "everyone" role.
    pub roles: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Members {
    NotLoaded,
    Loading,
    Failed,
    Loaded(Vec<MemberRow>),
}

/// The selected workspace of the selected server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceView {
    pub id: WorkspaceId,
    pub name: String,
    pub sections: Vec<ChannelSection>,
    /// The selected channel, as the main area shows it.
    pub channel: Option<ChannelRow>,
    pub members: Members,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Content {
    /// The lists of a workspace.
    Workspace(Box<WorkspaceView>),
    /// The account can see no workspace.
    NoWorkspace,
    /// The server has no lists to show; its status says why.
    State,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectedServer {
    pub id: ServerId,
    pub name: String,
    pub address: String,
    pub status: ServerStatus,
    pub keep_connected: bool,
    /// An action of the access flow runs.
    pub busy: bool,
    /// Why the last action of the access flow failed.
    pub error: Option<FlowError>,
    /// The recoveries that this device has not acknowledged, oldest first.
    pub recoveries: Vec<RecoveryNotice>,
    /// The key of this device belonged to another account at the last contact
    /// (ADR 0712).
    pub account_changed: bool,
    pub content: Content,
}

/// The person as the profile strip shows them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Person {
    /// The account on the selected server. Absent without one.
    pub account_id: Option<AccountId>,
    /// Empty when no name is known.
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellView {
    pub servers: Vec<ServerRow>,
    pub selected: Option<SelectedServer>,
    pub person: Person,
}

impl ShellView {
    pub fn workspace(&self) -> Option<&WorkspaceView> {
        match &self.selected.as_ref()?.content {
            Content::Workspace(workspace) => Some(workspace),
            Content::NoWorkspace | Content::State => None,
        }
    }
}

/// The selection as the view resolved it: what is shown now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Shown {
    pub server: Option<ServerId>,
    pub workspace: Option<WorkspaceId>,
    pub channel: Option<ChannelId>,
}

impl ShellView {
    pub fn shown(&self) -> Shown {
        let workspace = self.workspace();
        Shown {
            server: self.selected.as_ref().map(|server| server.id),
            workspace: workspace.map(|workspace| workspace.id),
            channel: workspace
                .and_then(|workspace| workspace.channel.as_ref())
                .map(|channel| channel.id),
        }
    }
}

/// The workspace of a server that is shown: the selected one when the state holds it, the
/// default workspace otherwise, and the first one when there is no default.
fn shown_workspace(model: &ServerModel, selected: Option<WorkspaceId>) -> Option<&WorkspaceModel> {
    selected
        .and_then(|id| model.workspace(id))
        .or_else(|| {
            model
                .workspaces
                .iter()
                .find(|workspace| workspace.workspace.is_default)
                .map(Arc::as_ref)
        })
        .or_else(|| model.workspaces.first().map(Arc::as_ref))
}

fn voice_state(
    server_id: ServerId,
    channel: &ChannelModel,
    workspace: &WorkspaceModel,
    model: &ServerModel,
    known: &Known,
) -> VoiceState {
    let own = model.account.id;
    let name_of = |account_id: AccountId| -> Option<String> {
        if account_id == own {
            return Some(model.account.name.clone());
        }
        workspace
            .members
            .loaded()
            .and_then(|members| {
                members
                    .iter()
                    .find(|member| member.account_id == account_id)
                    .map(|member| member.name.clone())
            })
            .or_else(|| known.names.get(&(server_id, account_id)).cloned())
    };
    let Some(call) = &channel.call else {
        return VoiceState {
            in_use: channel.in_use,
            hidden: channel.in_use && channel.channel.hides_occupants,
            user_limit: channel.channel.user_limit,
            ..VoiceState::default()
        };
    };
    let occupants: Vec<Occupant> = call
        .participants
        .iter()
        .map(|participant| Occupant {
            account_id: participant.account_id,
            name: name_of(participant.account_id),
            is_self: participant.account_id == own,
            microphone: match (&participant.microphone_block, participant.muted) {
                (Some(block), _) => Microphone::Held(block.reason),
                (None, true) => Microphone::Closed,
                (None, false) => Microphone::Open {
                    speaking: call.speaking.contains(&participant.account_id),
                },
            },
            sharing: participant.screen_share.is_some(),
            hand_raised: participant.hand_raised,
            sound_off: participant.deafened || participant.sound_block.is_some(),
        })
        .collect();
    let in_use = channel.in_use;
    VoiceState {
        in_use,
        hidden: false,
        // A lock can be in the state before the first person of a call is seen
        // (ADR 0719). It is shown only for a channel in use.
        locked: in_use && call.lock.is_some(),
        meeting: in_use && call.meeting.is_some(),
        recording: in_use && call.recording.is_some(),
        user_limit: channel.channel.user_limit,
        full: channel
            .channel
            .user_limit
            .is_some_and(|limit| occupants.len() >= limit as usize && limit > 0),
        occupants,
    }
}

fn channel_row(
    server_id: ServerId,
    channel: &ChannelModel,
    workspace: &WorkspaceModel,
    model: &ServerModel,
    known: &Known,
) -> ChannelRow {
    ChannelRow {
        id: channel.channel.id,
        kind: channel.channel.kind,
        name: channel.channel.name.clone(),
        selected: false,
        voice: (channel.channel.kind == ChannelKind::Voice)
            .then(|| voice_state(server_id, channel, workspace, model, known)),
    }
}

/// The channels of a workspace in the groups of the channel column: first the channels
/// without a category, by kind, then each category that holds a visible channel.
fn sections(
    server_id: ServerId,
    workspace: &WorkspaceModel,
    model: &ServerModel,
    known: &Known,
) -> Vec<ChannelSection> {
    let in_category = |channel: &ChannelModel| {
        channel.channel.category_id.filter(|id| {
            workspace
                .categories
                .iter()
                .any(|category| category.id == *id)
        })
    };
    let row = |channel: &ChannelModel| channel_row(server_id, channel, workspace, model, known);
    let mut made = Vec::new();
    for (kind, title) in [
        (ChannelKind::Text, SectionTitle::TextChannels),
        (ChannelKind::Voice, SectionTitle::VoiceChannels),
    ] {
        let rows: Vec<ChannelRow> = workspace
            .channels
            .iter()
            .filter(|channel| channel.channel.kind == kind && in_category(channel).is_none())
            .map(row)
            .collect();
        if !rows.is_empty() {
            made.push(ChannelSection { title, rows });
        }
    }
    for category in &workspace.categories {
        let rows: Vec<ChannelRow> = workspace
            .channels
            .iter()
            .filter(|channel| in_category(channel) == Some(category.id))
            .map(row)
            .collect();
        if !rows.is_empty() {
            made.push(ChannelSection {
                title: SectionTitle::Category(category.name.clone()),
                rows,
            });
        }
    }
    made
}

fn members(
    server_id: ServerId,
    workspace: &WorkspaceModel,
    model: &ServerModel,
    known: &Known,
) -> Members {
    match &workspace.members {
        Loadable::Loaded(members) => {
            let mut rows: Vec<MemberRow> = members
                .iter()
                .map(|member| MemberRow {
                    account_id: member.account_id,
                    name: member.name.clone(),
                    is_self: member.account_id == model.account.id,
                    roles: workspace
                        .roles
                        .iter()
                        .filter(|role| !role.is_everyone && member.role_ids.contains(&role.id))
                        .map(|role| role.name.clone())
                        .collect(),
                })
                .collect();
            rows.sort_by(|one, other| {
                one.name
                    .to_lowercase()
                    .cmp(&other.name.to_lowercase())
                    .then(one.account_id.cmp(&other.account_id))
            });
            Members::Loaded(rows)
        }
        Loadable::Loading => Members::Loading,
        Loadable::NotLoaded => {
            if known
                .members_failed
                .contains(&(server_id, workspace.workspace.id))
            {
                Members::Failed
            } else {
                Members::NotLoaded
            }
        }
    }
}

fn workspace_view(
    server_id: ServerId,
    workspace: &WorkspaceModel,
    model: &ServerModel,
    selection: &Selection,
    known: &Known,
) -> WorkspaceView {
    let mut sections = sections(server_id, workspace, model, known);
    let wanted = selection.channels.get(&workspace.workspace.id).copied();
    let rows = || sections.iter().flat_map(|section| section.rows.iter());
    let shown = wanted
        .filter(|id| rows().any(|row| row.id == *id))
        .or_else(|| {
            rows()
                .find(|row| row.kind == ChannelKind::Text)
                .map(|row| row.id)
        })
        .or_else(|| rows().next().map(|row| row.id));
    let mut channel = None;
    for row in sections
        .iter_mut()
        .flat_map(|section| section.rows.iter_mut())
    {
        if Some(row.id) == shown {
            row.selected = true;
            channel = Some(row.clone());
        }
    }
    WorkspaceView {
        id: workspace.workspace.id,
        name: workspace.workspace.name.clone(),
        sections,
        channel,
        members: members(server_id, workspace, model, known),
    }
}

/// The view of the whole window.
pub fn derive(servers: &[ServerSnapshot], selection: &Selection, known: &Known) -> ShellView {
    let selected_id = selection
        .server
        .filter(|id| servers.iter().any(|server| server.id == *id))
        .or_else(|| servers.first().map(|server| server.id));
    let rows = servers
        .iter()
        .map(|server| {
            let status = status(server);
            let selected = Some(server.id) == selected_id;
            let shown = shown_model(server).and_then(|model| {
                shown_workspace(model, selection.workspaces.get(&server.id).copied())
            });
            ServerRow {
                id: server.id,
                name: server.name.clone(),
                address: server.address.clone(),
                selected,
                has_recovery: matches!(
                    &server.access,
                    AccessState::LoggedIn { recoveries, .. } if !recoveries.is_empty()
                ),
                workspaces: shown_model(server)
                    .filter(|_| status.shows_content())
                    .map(|model| {
                        model
                            .workspaces
                            .iter()
                            .map(|workspace| WorkspaceRow {
                                id: workspace.workspace.id,
                                name: workspace.workspace.name.clone(),
                                selected: selected
                                    && shown.is_some_and(|shown| {
                                        shown.workspace.id == workspace.workspace.id
                                    }),
                            })
                            .collect()
                    })
                    .unwrap_or_default(),
                status,
            }
        })
        .collect();
    let server = servers.iter().find(|server| Some(server.id) == selected_id);
    let selected = server.map(|server| {
        let status = status(server);
        let content = match shown_model(server).filter(|_| status.shows_content()) {
            None => Content::State,
            Some(model) => {
                match shown_workspace(model, selection.workspaces.get(&server.id).copied()) {
                    Some(workspace) => Content::Workspace(Box::new(workspace_view(
                        server.id, workspace, model, selection, known,
                    ))),
                    None => Content::NoWorkspace,
                }
            }
        };
        SelectedServer {
            id: server.id,
            name: server.name.clone(),
            address: server.address.clone(),
            status,
            keep_connected: server.keep_connected,
            busy: server.in_progress.is_some(),
            error: server.error.clone(),
            recoveries: match &server.access {
                AccessState::LoggedIn { recoveries, .. } => recoveries.clone(),
                _ => Vec::new(),
            },
            account_changed: matches!(
                &server.access,
                AccessState::LoggedIn {
                    previous_account: Some(_),
                    ..
                }
            ),
            content,
        }
    });
    let account = server.and_then(shown_model).map(|model| Person {
        account_id: Some(model.account.id),
        name: model.account.name.clone(),
    });
    ShellView {
        servers: rows,
        selected,
        person: account.unwrap_or_else(|| Person {
            account_id: None,
            name: known.preset_name.clone(),
        }),
    }
}

#[cfg(test)]
mod tests;
