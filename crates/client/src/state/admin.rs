//! What the application does for the administration of a server (ADR 0761): which page is
//! open, which collections of the client core that page needs, and the one request of
//! the person that is on its way or was refused.

pub(crate) mod glossary;
mod moves;
mod notifications;
mod overrides;
mod people;
pub(crate) mod recordings;
pub mod records;
pub mod roles;
mod server;
pub(crate) mod structure;
pub mod view;

use std::collections::{HashMap, HashSet};
use std::future::Future;

use gpui::{Context, Task};
use junto_client_core::api::{ApiError, ServerApi};
use junto_client_core::live::LoadError;
use junto_client_core::model::AdminPart;
use junto_client_core::protocol::ids::{
    AccessTokenId, AccountId, CategoryId, ChannelId, ExportId, InviteId, JoinRequestId, ServerId,
    WorkspaceId, WorkspaceRoleId,
};

use junto_client_core::protocol::secret::{AccessToken, InviteCode};
use junto_client_core::protocol::{ServerPermission, WorkspacePermission};

pub use recordings::RecordingList;
pub use records::{AuditFilter, AuditLog, AuditPeriod, AuditScope, AuditTarget, ExportFiles};
pub use roles::{RoleLevel, RolePermission, RoleRef};
pub use view::AdminView;
use view::Scope;

use super::AppState;
use crate::text;

/// A page of the administration of a server.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AdminPage {
    Workspaces,
    /// The server itself: its logo (ADR 0101) and the limits of the files of messages
    /// (ADR 0408).
    Server,
    People,
    Invites,
    JoinRequests,
    Tokens,
    AuditLog,
    /// The server roles in their order (ADR 0309).
    ServerRoles,
    /// The categories and channels of the shown workspace.
    Channels,
    /// The roles of the shown workspace in their order.
    Roles,
    /// The archived channels, the archive period and the retention of the shown workspace.
    Archive,
    /// The members of the shown workspace.
    Members,
    /// The notification level that the shown workspace sets for its members (ADR 0412).
    Notifications,
    /// The recordings of the shown workspace that the account can see (ADR 0784).
    Recordings,
    /// The glossary of the shown workspace (ADR 0616). Every member reads it.
    Glossary,
    /// The exports of the person.
    Exports,
}

/// A page of the settings of one channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ChannelPage {
    Overview,
    Permissions,
    /// Archive, restore, export and deletion.
    Archive,
}

/// What the administration shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdminOpen {
    Server {
        server_id: ServerId,
        page: AdminPage,
    },
    Channel {
        server_id: ServerId,
        workspace_id: WorkspaceId,
        channel_id: ChannelId,
        page: ChannelPage,
    },
}

impl AdminOpen {
    pub const fn server_id(&self) -> ServerId {
        match self {
            Self::Server { server_id, .. } | Self::Channel { server_id, .. } => *server_id,
        }
    }
}

/// What a request of the administration asks for. A page shows the state of a request at
/// the control that made it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AskKind {
    CreateWorkspace,
    RenameWorkspace(WorkspaceId),
    DeleteWorkspace(WorkspaceId),
    RedeemInvite,
    CreateCategory,
    UpdateCategory(CategoryId),
    DeleteCategory(CategoryId),
    CreateChannel,
    UpdateChannel(ChannelId),
    /// A move of a channel, with the other channels whose position it changes, in one
    /// request (ADR 0761).
    MoveChannels(ChannelId),
    DeleteChannel(ChannelId),
    ArchiveChannel(ChannelId),
    RestoreChannel(ChannelId),
    CallSettings(ChannelId),
    SetOverride(ChannelId, WorkspaceRoleId),
    ArchivePeriod,
    /// The notification level of a workspace is set or removed.
    NotificationDefault(WorkspaceId),
    FileRetention,
    RecordingRetention,
    DirectRetention,
    AddMember(AccountId),
    /// The removal of a member from the shown workspace (ADR 0106).
    RemoveMember(AccountId),
    RemoveAccount(AccountId),
    BanAccount(AccountId),
    UnbanAccount(AccountId),
    /// The server owner gives a new device key access to an account (ADR 0104).
    RecoverAccount(AccountId),
    CreateInvite,
    RevokeInvite(InviteId),
    ApproveRequest(JoinRequestId),
    DenyRequest(JoinRequestId),
    CreateToken,
    RevokeToken(AccessTokenId),
    RequestExport,
    RemoveExport(ExportId),
    CreateRole(RoleLevel),
    /// A new name or other permissions of a role.
    UpdateRole(RoleRef),
    DeleteRole(RoleRef),
    MoveRole(RoleRef),
    /// A role is given to an account or taken from it.
    AssignRole(RoleRef, AccountId),
    /// A file is read and sent as the logo of the server (ADR 0101).
    SetLogo,
    RemoveLogo,
    /// A term is added at the end of the glossary of the shown workspace (ADR 0616).
    AddTerm,
    /// The term at this position of the glossary is changed, removed, or moved by one
    /// place.
    ChangeTerm(u32),
    RemoveTerm(u32),
    MoveTerm(u32),
    /// The largest file and the number of files of a message (ADR 0408).
    AttachmentLimits,
}

