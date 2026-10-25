//! What the administration shows (ADR 0761), derived from the state of a server without
//! a side effect: the pages that the person may use, and the rows and the allowed actions
//! of the open page. The server decides every request; a page shows as possible what the
//! server stated that the person holds (ADR 0310).

pub mod glossary;
pub mod notifications;
pub mod overrides;
pub mod people;
pub mod recordings;
pub mod records;
pub mod roles;
pub mod server;
#[cfg(test)]
mod tests;

use junto_client_core::model::{AdminPart, Collection, LoadFailure, ServerModel, WorkspaceModel};
use junto_client_core::protocol::ids::{CategoryId, ChannelId, ServerId, WorkspaceId};
use junto_client_core::protocol::structure::{
    ArchivedChannel, ChannelKind, ChannelMove, ChannelPlace, MOST_CHANNEL_MOVES,
};
use junto_client_core::protocol::{ServerPermission, WorkspacePermission};
use time::OffsetDateTime;

use super::{AdminOpen, AdminPage, AdminState, Ask, ChannelPage, Made, RoleLevel};
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
    /// The pages that hold work waiting for the person, with its count.
    pub waiting: Vec<(AdminPage, usize)>,
}

impl NavGroup {
    /// The count of the work that waits on a page for the person; 0 when none waits.
    pub fn waiting(&self, page: AdminPage) -> usize {
        self.waiting
            .iter()
            .find(|(held, _)| *held == page)
            .map_or(0, |(_, count)| *count)
    }
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
    /// The group of the row: its category, or none when the workspace holds no such
    /// category.
    pub category_id: Option<CategoryId>,
    pub position: i32,
    /// The category and the position that the state holds, which a move states as the
    /// place where it read the channel.
    pub place: ChannelPlace,
    pub permanent: bool,
    /// The first or the last channel of its group: it moves in one direction only.
    pub first: bool,
    pub last: bool,
    /// The person holds "manage channel" in this channel (ADR 0790).
    pub may_manage: bool,
    /// The move by one place is possible: the channel is not the first or the last one,
    /// the person holds "manage channel" in every channel whose position the move
    /// changes, and the move names at most the channels that one request names.
    pub moves_up: Result<(), Unplanned>,
    pub moves_down: Result<(), Unplanned>,
}

