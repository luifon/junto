//! What the administration shows (ADR 0761), derived from the state of a server without
//! a side effect: the pages that the person may use, and the rows and the allowed actions
//! of the open page. The server decides every request; a page shows as possible what the
//! roles of the person allow.

pub mod overrides;
pub mod people;
pub mod records;
#[cfg(test)]
mod tests;

use junto_client_core::model::{AdminPart, Collection, LoadFailure, ServerModel, WorkspaceModel};
use junto_client_core::protocol::ids::{CategoryId, ChannelId, ServerId, WorkspaceId};
use junto_client_core::protocol::structure::{ArchivedChannel, ChannelKind};
use junto_client_core::protocol::{ServerPermission, WorkspacePermission};
use time::OffsetDateTime;

use super::{AdminOpen, AdminPage, AdminState, Ask, ChannelPage, Made};
use crate::state::view::{Known, Selection, ServerSnapshot, shown_model, shown_workspace, status};

/// A collection of the client core as a page shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Listed<T> {
    Loading,
    Failed(LoadFailure),
    Loaded(T),
}

impl<T> Listed<T> {
    /// A collection that no reader has open yet is on its way: the window opens it with
    /// the page.
    pub fn of<S>(collection: &Collection<S>, loaded: impl FnOnce(&S) -> T) -> Self {
        match collection {
            Collection::NotLoaded | Collection::Loading => Self::Loading,
            Collection::Failed(failure) => Self::Failed(*failure),
            Collection::Loaded(value) => Self::Loaded(loaded(value)),
        }
    }