/// The place of a page that tells the state of a request: one control, or one row or block
/// that tells the requests of several controls with one sentence (ADR 0761).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AskPlace {
    /// A control that tells only the requests of its own kind.
    Own(AskKind),
    Workspace(WorkspaceId),
    Category(CategoryId),
    /// The settings of a channel: its overview, its call settings and its overrides.
    ChannelSettings(ChannelId),
    ArchivedChannel(ChannelId),
    Account(AccountId),
    /// The roles of an account, open under its row.
    AccountRoles(AccountId),
    JoinRequest(JoinRequestId),
    Role(RoleRef),
    Logo,
    /// The terms of the glossary, which the page changes one after another (ADR 0616).
    Glossary,
}

impl AskKind {
    /// Where the page tells the state of this request.
    fn place(self) -> AskPlace {
        match self {
            AskKind::RenameWorkspace(id) | AskKind::DeleteWorkspace(id) => AskPlace::Workspace(id),
            AskKind::UpdateCategory(id) | AskKind::DeleteCategory(id) => AskPlace::Category(id),
            AskKind::UpdateChannel(id)
            | AskKind::CallSettings(id)
            | AskKind::SetOverride(id, _) => AskPlace::ChannelSettings(id),
            AskKind::ArchiveChannel(id) | AskKind::RestoreChannel(id) => {
                AskPlace::ArchivedChannel(id)
            }
            AskKind::RemoveAccount(id)
            | AskKind::BanAccount(id)
            | AskKind::UnbanAccount(id)
            | AskKind::RecoverAccount(id) => AskPlace::Account(id),
            AskKind::AssignRole(_, account_id) => AskPlace::AccountRoles(account_id),
            AskKind::ApproveRequest(id) | AskKind::DenyRequest(id) => AskPlace::JoinRequest(id),
            AskKind::UpdateRole(role) | AskKind::DeleteRole(role) | AskKind::MoveRole(role) => {
                AskPlace::Role(role)
            }
            AskKind::SetLogo | AskKind::RemoveLogo => AskPlace::Logo,
            AskKind::AddTerm
            | AskKind::ChangeTerm(_)
            | AskKind::RemoveTerm(_)
            | AskKind::MoveTerm(_) => AskPlace::Glossary,
            what => AskPlace::Own(what),
        }
    }
}

/// Where a request stands. A request that the server did leaves no state: its effect
/// arrives with the state of the server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AskState {
    Sending,
    /// Why the server did not do it, as the page states it.
    Refused(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ask {
    pub what: AskKind,
    pub state: AskState,
}

/// What a request made and the person has not put away.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Made {
    /// The code of a new invite. No other answer of the server carries it (ADR 0111).
    Invite {
        invite_id: InviteId,
        code: InviteCode,
    },
    /// The secret of a new access token. No other answer carries it (ADR 0210).
    Token {
        access_token_id: AccessTokenId,
        token: AccessToken,
    },
    /// The workspaces that the account entered with an invite code (ADR 0115).
    Entered(Vec<WorkspaceId>),
    /// An export was asked for. Its state is on the page of the exports.
    Export(ExportId),
    /// The recovery of an account: the answer states when the server did it (ADR 0112).
    Recovered {
        account_id: AccountId,
        notice: junto_client_core::protocol::account::RecoveryNotice,
    },
}

impl Made {
    /// The kind of the request that makes this. Its place on the page tells it.
    pub const fn kind(&self) -> AskKind {
        match self {
            Self::Invite { .. } => AskKind::CreateInvite,
            Self::Token { .. } => AskKind::CreateToken,
            Self::Entered(_) => AskKind::RedeemInvite,
            Self::Export(_) => AskKind::RequestExport,
            Self::Recovered { account_id, .. } => AskKind::RecoverAccount(*account_id),
        }
    }
}