/// Why the page does not offer a place to a channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unplanned {
    /// The channel is at the place already, or the page does not hold the channel or the
    /// group.
    Nothing,
    /// Each way of the move changes a channel in which the person does not hold "manage
    /// channel" (ADR 0761).
    NotManaged,
    /// Each way of the move that the person may send names more channels than one
    /// request names (ADR 0304).
    TooMany,
    /// A refused move is drawn: its order is not the order of the state.
    RefusalShown,
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
    /// The person holds "manage channel" in the workspace.
    pub may_manage: bool,
    /// The channels without a category first, then each category in its order.
    pub groups: Vec<ChannelGroup>,
    /// A move that the server refused, drawn at its place until the person acts again:
    /// the moved channel, at whose row the refusal is stated, and the reason. While it is
    /// drawn, the page offers no move (ADR 0730).
    pub refusal: Option<(ChannelId, String)>,
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

    /// The channels of one group, in their order.
    pub fn group(&self, category_id: Option<CategoryId>) -> Option<&ChannelGroup> {
        self.groups
            .iter()
            .find(|group| group.category.as_ref().map(|category| category.id) == category_id)
    }

    /// The moves that put the channel at the place `to` of the channels of `category_id`,
    /// computed from this page alone, the moved channel first: the way with the fewest
    /// changed channels (see [`placings`](crate::state::admin::structure::placings)) in
    /// which the person holds "manage channel" in every channel whose position or category
    /// changes (ADR 0761). The channels next to the place, as the page read them, follow as
    /// moves that only check where they are, so that the server refuses a move computed from
    /// an order that changed (ADR 0304); such a check needs "view channel" alone. The moves
    /// name at most the channels that one request names. The drag, Up, Down and the places
    /// of "Move to" all use it, so they offer the same destinations.
    pub fn plan(
        &self,
        channel_id: ChannelId,
        category_id: Option<CategoryId>,
        to: usize,
    ) -> Result<Vec<ChannelMove>, Unplanned> {
        if self.refusal.is_some() {
            return Err(Unplanned::RefusalShown);
        }
        self.channel(channel_id).ok_or(Unplanned::Nothing)?;
        let group = self.group(category_id).ok_or(Unplanned::Nothing)?;
        let order: Vec<(ChannelId, i32)> = group
            .channels
            .iter()
            .map(|line| (line.id, line.position))
            .collect();
        let others: Vec<&ChannelLine> = group
            .channels
            .iter()
            .filter(|line| line.id != channel_id)
            .collect();
        let at = to.min(others.len());
        let neighbours: Vec<&ChannelLine> = at
            .checked_sub(1)
            .and_then(|before| others.get(before))
            .into_iter()
            .chain(others.get(at))
            .copied()
            .collect();
        let ways = crate::state::admin::structure::placings(&order, channel_id, to);
        let mut refused = Unplanned::Nothing;
        for changes in ways {
            let lines: Option<Vec<&ChannelLine>> = changes
                .iter()
                .map(|(id, _)| self.channel(*id).filter(|line| line.may_manage))
                .collect();
            let Some(lines) = lines else {
                if refused == Unplanned::Nothing {
                    refused = Unplanned::NotManaged;
                }
                continue;
            };
            let mut moves: Vec<ChannelMove> = changes
                .iter()
                .zip(lines)
                .map(|((id, position), line)| ChannelMove {
                    channel_id: *id,
                    from: line.place,
                    to: ChannelPlace {
                        category_id,
                        position: *position,
                    },
                })
                .collect();
            if let Some(at) = moves.iter().position(|held| held.channel_id == channel_id) {
                let own = moves.remove(at);
                moves.insert(0, own);
            }
            for line in &neighbours {
                if moves.iter().all(|held| held.channel_id != line.id) {
                    moves.push(ChannelMove {
                        channel_id: line.id,
                        from: line.place,
                        to: line.place,
                    });
                }
            }
            if moves.len() > MOST_CHANNEL_MOVES {
                refused = Unplanned::TooMany;
                continue;
            }
            return Ok(moves);
        }
        Err(refused)
    }

    /// True when a drop of the channel at the place `to` of the channels of `category_id`
    /// is offered.
    pub fn takes(&self, channel_id: ChannelId, category_id: Option<CategoryId>, to: usize) -> bool {
        self.plan(channel_id, category_id, to).is_ok()
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
    /// The person holds "manage channel" in the workspace: restore and delete. The list of
    /// one archived channel is read only for the channel whose settings are open.
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
    /// The person holds "export channel" in the channel.
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
    /// The person holds "manage channel" in the channel.
    pub may_manage: bool,
    /// The state holds what the person may do in the channel. False for an archived
    /// channel until its own list is read (ADR 0790).
    pub permissions_read: bool,
    pub pages: Vec<ChannelPage>,
    pub selected: ChannelPage,
    pub page: ChannelPageView,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerPage {
    Workspaces(WorkspacesPage),
    /// The server itself: its logo.
    Server(server::ServerSettingsPage),
    Channels(ChannelsPage),
    Archive(ArchivePage),
    Members(people::MembersPage),
    Notifications(notifications::NotificationsPage),
    People(people::PeoplePage),
    Invites(people::InvitesPage),
    JoinRequests(people::RequestsPage),
    Tokens(records::TokensPage),
    Exports(records::ExportsPage),
    /// In a box: the page with its filters is larger than every other page.
    AuditLog(Box<records::AuditPage>),
    Recordings(recordings::RecordingsPage),
    Glossary(glossary::GlossaryPage),
    /// The server roles, or the roles of the shown workspace.
    Roles(roles::RolesPage),
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
    /// The requests of the person that are on their way or were refused: one for each kind
    /// of request, in the order in which they were made.
    pub asks: Vec<Ask>,
    /// What the requests of the page made and the person has not put away, one for each
    /// place of the page.
    pub made: Vec<Made>,
    /// The permissions of roles that the person changed and whose change the state does
    /// not hold yet, with the asked state: their switches show it and take no further
    /// change (ADR 0791, ADR 0311).
    pub changing_roles: Vec<(super::RoleRef, super::RolePermission, bool)>,
}

impl AdminView {
    /// The state that a waiting change of a permission of a role asked for.
    pub fn permission_asked(
        &self,
        role: super::RoleRef,
        permission: super::RolePermission,
    ) -> Option<bool> {
        self.changing_roles
            .iter()
            .find(|(of, held, _)| *of == role && *held == permission)
            .map(|(_, _, on)| *on)
    }

    /// The state of the request `what`, when the page tells one.
    pub fn asked(&self, what: super::AskKind) -> Option<&super::AskState> {
        self.asks
            .iter()
            .find(|ask| ask.what == what)
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

/// True when the person holds the permission in the workspace, as the server stated it
/// (ADR 0310). A channel can have another list: a control of one channel asks for that
/// one.
pub fn holds_workspace(workspace: &WorkspaceModel, permission: WorkspacePermission) -> bool {
    workspace.holds(permission)
}

/// The join requests that wait for a decision, when the person may decide them and the
/// window holds their list. The window holds it for every server whose account may decide
/// them, so that the server book and the navigation show the count (ADR 0761).
pub fn waiting_join_requests(model: &ServerModel) -> usize {
    if !holds_server(model, ServerPermission::ApproveJoinRequests) {
        return 0;
    }
    model.admin.join_requests.loaded().map_or(0, Vec::len)
}

/// The count of the work that waits for the person on a page of the administration.
pub fn waiting_on(page: AdminPage, model: &ServerModel) -> usize {
    match page {
        AdminPage::JoinRequests => waiting_join_requests(model),
        _ => 0,
    }
}

/// The pages that the person may use, in their groups. A page that the person may not
/// use is absent.
pub fn nav(model: &ServerModel, workspace: Option<&WorkspaceModel>) -> Vec<NavGroup> {
    let mut server = vec![AdminPage::Workspaces];
    // Only an administrator sets or removes the logo of the server (ADR 0101).
    if model.is_administrator() {
        server.push(AdminPage::Server);
    }
    for (page, permission) in [
        (AdminPage::People, ServerPermission::BanMembers),
        (AdminPage::Invites, ServerPermission::CreateInvites),
        (
            AdminPage::JoinRequests,
            ServerPermission::ApproveJoinRequests,
        ),
        // An administrator manages the tokens of every account, and the server roles.
        (AdminPage::Tokens, ServerPermission::Administrator),
        (AdminPage::ServerRoles, ServerPermission::Administrator),
    ] {
        if holds_server(model, permission) {
            server.push(page);
        }
    }
    let workspace_log = workspace
        .is_some_and(|workspace| holds_workspace(workspace, WorkspacePermission::ViewAuditEntries));
    if holds_server(model, ServerPermission::ViewServerAuditLog) || workspace_log {
        server.push(AdminPage::AuditLog);
    }
    let mut groups = vec![NavGroup {
        title: NavTitle::Server,
        waiting: waiting_of(&server, model),
        pages: server,
    }];
    if let Some(workspace) = workspace {
        let mut pages = Vec::new();
        if holds_workspace(workspace, WorkspacePermission::ManageChannel) {
            pages.push(AdminPage::Channels);
        }
        if holds_workspace(workspace, WorkspacePermission::ManageRoles) {
            pages.push(AdminPage::Roles);
        }
        // Every member lists the recordings: the server answers with what the account
        // may see.
        pages.extend([AdminPage::Archive, AdminPage::Members]);
        // Only an administrator sets the notification level of a workspace (ADR 0412).
        if model.is_administrator() {
            pages.push(AdminPage::Notifications);
        }
        pages.push(AdminPage::Recordings);
        // Every member reads the glossary; "manage glossary" changes it (ADR 0603).
        pages.push(AdminPage::Glossary);
        groups.push(NavGroup {
            title: NavTitle::Workspace(workspace.workspace.name.clone()),
            waiting: waiting_of(&pages, model),
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
        waiting: waiting_of(&own, model),
        pages: own,
    });
    groups
}

fn waiting_of(pages: &[AdminPage], model: &ServerModel) -> Vec<(AdminPage, usize)> {
    pages
        .iter()
        .map(|page| (*page, waiting_on(*page, model)))
        .filter(|(_, count)| *count > 0)
        .collect()
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
            // The snapshot carries the notification level of a workspace.
            AdminPage::Workspaces | AdminPage::Channels | AdminPage::Notifications => Vec::new(),
            // The logo is no collection: its image is read from the public route. The page
            // is an administrator's.
            AdminPage::Server => {
                if administrator {
                    vec![AdminPart::AttachmentLimits]
                } else {
                    Vec::new()
                }
            }
            // The filters by person offer the accounts and the server roles of the server
            // to a person who may list them. A row names the access token of its action by
            // the list of the tokens: every token for an administrator, the own tokens for
            // every other person (ADR 0210).
            AdminPage::AuditLog => {
                let mut parts = vec![AdminPart::AccessTokens];
                if holds_server(model, ServerPermission::BanMembers) {
                    parts.extend([AdminPart::Accounts, AdminPart::ServerRoles]);
                }
                parts
            }
            // The server gives the server roles to every person who has the page: their
            // order decides who can be removed and banned (ADR 0309).
            AdminPage::People => vec![AdminPart::Accounts, AdminPart::ServerRoles],
            AdminPage::ServerRoles => vec![AdminPart::ServerRoles],
            // An administrator stands in a workspace at the position of a server role. The
            // server states what the person may do with each role (ADR 0311).
            AdminPage::Roles => {
                let mut parts: Vec<AdminPart> = workspace_id
                    .map(AdminPart::RoleActions)
                    .into_iter()
                    .collect();
                if administrator && !model.account.is_owner {
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
            // A recording of an archived channel is named by the archived list.
            AdminPage::Recordings => workspace_id.map(AdminPart::Archived).into_iter().collect(),
            AdminPage::Glossary => workspace_id.map(AdminPart::Glossary).into_iter().collect(),
            AdminPage::Members => {
                let mut parts = Vec::new();
                if administrator {
                    parts.push(AdminPart::Accounts);
                    if !model.account.is_owner {
                        parts.push(AdminPart::ServerRoles);
                    }
                }
                // The roles of a member are given and taken by the statement of the
                // server for each role (ADR 0311).
                let manages_roles = workspace_id
                    .and_then(|workspace_id| model.workspace(workspace_id))
                    .is_some_and(|workspace| workspace.holds(WorkspacePermission::ManageRoles));
                if manages_roles {
                    parts.extend(workspace_id.map(AdminPart::RoleActions));
                }
                parts
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
            // The settings of an archived channel find it in the archived list. The
            // snapshot states no permissions for it: they are read by themselves
            // (ADR 0310).
            let mut parts = vec![AdminPart::Archived(workspace_id)];
            if model.channel(channel_id).is_none() {
                parts.push(AdminPart::ChannelPermissions(channel_id));
            }
            let may_manage = model.holds_in(channel_id, WorkspacePermission::ManageChannel);
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

/// The page of the channels of a workspace. `waiting` holds the moves of the channels of
/// the workspace that the person sent and that the state does not hold yet, the oldest
/// first: the page shows each such channel at the place that its newest move asks for,
/// and states that place as the one where the channel is, so the order changes at the
/// action of the person and not at the answer or at the events (ADR 0730).
fn channels_page(
    workspace: &WorkspaceModel,
    waiting: &[ChannelMove],
    refusal: Option<(ChannelId, String)>,
) -> ChannelsPage {
    let in_category = |category_id: Option<CategoryId>| {
        category_id.filter(|id| workspace.categories.iter().any(|held| held.id == *id))
    };
    let place_of = |held: &junto_client_core::model::ChannelModel| {
        waiting
            .iter()
            .rev()
            .find(|moved| moved.channel_id == held.channel.id)
            .map_or(
                ChannelPlace {
                    category_id: held.channel.category_id,
                    position: held.channel.position,
                },
                |moved| moved.to,
            )
    };
    let lines = |category_id: Option<CategoryId>| {
        let mut lines: Vec<ChannelLine> = workspace
            .channels
            .iter()
            .map(|held| (held, place_of(held)))
            .filter(|(_, place)| in_category(place.category_id) == category_id)
            .map(|(held, place)| ChannelLine {
                id: held.channel.id,
                name: held.channel.name.clone(),
                kind: held.channel.kind,
                category_id,
                position: place.position,
                place,
                permanent: held.channel.permanent_messages,
                first: false,
                last: false,
                may_manage: workspace.holds_in(held.channel.id, WorkspacePermission::ManageChannel),
                moves_up: Err(Unplanned::Nothing),
                moves_down: Err(Unplanned::Nothing),
            })
            .collect();
        // The order of the state: by position, and by identifier for equal positions.
        lines.sort_by_key(|line| (line.position, line.id.as_uuid().as_u128()));
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
    let mut page = ChannelsPage {
        workspace_id: workspace.workspace.id,
        workspace_name: workspace.workspace.name.clone(),
        may_manage: holds_workspace(workspace, WorkspacePermission::ManageChannel),
        groups,
        refusal,
    };
    // A move by one place is the move to the place before or after, with the same rule of
    // positions and permissions as a drag.
    type Offered = (Result<(), Unplanned>, Result<(), Unplanned>);
    let offered: Vec<Vec<Offered>> = page
        .groups
        .iter()
        .map(|group| {
            let category_id = group.category.as_ref().map(|category| category.id);
            let offered =
                |line: &ChannelLine, to: usize| page.plan(line.id, category_id, to).map(|_| ());
            group
                .channels
                .iter()
                .enumerate()
                .map(|(index, line)| {
                    let up = index
                        .checked_sub(1)
                        .map_or(Err(Unplanned::Nothing), |to| offered(line, to));
                    (up, offered(line, index + 1))
                })
                .collect()
        })
        .collect();
    for (group, flags) in page.groups.iter_mut().zip(offered) {
        for (line, (up, down)) in group.channels.iter_mut().zip(flags) {
            line.moves_up = up;
            line.moves_down = down;
        }
    }
    page
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
        may_manage: holds_workspace(workspace, WorkspacePermission::ManageChannel),
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
    let may_manage = model.holds_in(channel_id, WorkspacePermission::ManageChannel);
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
            may_export: model.holds_in(channel_id, WorkspacePermission::ExportChannel),
        }),
    };
    Some(ChannelSettings {
        workspace_id,
        channel_id,
        name: channel.name.clone(),
        kind: channel.kind,
        archived: archived.is_some(),
        may_manage,
        permissions_read: model.permissions_in(channel_id).is_some(),
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
                (AdminPage::Channels, Some(workspace)) => ServerPage::Channels(channels_page(
                    workspace,
                    &admin.channel_moves.of(server_id, workspace.workspace.id),
                    admin
                        .channel_moves
                        .refusal(server_id, workspace.workspace.id),
                )),
                (AdminPage::Archive, Some(workspace)) => {
                    ServerPage::Archive(archive_page(model, workspace, known))
                }
                (AdminPage::Members, Some(workspace)) => {
                    ServerPage::Members(people::members(server_id, model, workspace, known))
                }
                (AdminPage::Notifications, Some(workspace)) => {
                    ServerPage::Notifications(notifications::derive(server_id, workspace, known))
                }
                (AdminPage::Recordings, Some(workspace)) => ServerPage::Recordings(
                    recordings::recordings(server_id, model, workspace, known, admin),
                ),
                (AdminPage::Glossary, Some(workspace)) => {
                    ServerPage::Glossary(glossary::derive(server_id, model, workspace, admin))
                }
                (AdminPage::Server, _) => {
                    ServerPage::Server(server::settings(server, model, known))
                }
                (AdminPage::People, _) => ServerPage::People(people::people(model, known)),
                (AdminPage::Roles, Some(workspace)) => {
                    match roles::derive(model, RoleLevel::Workspace(workspace.workspace.id)) {
                        Some(page) => ServerPage::Roles(page),
                        None => ServerPage::Workspaces(workspaces_page(model, None)),
                    }
                }
                (AdminPage::ServerRoles, _) => match roles::derive(model, RoleLevel::Server) {
                    Some(page) => ServerPage::Roles(page),
                    None => ServerPage::Workspaces(workspaces_page(model, None)),
                },
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
                (AdminPage::AuditLog, _) => ServerPage::AuditLog(Box::new(records::audit(
                    server_id, model, workspace, known, admin,
                ))),
                (
                    AdminPage::Workspaces
                    | AdminPage::Channels
                    | AdminPage::Roles
                    | AdminPage::Archive
                    | AdminPage::Members
                    | AdminPage::Notifications
                    | AdminPage::Recordings
                    | AdminPage::Glossary,
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
        asks: admin.asks(),
        made: admin.made.clone(),
        changing_roles: admin.role_changes.of(server_id),
    })
}