    pub const fn loaded(&self) -> Option<&T> {
        match self {
            Self::Loaded(value) => Some(value),
            Self::Loading | Self::Failed(_) => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NavTitle {
    Server,
    /// The shown workspace, by its name.
    Workspace(String),
    Own,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NavGroup {
    pub title: NavTitle,
    pub pages: Vec<AdminPage>,
}

/// One workspace of the server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceLine {
    pub id: WorkspaceId,
    pub name: String,
    /// The workspace that an invite without a workspace grants. It is never deleted.
    pub is_default: bool,
    /// The workspace that the window shows.
    pub shown: bool,
    pub member: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspacesPage {
    pub rows: Vec<WorkspaceLine>,
    /// The person holds "create workspace".
    pub may_create: bool,
    /// The person is an administrator: rename and delete.
    pub may_manage: bool,
    pub is_owner: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CategoryLine {
    pub id: CategoryId,
    pub name: String,
    pub position: i32,
    pub first: bool,
    pub last: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelLine {
    pub id: ChannelId,
    pub name: String,
    pub kind: ChannelKind,
    pub category_id: Option<CategoryId>,
    pub position: i32,
    pub permanent: bool,
    /// The first or the last channel of its group: it moves in one direction only.
    pub first: bool,
    pub last: bool,
}

/// The channels of one category, or the channels without a category.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelGroup {
    pub category: Option<CategoryLine>,
    pub channels: Vec<ChannelLine>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelsPage {
    pub workspace_id: WorkspaceId,
    pub workspace_name: String,
    /// The roles of the person hold "manage channel" in the workspace.
    pub may_manage: bool,
    /// The channels without a category first, then each category in its order.
    pub groups: Vec<ChannelGroup>,
}

impl ChannelsPage {
    pub fn categories(&self) -> impl Iterator<Item = &CategoryLine> {
        self.groups
            .iter()
            .filter_map(|group| group.category.as_ref())
    }

    pub fn channel(&self, channel_id: ChannelId) -> Option<&ChannelLine> {
        self.groups
            .iter()
            .flat_map(|group| group.channels.iter())
            .find(|line| line.id == channel_id)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchivedLine {
    pub id: ChannelId,
    pub name: String,
    pub kind: ChannelKind,
    pub archived_at: OffsetDateTime,
    /// The server does not delete the channel before this time. Absent: it is not
    /// deleted by the server.
    pub deletes_at: Option<OffsetDateTime>,
    pub permanent: bool,
}

impl ArchivedLine {
    fn of(archived: &ArchivedChannel, known: &Known) -> Self {
        Self {
            id: archived.channel.id,
            name: archived.channel.name.clone(),
            kind: archived.channel.kind,
            archived_at: known.zone.local(archived.archived_at),
            deletes_at: archived.deletes_at.map(|at| known.zone.local(at)),
            permanent: archived.channel.permanent_messages,
        }
    }
}

/// The settings of a workspace that an administrator reads: days, or never.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkspaceDays {
    pub archive_period: Option<u32>,
    pub file_retention: Option<u32>,
    pub recording_retention: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchivePage {
    pub workspace_id: WorkspaceId,
    pub workspace_name: String,
    pub channels: Listed<Vec<ArchivedLine>>,
    /// The roles of the person hold "manage channel": restore and delete.
    pub may_manage: bool,
    pub is_owner: bool,
    /// Present for an administrator.
    pub settings: Option<Listed<WorkspaceDays>>,
    /// The retention of the files of direct messages, for an administrator.
    pub direct_retention: Option<Listed<Option<u32>>>,
}

/// Whether a channel is private: its "everyone" role is denied "view channel"
/// (ADR 0304).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Privacy {
    /// The overrides of the channel are on their way, or the person may not read them.
    Unknown,
    Open,
    Private,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VoiceSettings {
    pub user_limit: Option<u32>,
    pub hides_occupants: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverviewPage {
    pub name: String,
    pub category_id: Option<CategoryId>,
    pub categories: Vec<(CategoryId, String)>,
    pub permanent: bool,
    pub privacy: Privacy,
    pub voice: Option<VoiceSettings>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelArchivePage {
    /// Present for an archived channel: when it was archived, and the time before which
    /// the server does not delete it.
    pub archived: Option<(OffsetDateTime, Option<OffsetDateTime>)>,
    pub permanent: bool,
    pub is_owner: bool,
    /// The roles of the person hold "export channel".
    pub may_export: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChannelPageView {
    Overview(OverviewPage),
    Permissions(overrides::PermissionsPage),
    Archive(ChannelArchivePage),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelSettings {
    pub workspace_id: WorkspaceId,
    pub channel_id: ChannelId,
    pub name: String,
    pub kind: ChannelKind,
    /// An archived channel takes no change of its settings (ADR 0410).
    pub archived: bool,
    /// The roles of the person hold "manage channel" in the workspace.
    pub may_manage: bool,
    pub pages: Vec<ChannelPage>,
    pub selected: ChannelPage,
    pub page: ChannelPageView,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerPage {
    Workspaces(WorkspacesPage),
    Channels(ChannelsPage),
    Archive(ArchivePage),
    Members(people::MembersPage),
    People(people::PeoplePage),
    Invites(people::InvitesPage),
    JoinRequests(people::RequestsPage),
    Tokens(records::TokensPage),
    Exports(records::ExportsPage),
    AuditLog(records::AuditPage),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scope {
    Server {
        nav: Vec<NavGroup>,
        selected: AdminPage,
        page: ServerPage,
    },
    Channel(Box<ChannelSettings>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdminView {
    pub server_id: ServerId,
    pub server_name: String,
    pub scope: Scope,
    /// The request of the person that is on its way or was refused.
    pub ask: Option<Ask>,
    /// What the last request made and the person has not put away.
    pub made: Option<Made>,
}

impl AdminView {
    /// The state of the request `what`, when it is the one that the page tells about.
    pub fn asked(&self, what: super::AskKind) -> Option<&super::AskState> {
        self.ask
            .as_ref()
            .filter(|ask| ask.what == what)
            .map(|ask| &ask.state)
    }

    /// True while the request `what` is on its way.
    pub fn sending(&self, what: super::AskKind) -> bool {
        matches!(self.asked(what), Some(super::AskState::Sending))
    }

    /// Why the server refused the request `what`.
    pub fn refused(&self, what: super::AskKind) -> Option<&str> {
        match self.asked(what) {
            Some(super::AskState::Refused(reason)) => Some(reason),
            _ => None,
        }
    }
}

/// True for the server owner, an administrator, and a holder of the server permission.
pub fn holds_server(model: &ServerModel, permission: ServerPermission) -> bool {
    model.is_administrator() || model.account.server_permissions.contains(&permission)
}

/// True when the roles of the person give the permission in the workspace. The overrides
/// of a channel are not counted, and the server decides every request.
pub fn holds_workspace(
    model: &ServerModel,
    workspace: &WorkspaceModel,
    permission: WorkspacePermission,
) -> bool {
    model.is_administrator() || workspace.role_permissions().contains(&permission)
}

/// The pages that the person may use, in their groups. A page that the person may not
/// use is absent.
pub fn nav(model: &ServerModel, workspace: Option<&WorkspaceModel>) -> Vec<NavGroup> {
    let mut server = vec![AdminPage::Workspaces];
    for (page, permission) in [
        (AdminPage::People, ServerPermission::BanMembers),
        (AdminPage::Invites, ServerPermission::CreateInvites),
        (
            AdminPage::JoinRequests,
            ServerPermission::ApproveJoinRequests,
        ),
        // An administrator manages the tokens of every account.
        (AdminPage::Tokens, ServerPermission::Administrator),
    ] {
        if holds_server(model, permission) {
            server.push(page);
        }
    }
    let workspace_log = workspace.is_some_and(|workspace| {
        holds_workspace(model, workspace, WorkspacePermission::ViewAuditEntries)
    });
    if holds_server(model, ServerPermission::ViewServerAuditLog) || workspace_log {
        server.push(AdminPage::AuditLog);
    }
    let mut groups = vec![NavGroup {
        title: NavTitle::Server,
        pages: server,
    }];
    if let Some(workspace) = workspace {
        let mut pages = Vec::new();
        if holds_workspace(model, workspace, WorkspacePermission::ManageChannel) {
            pages.push(AdminPage::Channels);
        }
        pages.extend([AdminPage::Archive, AdminPage::Members]);
        groups.push(NavGroup {
            title: NavTitle::Workspace(workspace.workspace.name.clone()),
            pages,
        });
    }
    // Every account lists and revokes its own tokens, also after it lost the
    // administrator permission (ADR 0210).
    let mut own = Vec::new();
    if !model.is_administrator() {
        own.push(AdminPage::Tokens);
    }
    own.push(AdminPage::Exports);
    groups.push(NavGroup {
        title: NavTitle::Own,
        pages: own,
    });
    groups
}

/// The collections of the client core that a page needs.
pub fn needs(
    open: &AdminOpen,
    model: &ServerModel,
    workspace_id: Option<WorkspaceId>,
) -> Vec<AdminPart> {
    let administrator = model.is_administrator();
    match *open {
        AdminOpen::Server { page, .. } => match page {
            AdminPage::Workspaces | AdminPage::Channels | AdminPage::AuditLog => Vec::new(),
            AdminPage::People => {
                let mut parts = vec![AdminPart::Accounts];
                if administrator {
                    parts.push(AdminPart::ServerRoles);
                }
                parts
            }
            AdminPage::Invites => vec![AdminPart::Invites],
            AdminPage::JoinRequests => vec![AdminPart::JoinRequests],
            AdminPage::Tokens => vec![AdminPart::AccessTokens],
            AdminPage::Exports => {
                // An export of an archived channel is named by the archived list.
                let mut parts = vec![AdminPart::Exports];
                parts.extend(workspace_id.map(AdminPart::Archived));
                parts
            }
            AdminPage::Members => {
                if administrator {
                    vec![AdminPart::Accounts]
                } else {
                    Vec::new()
                }
            }
            AdminPage::Archive => {
                let mut parts = Vec::new();
                if let Some(workspace_id) = workspace_id {
                    parts.push(AdminPart::Archived(workspace_id));
                    if administrator {
                        parts.push(AdminPart::WorkspaceSettings(workspace_id));
                    }
                }
                if administrator {
                    parts.push(AdminPart::DirectRetention);
                }
                parts
            }
        },
        AdminOpen::Channel {
            workspace_id,
            channel_id,
            page,
            ..
        } => {
            // The settings of an archived channel find it in the archived list.
            let mut parts = vec![AdminPart::Archived(workspace_id)];
            let may_manage = model.workspace(workspace_id).is_some_and(|workspace| {
                holds_workspace(model, workspace, WorkspacePermission::ManageChannel)
            });
            if may_manage && matches!(page, ChannelPage::Overview | ChannelPage::Permissions) {
                parts.push(AdminPart::Overrides(channel_id));
            }
            parts
        }
    }
}

fn workspaces_page(model: &ServerModel, shown: Option<WorkspaceId>) -> WorkspacesPage {
    WorkspacesPage {
        rows: model
            .workspaces
            .iter()
            .map(|workspace| WorkspaceLine {
                id: workspace.workspace.id,
                name: workspace.workspace.name.clone(),
                is_default: workspace.workspace.is_default,
                shown: shown == Some(workspace.workspace.id),
                member: model
                    .account
                    .workspace_ids
                    .contains(&workspace.workspace.id),
            })
            .collect(),
        may_create: holds_server(model, ServerPermission::CreateWorkspace),
        may_manage: model.is_administrator(),
        is_owner: model.account.is_owner,
    }
}

fn channels_page(model: &ServerModel, workspace: &WorkspaceModel) -> ChannelsPage {
    let in_category = |category_id: Option<CategoryId>| {
        category_id.filter(|id| workspace.categories.iter().any(|held| held.id == *id))
    };
    let lines = |category_id: Option<CategoryId>| {
        let mut lines: Vec<ChannelLine> = workspace
            .channels
            .iter()
            .filter(|held| in_category(held.channel.category_id) == category_id)
            .map(|held| ChannelLine {
                id: held.channel.id,
                name: held.channel.name.clone(),
                kind: held.channel.kind,
                category_id,
                position: held.channel.position,
                permanent: held.channel.permanent_messages,
                first: false,
                last: false,
            })
            .collect();
        if let Some(first) = lines.first_mut() {
            first.first = true;
        }
        if let Some(last) = lines.last_mut() {
            last.last = true;
        }
        lines
    };
    let mut groups = vec![ChannelGroup {
        category: None,
        channels: lines(None),
    }];
    let count = workspace.categories.len();
    for (index, category) in workspace.categories.iter().enumerate() {
        groups.push(ChannelGroup {
            category: Some(CategoryLine {
                id: category.id,
                name: category.name.clone(),
                position: category.position,
                first: index == 0,
                last: index + 1 == count,
            }),
            channels: lines(Some(category.id)),
        });
    }
    ChannelsPage {
        workspace_id: workspace.workspace.id,
        workspace_name: workspace.workspace.name.clone(),
        may_manage: holds_workspace(model, workspace, WorkspacePermission::ManageChannel),
        groups,
    }
}

fn archive_page(model: &ServerModel, workspace: &WorkspaceModel, known: &Known) -> ArchivePage {
    let workspace_id = workspace.workspace.id;
    let administrator = model.is_administrator();
    ArchivePage {
        workspace_id,
        workspace_name: workspace.workspace.name.clone(),
        channels: Listed::of(model.admin.archived(workspace_id), |channels| {
            channels
                .iter()
                .map(|archived| ArchivedLine::of(archived, known))
                .collect()
        }),
        may_manage: holds_workspace(model, workspace, WorkspacePermission::ManageChannel),
        is_owner: model.account.is_owner,
        settings: administrator.then(|| {
            Listed::of(model.admin.workspace_settings(workspace_id), |settings| {
                WorkspaceDays {
                    archive_period: settings.archive_period.days,
                    file_retention: settings.file_retention.days,
                    recording_retention: settings.recording_retention.days,
                }
            })
        }),
        direct_retention: administrator
            .then(|| Listed::of(&model.admin.direct_retention, |retention| retention.days)),
    }
}

fn channel_settings(
    model: &ServerModel,
    workspace_id: WorkspaceId,
    channel_id: ChannelId,
    selected: ChannelPage,
    known: &Known,
) -> Option<ChannelSettings> {
    let workspace = model.workspace(workspace_id)?;
    let channel = model
        .readable_channel(channel_id)
        .filter(|channel| channel.workspace_id == workspace_id)?;
    let archived = model.admin.archived_channel(channel_id);
    let may_manage = holds_workspace(model, workspace, WorkspacePermission::ManageChannel);
    let mut pages = Vec::new();
    if may_manage {
        pages.extend([ChannelPage::Overview, ChannelPage::Permissions]);
    }
    pages.push(ChannelPage::Archive);
    let selected = if pages.contains(&selected) {
        selected
    } else {
        ChannelPage::Archive
    };
    let page = match selected {
        ChannelPage::Overview => {
            let everyone = workspace.roles.iter().find(|role| role.is_everyone);
            let privacy = match (model.admin.overrides(channel_id).loaded(), everyone) {
                (Some(overrides), Some(everyone)) => {
                    let denied = overrides.iter().any(|held| {
                        held.role_id == everyone.id
                            && held.deny.contains(&WorkspacePermission::ViewChannel)
                    });
                    if denied {
                        Privacy::Private
                    } else {
                        Privacy::Open
                    }
                }
                _ => Privacy::Unknown,
            };
            ChannelPageView::Overview(OverviewPage {
                name: channel.name.clone(),
                category_id: channel.category_id,
                categories: workspace
                    .categories
                    .iter()
                    .map(|held| (held.id, held.name.clone()))
                    .collect(),
                permanent: channel.permanent_messages,
                privacy,
                voice: (channel.kind == ChannelKind::Voice).then_some(VoiceSettings {
                    user_limit: channel.user_limit,
                    hides_occupants: channel.hides_occupants,
                }),
            })
        }
        ChannelPage::Permissions => {
            ChannelPageView::Permissions(overrides::derive(model, workspace, channel))
        }
        ChannelPage::Archive => ChannelPageView::Archive(ChannelArchivePage {
            archived: archived.map(|held| {
                (
                    known.zone.local(held.archived_at),
                    held.deletes_at.map(|at| known.zone.local(at)),
                )
            }),
            permanent: channel.permanent_messages,
            is_owner: model.account.is_owner,
            may_export: holds_workspace(model, workspace, WorkspacePermission::ExportChannel),
        }),
    };
    Some(ChannelSettings {
        workspace_id,
        channel_id,
        name: channel.name.clone(),
        kind: channel.kind,
        archived: archived.is_some(),
        may_manage,
        pages,
        selected,
        page,
    })
}

/// The state of a server for the account that is logged in on it, while the server
/// shows content.
pub(crate) fn shown(server: &ServerSnapshot) -> Option<&ServerModel> {
    shown_model(server).filter(|_| status(server).shows_content())
}

/// The administration as the window shows it. Absent when the server of the open page
/// shows no content, or when the channel of the open settings is gone.
pub fn derive(
    servers: &[ServerSnapshot],
    selection: &Selection,
    known: &Known,
    admin: &AdminState,
    open: AdminOpen,
) -> Option<AdminView> {
    let server_id = open.server_id();
    let server = servers.iter().find(|server| server.id == server_id)?;
    let model = shown(server)?;
    let workspace = shown_workspace(model, selection.workspaces.get(&server_id).copied());
    let scope = match open {
        AdminOpen::Channel {
            workspace_id,
            channel_id,
            page,
            ..
        } => Scope::Channel(Box::new(channel_settings(
            model,
            workspace_id,
            channel_id,
            page,
            known,
        )?)),
        AdminOpen::Server { page, .. } => {
            let nav = nav(model, workspace);
            // A page that the person may not use any more gives way to the first one.
            let selected = if nav.iter().any(|group| group.pages.contains(&page)) {
                page
            } else {
                AdminPage::Workspaces
            };
            let page = match (selected, workspace) {
                (AdminPage::Channels, Some(workspace)) => {
                    ServerPage::Channels(channels_page(model, workspace))
                }
                (AdminPage::Archive, Some(workspace)) => {
                    ServerPage::Archive(archive_page(model, workspace, known))
                }
                (AdminPage::Members, Some(workspace)) => {
                    ServerPage::Members(people::members(server_id, model, workspace, known))
                }
                (AdminPage::People, _) => ServerPage::People(people::people(model, known)),
                (AdminPage::Invites, _) => {
                    ServerPage::Invites(people::invites(server_id, model, known, admin))
                }
                (AdminPage::JoinRequests, _) => {
                    ServerPage::JoinRequests(people::requests(model, known))
                }
                (AdminPage::Tokens, _) => {
                    ServerPage::Tokens(records::tokens(server_id, model, known, admin))
                }
                (AdminPage::Exports, _) => {
                    ServerPage::Exports(records::exports(server_id, model, known, admin))
                }
                (AdminPage::AuditLog, _) => {
                    ServerPage::AuditLog(records::audit(server_id, model, workspace, known, admin))
                }
                (
                    AdminPage::Workspaces
                    | AdminPage::Channels
                    | AdminPage::Archive
                    | AdminPage::Members,
                    _,
                ) => ServerPage::Workspaces(workspaces_page(
                    model,
                    workspace.map(|workspace| workspace.workspace.id),
                )),
            };
            let selected = match &page {
                ServerPage::Workspaces(_) => AdminPage::Workspaces,
                _ => selected,
            };
            Scope::Server {
                nav,
                selected,
                page,
            }
        }
    };
    Some(AdminView {
        server_id,
        server_name: server.name.clone(),
        scope,
        ask: admin.ask.clone(),
        made: admin.made.clone(),
    })
}