/// What the administration holds beside the state of the servers.
#[derive(Default)]
pub struct AdminState {
    pub(crate) open: Option<AdminOpen>,
    /// The collections that the window needs, with the task that opened each in the
    /// client core.
    held: HashMap<(ServerId, AdminPart), Task<()>>,
    /// The held collections whose open was answered by the client core. One of them that
    /// the state of its server does not hold any more is opened again: the client core
    /// dropped its readers with the account or the connection.
    settled: HashSet<(ServerId, AdminPart)>,
    /// The account of the server of the open layer. The layer and what it holds belong
    /// to that account.
    account: Option<AccountId>,
    /// The number of requests that the person made. An answer to an earlier request of
    /// the same kind does not change what the page shows.
    asks: u64,
    /// The number of times the layer changed its page or was closed. An answer that
    /// arrives after such a change is dropped: what it made belongs to a page that is
    /// gone.
    session: u64,
    /// The requests of the open page that are on their way or were refused, each with the
    /// number that `asks` gave it. The page tells one state for each kind of request, so a
    /// request does not replace what the page tells about a request of another kind.
    requests: Vec<(u64, Ask)>,
    /// What the requests of the open page made and the person has not put away: a code, a
    /// token, a recovery. Each place of the page holds what its newest request made.
    pub(crate) made: Vec<Made>,
    /// The entries of the audit log that the page has read.
    pub(crate) audit: records::AuditLog,
    /// The files of exports that the person saves, and those that were saved.
    pub(crate) exports: records::ExportFiles,
    /// The recordings of a workspace that the page has read.
    pub(crate) recordings: recordings::RecordingList,
    /// The roles whose permissions the person changed and whose change the state does
    /// not show yet (ADR 0791).
    pub(crate) role_changes: roles::RoleChanges,
    /// The glossary that the answer to the last change of the person carried, while the
    /// state shows no read of it that started after the answer (ADR 0616).
    pub(crate) glossary_answered: Option<glossary::Answered>,
    /// The moves of channels that the person sent and that the state does not show yet.
    pub(crate) channel_moves: moves::ChannelMoves,
}

/// The request that an answer belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Stamp {
    what: AskKind,
    number: u64,
    session: u64,
}

/// What an answer of the server is to the layer now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Answered {
    /// The page that asked is gone: nothing of the answer is kept or shown.
    Gone,
    /// The person made another request of the same kind on the page afterwards: the
    /// answer does not replace what the page tells about that one.
    Earlier,
    Current,
}

impl AdminState {
    /// Shows a page. What the page before told about a request and what a request made
    /// are put away, and a later answer to a request of that page is dropped.
    pub(crate) fn show(&mut self, open: AdminOpen) {
        if self.open != Some(open) {
            self.requests.clear();
            self.made.clear();
            self.session += 1;
            // The list of the recordings belongs to its page. A late answer finds no
            // list that waits for it.
            self.recordings = recordings::RecordingList::default();
            self.glossary_answered = None;
            self.channel_moves.release();
        }
        if self.open.map(|held| held.server_id()) != Some(open.server_id()) {
            self.audit = records::AuditLog::default();
            self.account = None;
        }
        self.open = Some(open);
    }

    pub(crate) fn close(&mut self) {
        self.open = None;
        self.account = None;
        self.session += 1;
        self.requests.clear();
        self.made.clear();
        self.audit = records::AuditLog::default();
        self.recordings = recordings::RecordingList::default();
        self.glossary_answered = None;
        self.channel_moves.release();
    }

    /// Binds the open layer to the account that its server shows now. A layer that was
    /// open for another account, or whose server shows no account any more, is closed
    /// with everything that it held of that account. Returns true when it closed.
    pub(crate) fn follow_account(&mut self, account: Option<AccountId>) -> bool {
        if self.open.is_none() {
            self.account = None;
            return false;
        }
        match (self.account, account) {
            (None, account) => {
                self.account = account;
                false
            }
            (Some(held), Some(now)) if held == now => false,
            (Some(_), _) => {
                self.close();
                // The changes that wait belong to the account of before.
                self.role_changes.forget();
                self.channel_moves.forget();
                true
            }
        }
    }

    /// Counts a request of the open page. It replaces what the page told about the earlier
    /// requests at the same place, so the place tells the newest one; a request at another
    /// place keeps its state.
    pub(crate) fn begin(&mut self, what: AskKind) -> Stamp {
        self.asks += 1;
        let place = what.place();
        self.requests.retain(|(_, ask)| ask.what.place() != place);
        self.requests.push((
            self.asks,
            Ask {
                what,
                state: AskState::Sending,
            },
        ));
        Stamp {
            what,
            number: self.asks,
            session: self.session,
        }
    }

    /// Takes the answer to a request: `refused` is why the server did not do it. Returns
    /// true when the answer is the one to the newest request of its kind on the open page;
    /// only then the page tells the refusal, or takes what the request made.
    pub(crate) fn settle(&mut self, stamp: Stamp, refused: Option<String>) -> bool {
        if self.answered(stamp) != Answered::Current {
            return false;
        }
        self.requests.retain(|(_, ask)| ask.what != stamp.what);
        if let Some(reason) = refused {
            self.requests.push((
                stamp.number,
                Ask {
                    what: stamp.what,
                    state: AskState::Refused(reason),
                },
            ));
        }
        true
    }

    pub(crate) fn answered(&self, stamp: Stamp) -> Answered {
        if stamp.session != self.session {
            Answered::Gone
        } else if self
            .requests
            .iter()
            .any(|(number, ask)| *number == stamp.number && ask.what == stamp.what)
        {
            Answered::Current
        } else {
            Answered::Earlier
        }
    }

    /// Holds what a request made. It replaces what an earlier request at the same place
    /// made; what requests at other places made stays (ADR 0761).
    pub(crate) fn make(&mut self, made: Made) {
        let place = made.kind().place();
        self.made.retain(|held| held.kind().place() != place);
        self.made.push(made);
    }

    /// Puts away what the request of this kind made, and what the earlier requests at its
    /// place made.
    pub(crate) fn put_away(&mut self, what: AskKind) {
        let place = what.place();
        self.made.retain(|made| made.kind().place() != place);
    }

    /// What the page tells about its requests, in the order in which they were made.
    pub(crate) fn asks(&self) -> Vec<Ask> {
        self.requests.iter().map(|(_, ask)| ask.clone()).collect()
    }

    /// Puts away the refusals that the page tells. The requests on their way stay.
    pub(crate) fn dismiss_refusals(&mut self) {
        self.requests
            .retain(|(_, ask)| ask.state == AskState::Sending);
    }
}

impl AppState {
    /// The administration as the window shows it. Absent while it is closed, and when
    /// the server of the open page is gone.
    pub fn admin_view(&self) -> Option<AdminView> {
        let open = self.admin.open?;
        view::derive(
            &self.snapshots(),
            &self.selection,
            &self.known,
            &self.admin,
            open,
        )
    }

    pub fn admin_open(&self) -> Option<AdminOpen> {
        self.admin.open
    }

    /// Shows a page of the administration. What the page before told about a request is
    /// put away.
    pub fn open_admin(&mut self, open: AdminOpen, cx: &mut Context<Self>) {
        self.admin.show(open);
        self.changed(cx);
    }

    pub fn close_admin(&mut self, cx: &mut Context<Self>) {
        self.admin.close();
        self.changed(cx);
    }

    /// Puts away the refusals that the page tells.
    pub fn dismiss_admin_ask(&mut self, cx: &mut Context<Self>) {
        self.admin.dismiss_refusals();
        // An act of the person ends the drawing of a refused move (ADR 0730).
        self.admin.channel_moves.release();
        cx.notify();
    }

    /// Puts away the recoveries that the rows of their accounts tell. A code or a token
    /// that a request made stays.
    pub fn dismiss_admin_recovered(&mut self, cx: &mut Context<Self>) {
        let before = self.admin.made.len();
        self.admin
            .made
            .retain(|made| !matches!(made, Made::Recovered { .. }));
        if self.admin.made.len() != before {
            cx.notify();
        }
    }

    /// Puts away what the request of this kind made. A code and a token are gone with it.
    pub fn dismiss_admin_made(&mut self, what: AskKind, cx: &mut Context<Self>) {
        self.admin.put_away(what);
        cx.notify();
    }

    /// The page that the layer shows becomes its selected page. A page that the person may
    /// not use gives way to another one when the view is derived; the page that was asked for
    /// would otherwise return when the permission arrives, and replace the page under the
    /// pointer (ADR 0761).
    fn keep_shown_admin_page(&mut self) {
        let (Some(open), Some(view)) = (self.admin.open, self.admin_view()) else {
            return;
        };
        let shown = match (open, &view.scope) {
            (AdminOpen::Server { server_id, .. }, Scope::Server { selected, .. }) => {
                AdminOpen::Server {
                    server_id,
                    page: *selected,
                }
            }
            (
                AdminOpen::Channel {
                    server_id,
                    workspace_id,
                    channel_id,
                    ..
                },
                Scope::Channel(settings),
            ) => AdminOpen::Channel {
                server_id,
                workspace_id,
                channel_id,
                page: settings.selected,
            },
            _ => return,
        };
        self.admin.open = Some(shown);
    }

    /// The collections that the window needs for archived channels outside the
    /// administration (ADR 0762): for an archived channel that the main area shows, the
    /// archived list of its workspace and the permissions of the account in the channel
    /// (ADR 0310); for the open notices and the open search, the archived list of each
    /// workspace that a notice or a result names with a channel that is in no channel
    /// list. The notices that the call window shows are those of the server of its call,
    /// whatever the main window shows (ADR 0742). The join requests of every server whose
    /// account may decide them, for their count (ADR 0761).
    pub(super) fn archived_readers(
        &self,
        view: &super::view::ShellView,
        snapshots: &[super::view::ServerSnapshot],
    ) -> Vec<(ServerId, AdminPart)> {
        let mut wanted: Vec<(ServerId, AdminPart)> = Vec::new();
        let mut want = |key: (ServerId, AdminPart)| {
            if !wanted.contains(&key) {
                wanted.push(key);
            }
        };
        for server in snapshots {
            let Some(model) = view::shown(server) else {
                continue;
            };
            if view::holds_server(model, ServerPermission::ApproveJoinRequests) {
                want((server.id, AdminPart::JoinRequests));
            }
            // The counts of the navigation (ADR 0761): the exports of the account, and the
            // archived channels of each workspace whose channels the person may restore.
            want((server.id, AdminPart::Exports));
            for workspace in &model.workspaces {
                if view::holds_workspace(workspace, WorkspacePermission::ManageChannel) {
                    want((server.id, AdminPart::Archived(workspace.workspace.id)));
                }
            }
        }
        let call_notices = self
            .media_calls()
            .call
            .as_ref()
            .map(|call| call.target.server_id)
            .filter(|_| self.call_window.is_some() && self.call_window_notices);
        if let Some(call_server) = call_notices
            && let Some(model) = snapshots
                .iter()
                .find(|server| server.id == call_server)
                .and_then(|server| view::shown(server))
        {
            for workspace_id in super::notices::archived_lists_of_notices(model) {
                want((call_server, AdminPart::Archived(workspace_id)));
            }
        }
        let Some(server_id) = self.shown.server else {
            return wanted;
        };
        if let Some((workspace_id, channel_id)) = view
            .workspace()
            .filter(|workspace| workspace.archived.is_some())
            .and_then(|workspace| Some((workspace.id, workspace.channel.as_ref()?.id)))
        {
            want((server_id, AdminPart::Archived(workspace_id)));
            want((server_id, AdminPart::ChannelPermissions(channel_id)));
        }
        let Some(model) = snapshots
            .iter()
            .find(|server| server.id == server_id)
            .and_then(|server| view::shown(server))
        else {
            return wanted;
        };
        if self.selection.notices == Some(server_id) {
            for workspace_id in super::notices::archived_lists_of_notices(model) {
                want((server_id, AdminPart::Archived(workspace_id)));
            }
        }
        for workspace_id in super::search::archived_lists_of_search(server_id, model, &self.known) {
            want((server_id, AdminPart::Archived(workspace_id)));
        }
        wanted
    }

    /// The collections of the client core that the window needs are held open: those of
    /// the open page, and those of [`Self::archived_readers`]. Every other collection is
    /// released.
    pub(super) fn follow_admin(
        &mut self,
        readers: Vec<(ServerId, AdminPart)>,
        cx: &mut Context<Self>,
    ) {
        // A save of an export does not outlive its server or its account.
        self.end_stale_export_saves();
        self.settle_role_changes();
        self.settle_glossary_answered();
        self.settle_channel_moves();
        self.keep_shown_admin_page();
        let mut wanted: Vec<(ServerId, AdminPart)> = Vec::new();
        let snapshots = self.snapshots();
        self.settle_saved_exports(&snapshots, cx);
        let model_of = |server_id: ServerId| {
            snapshots
                .iter()
                .find(|server| server.id == server_id)
                .and_then(|server| view::shown(server))
        };
        if let Some(open) = self.admin.open {
            let account = model_of(open.server_id()).map(|model| model.account.id);
            if self.admin.follow_account(account) {
                // The layer was open for another account: nothing of it is kept.
                self.admin.held.clear();
                self.admin.settled.clear();
            }
        }
        if let Some(open) = self.admin.open {
            let server_id = open.server_id();
            if let Some(model) = model_of(server_id) {
                let workspace_id = self
                    .shown
                    .workspace
                    .filter(|_| self.shown.server == Some(server_id));
                for part in view::needs(&open, model, workspace_id) {
                    wanted.push((server_id, part));
                }
            }
        }
        for key in readers {
            if !wanted.contains(&key) {
                wanted.push(key);
            }
        }
        let gone: Vec<(ServerId, AdminPart)> = self
            .admin
            .held
            .keys()
            .filter(|key| !wanted.contains(key))
            .copied()
            .collect();
        for key in gone {
            self.admin.held.remove(&key);
            self.admin.settled.remove(&key);
            if let Some(server) = self.client.server(key.0) {
                server.live().release_admin(key.1);
            }
        }
        // A collection whose open was answered and that the state does not hold: the
        // client core dropped it with the account or the connection. It is opened again.
        let dropped: Vec<(ServerId, AdminPart)> = self
            .admin
            .settled
            .iter()
            .filter(|key| {
                model_of(key.0).is_some_and(|model| {
                    model.admin.state(key.1) == junto_client_core::model::CollectionState::NotLoaded
                })
            })
            .copied()
            .collect();
        for key in dropped {
            self.admin.held.remove(&key);
            self.admin.settled.remove(&key);
        }
        for key in wanted {
            if self.admin.held.contains_key(&key) {
                continue;
            }
            let Some(server) = self.client.server(key.0) else {
                continue;
            };
            let opening = server.live().open_admin(key.1);
            let task = cx.spawn(async move |this, cx| {
                let result = opening.await;
                let _ = this.update(cx, |this, cx| {
                    // The client core did not take the collection: it holds no reader of
                    // it, and the next change of the state asks again. A read that failed
                    // leaves the collection open with its failure.
                    if matches!(result, Err(LoadError::Off | LoadError::NotInState)) {
                        this.admin.held.remove(&key);
                    } else if this.admin.held.contains_key(&key) {
                        this.admin.settled.insert(key);
                    }
                    cx.notify();
                });
            });
            self.admin.held.insert(key, task);
        }
    }

    /// Reads the archived lists of workspaces of a server again after a read that failed:
    /// the notices and the results that they place follow (ADR 0762).
    pub fn read_archived_lists_again(
        &mut self,
        server_id: ServerId,
        workspaces: &[WorkspaceId],
        cx: &mut Context<Self>,
    ) {
        for workspace_id in workspaces {
            self.reload_admin(server_id, AdminPart::Archived(*workspace_id));
        }
        self.changed(cx);
    }

    /// Reads every archived list of a server that the window holds and whose last read
    /// failed. A new search starts with it.
    pub(super) fn read_failed_archived_lists_again(&mut self, server_id: ServerId) {
        let held: Vec<AdminPart> = self
            .admin
            .held
            .keys()
            .filter(|(of, part)| *of == server_id && matches!(part, AdminPart::Archived(_)))
            .map(|(_, part)| *part)
            .collect();
        for part in held {
            self.reload_admin(server_id, part);
        }
    }

    /// Reads a collection of the open page again after a read that failed.
    pub fn reload_admin(&mut self, server_id: ServerId, part: AdminPart) {
        if let Some(server) = self.client.server(server_id) {
            server.live().read_admin_again(part);
        }
    }

    /// Sends one request of the administration. While it is on its way the page shows
    /// it as sending; a refusal is stated by the page; `done` runs when the server did
    /// it. The change itself arrives with the state of the server.
    pub(crate) fn admin_ask<T, F>(
        &mut self,
        server_id: ServerId,
        what: AskKind,
        request: impl FnOnce(ServerApi) -> F,
        done: impl FnOnce(&mut Self, T, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) where
        T: 'static,
        F: Future<Output = Result<T, ApiError>> + 'static,
    {
        let Some(server) = self.client.server(server_id) else {
            return;
        };
        let stamp = self.admin.begin(what);
        let asked = request(server.api());
        cx.spawn(async move |this, cx| {
            let answer = asked.await;
            let _ = this.update(cx, |this, cx| {
                let refused = answer
                    .as_ref()
                    .err()
                    .map(|error| text::admin::refusal(what, error));
                // Only the answer to the newest request of its kind on the open page
                // changes what the page tells and holds. What an earlier or a late answer carries, a
                // code or a token included, is shown to nobody and is not kept; the
                // effect of its request arrives with the state.
                if this.admin.settle(stamp, refused)
                    && let Ok(value) = answer
                {
                    done(this, value, cx);
                }
                this.changed(cx);
            });
        })
        .detach();
        cx.notify();
    }

    /// Draws this state of a request and of what a request made in place of what the
    /// servers told. The screen tool uses it.
    #[cfg(feature = "test-support")]
    pub fn show_admin_fixture(
        &mut self,
        ask: Option<Ask>,
        made: Option<Made>,
        cx: &mut Context<Self>,
    ) {
        self.admin.requests = ask.into_iter().map(|ask| (0, ask)).collect();
        self.admin.made = made.into_iter().collect();
        self.changed(cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use junto_client_core::protocol::ids::ServerRoleId;

    fn server_page(server_id: ServerId, page: AdminPage) -> AdminOpen {
        AdminOpen::Server { server_id, page }
    }

    #[test]
    fn an_answer_belongs_to_the_page_and_to_the_request_that_asked() {
        let server_id = ServerId::generate();
        let mut admin = AdminState::default();
        admin.show(server_page(server_id, AdminPage::Invites));
        let first = admin.begin(AskKind::CreateInvite);
        assert_eq!(admin.answered(first), Answered::Current);
        // The same page shown again keeps its request.
        admin.show(server_page(server_id, AdminPage::Invites));
        assert_eq!(admin.answered(first), Answered::Current);
        // A later request on the page makes the first one the earlier one.
        let second = admin.begin(AskKind::CreateInvite);
        assert_eq!(admin.answered(first), Answered::Earlier);
        assert_eq!(admin.answered(second), Answered::Current);

        // Another page: the answers of the page before are for nobody, and what that
        // page told and made is put away.
        admin.make(Made::Entered(Vec::new()));
        admin.show(server_page(server_id, AdminPage::Tokens));
        assert_eq!(admin.answered(first), Answered::Gone);
        assert_eq!(admin.answered(second), Answered::Gone);
        assert_eq!(admin.asks(), []);
        assert_eq!(admin.made, []);
        let token = admin.begin(AskKind::CreateToken);
        assert_eq!(admin.answered(token), Answered::Current);

        // The closed layer takes no answer.
        admin.close();
        assert_eq!(admin.answered(token), Answered::Gone);
        assert_eq!(admin.asks(), []);
        // A layer that is opened again does not take the answers of the one before.
        admin.show(server_page(server_id, AdminPage::Tokens));
        assert_eq!(admin.answered(token), Answered::Gone);
    }

    #[test]
    fn only_the_answer_to_the_newest_request_of_a_kind_changes_what_the_page_tells() {
        let server_id = ServerId::generate();
        let mut admin = AdminState::default();
        admin.show(server_page(server_id, AdminPage::Tokens));
        // Two tokens are asked for; the answer to the second one arrives first.
        let first = admin.begin(AskKind::CreateToken);
        let second = admin.begin(AskKind::CreateToken);
        assert_eq!(admin.answered(first), Answered::Earlier);
        assert!(admin.settle(second, None));
        assert_eq!(admin.asks(), []);
        admin.make(Made::Entered(Vec::new()));
        // The late answer to the first one takes nothing: its result would replace the
        // result of the second one.
        assert!(!admin.settle(first, None));
        assert!(!admin.settle(first, Some("No.".to_owned())));
        assert_eq!(admin.asks(), []);
        assert!(!admin.made.is_empty());

        // A refusal of the newest request is told at its control.
        let third = admin.begin(AskKind::CreateToken);
        assert_eq!(
            admin.asks(),
            [Ask {
                what: AskKind::CreateToken,
                state: AskState::Sending,
            }]
        );
        assert!(admin.settle(third, Some("No.".to_owned())));
        assert_eq!(
            admin.asks(),
            [Ask {
                what: AskKind::CreateToken,
                state: AskState::Refused("No.".to_owned()),
            }]
        );
        // The next request of the kind puts the refusal away.
        let fourth = admin.begin(AskKind::CreateToken);
        assert_eq!(admin.asks()[0].state, AskState::Sending);
        // After the page changed, an answer takes nothing.
        admin.show(server_page(server_id, AdminPage::Invites));
        assert!(!admin.settle(fourth, None));
    }

    /// What a request made replaces only what a request at the same place made: the rows of
    /// two recovered accounts each tell their recovery, and a new one at a row replaces the
    /// one before at that row (ADR 0761).
    #[test]
    fn what_a_request_made_is_replaced_only_at_its_place() {
        use junto_client_core::protocol::account::RecoveryNotice;
        use junto_client_core::protocol::ids::RecoveryId;

        let recovered = |account_id| Made::Recovered {
            account_id,
            notice: RecoveryNotice {
                id: RecoveryId::generate(),
                device_id: None,
                performed_by: None,
                performed_at: time::OffsetDateTime::UNIX_EPOCH,
            },
        };
        let mut admin = AdminState::default();
        admin.show(server_page(ServerId::generate(), AdminPage::People));
        let (ana, bruno) = (AccountId::generate(), AccountId::generate());
        let first = recovered(ana);
        admin.make(first.clone());
        let second = recovered(bruno);
        admin.make(second.clone());
        assert_eq!(admin.made, [first, second.clone()]);
        let again = recovered(ana);
        admin.make(again.clone());
        assert_eq!(admin.made, [second.clone(), again]);
        admin.put_away(AskKind::RecoverAccount(ana));
        assert_eq!(admin.made, [second]);
    }

    #[test]
    fn a_request_of_another_kind_keeps_the_state_of_a_request_on_its_way() {
        let server_id = ServerId::generate();
        let mut admin = AdminState::default();
        admin.show(server_page(server_id, AdminPage::Tokens));
        let revoked = AccessTokenId::generate();
        let revoke = admin.begin(AskKind::RevokeToken(revoked));
        let create = admin.begin(AskKind::CreateToken);
        assert_eq!(admin.answered(revoke), Answered::Current);
        assert_eq!(admin.answered(create), Answered::Current);
        assert_eq!(
            admin.asks(),
            [
                Ask {
                    what: AskKind::RevokeToken(revoked),
                    state: AskState::Sending,
                },
                Ask {
                    what: AskKind::CreateToken,
                    state: AskState::Sending,
                },
            ]
        );
        // The refusal of the first request is told while the second one is on its way.
        assert!(admin.settle(revoke, Some("No.".to_owned())));
        assert!(admin.settle(create, None));
        assert_eq!(
            admin.asks(),
            [Ask {
                what: AskKind::RevokeToken(revoked),
                state: AskState::Refused("No.".to_owned()),
            }]
        );
        // Putting the refusals away keeps a request on its way.
        let again = admin.begin(AskKind::CreateToken);
        admin.dismiss_refusals();
        assert_eq!(
            admin.asks(),
            [Ask {
                what: AskKind::CreateToken,
                state: AskState::Sending,
            }]
        );
        assert_eq!(admin.answered(again), Answered::Current);
    }

    #[test]
    fn a_row_that_tells_several_kinds_of_request_tells_the_newest() {
        let server_id = ServerId::generate();
        let mut admin = AdminState::default();
        admin.show(server_page(server_id, AdminPage::ServerRoles));
        let role = RoleRef::Server(ServerRoleId::generate());
        let other = RoleRef::Server(ServerRoleId::generate());
        let changed = admin.begin(AskKind::UpdateRole(role));
        assert!(admin.settle(changed, Some("Busy.".to_owned())));
        let elsewhere = admin.begin(AskKind::MoveRole(other));
        let moved = admin.begin(AskKind::MoveRole(role));
        assert_eq!(admin.answered(changed), Answered::Earlier);
        assert!(admin.settle(moved, Some("No answer.".to_owned())));
        assert_eq!(admin.answered(elsewhere), Answered::Current);
        assert_eq!(
            admin.asks(),
            [
                Ask {
                    what: AskKind::MoveRole(other),
                    state: AskState::Sending,
                },
                Ask {
                    what: AskKind::MoveRole(role),
                    state: AskState::Refused("No answer.".to_owned()),
                },
            ]
        );
    }

    #[test]
    fn a_change_of_the_glossary_replaces_the_state_of_every_earlier_change_of_its_terms() {
        let server_id = ServerId::generate();
        let mut admin = AdminState::default();
        admin.show(server_page(server_id, AdminPage::Glossary));
        let removed = admin.begin(AskKind::RemoveTerm(3));
        assert!(admin.settle(removed, Some("No.".to_owned())));
        let added = admin.begin(AskKind::AddTerm);
        assert_eq!(
            admin.asks(),
            [Ask {
                what: AskKind::AddTerm,
                state: AskState::Sending,
            }]
        );
        let moved = admin.begin(AskKind::MoveTerm(0));
        assert_eq!(admin.answered(added), Answered::Earlier);
        assert_eq!(admin.answered(moved), Answered::Current);
    }

    #[test]
    fn the_layer_belongs_to_the_account_that_it_was_opened_for() {
        let server_id = ServerId::generate();
        let (first, second) = (AccountId::generate(), AccountId::generate());
        let mut admin = AdminState::default();
        // A closed layer follows nothing.
        assert!(!admin.follow_account(Some(first)));

        admin.show(server_page(server_id, AdminPage::Invites));
        let asked = admin.begin(AskKind::CreateInvite);
        admin.make(Made::Entered(Vec::new()));
        // The state of the server arrives after the layer opened.
        assert!(!admin.follow_account(None));
        assert!(!admin.follow_account(Some(first)));
        assert!(!admin.follow_account(Some(first)));
        assert!(admin.open.is_some() && !admin.made.is_empty());

        // Another account on the same server: the layer closes with what it held.
        assert!(admin.follow_account(Some(second)));
        assert_eq!(admin.open, None);
        assert_eq!(admin.made, []);
        assert_eq!(admin.asks(), []);
        assert_eq!(admin.answered(asked), Answered::Gone);

        // The server shows no account any more: the layer closes too.
        admin.show(server_page(server_id, AdminPage::Workspaces));
        assert!(!admin.follow_account(Some(second)));
        assert!(admin.follow_account(None));
        assert_eq!(admin.open, None);

        // The page of another server starts without the account of the first one.
        admin.show(server_page(server_id, AdminPage::Workspaces));
        assert!(!admin.follow_account(Some(first)));
        admin.show(server_page(ServerId::generate(), AdminPage::Workspaces));
        assert!(!admin.follow_account(Some(second)));
        assert!(admin.open.is_some());
    }
}
