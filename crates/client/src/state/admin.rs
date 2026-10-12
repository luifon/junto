//! What the application does for the administration of a server (ADR 0761): which page is
//! open, which collections of the client core that page needs, and the one request of
//! the person that is on its way or was refused.

mod overrides;
mod people;
pub(crate) mod recordings;
mod records;
mod structure;
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

pub use recordings::RecordingList;
pub use records::{AuditLog, AuditScope, ExportFiles};
pub use view::AdminView;

use super::AppState;
use crate::text;

/// A page of the administration of a server.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AdminPage {
    Workspaces,
    People,
    Invites,
    JoinRequests,
    Tokens,
    AuditLog,
    /// The categories and channels of the shown workspace.
    Channels,
    /// The archived channels, the archive period and the retention of the shown workspace.
    Archive,
    /// The members of the shown workspace.
    Members,
    /// The recordings of the shown workspace that the account can see (ADR 0784).
    Recordings,
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
    DeleteChannel(ChannelId),
    ArchiveChannel(ChannelId),
    RestoreChannel(ChannelId),
    CallSettings(ChannelId),
    SetOverride(ChannelId, WorkspaceRoleId),
    ArchivePeriod,
    FileRetention,
    RecordingRetention,
    DirectRetention,
    AddMember(AccountId),
    RemoveAccount(AccountId),
    BanAccount(AccountId),
    UnbanAccount(AccountId),
    CreateInvite,
    RevokeInvite(InviteId),
    ApproveRequest(JoinRequestId),
    DenyRequest(JoinRequestId),
    CreateToken,
    RevokeToken(AccessTokenId),
    RequestExport,
    RemoveExport(ExportId),
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
    /// The number of requests that the person made. An answer to an earlier request does
    /// not change what the page shows.
    asks: u64,
    /// The number of times the layer changed its page or was closed. An answer that
    /// arrives after such a change is dropped: what it made belongs to a page that is
    /// gone.
    session: u64,
    pub(crate) ask: Option<Ask>,
    /// What the last request made and the person has not put away: a code, a token.
    pub(crate) made: Option<Made>,
    /// The entries of the audit log that the page has read.
    pub(crate) audit: records::AuditLog,
    /// The files of exports that the person saves, and those that were saved.
    pub(crate) exports: records::ExportFiles,
    /// The recordings of a workspace that the page has read.
    pub(crate) recordings: recordings::RecordingList,
}

/// The request that an answer belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Stamp {
    number: u64,
    session: u64,
}

/// What an answer of the server is to the layer now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Answered {
    /// The page that asked is gone: nothing of the answer is kept or shown.
    Gone,
    /// The person made another request on the page afterwards: the answer does not
    /// replace what the page tells about that one.
    Earlier,
    Current,
}

impl AdminState {
    /// Shows a page. What the page before told about a request and what a request made
    /// are put away, and a later answer to a request of that page is dropped.
    pub(crate) fn show(&mut self, open: AdminOpen) {
        if self.open != Some(open) {
            self.ask = None;
            self.made = None;
            self.session += 1;
            // The list of the recordings belongs to its page. A late answer finds no
            // list that waits for it.
            self.recordings = recordings::RecordingList::default();
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
        self.ask = None;
        self.made = None;
        self.audit = records::AuditLog::default();
        self.recordings = recordings::RecordingList::default();
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
                true
            }
        }
    }

    /// Counts a request of the open page.
    pub(crate) fn begin(&mut self, what: AskKind) -> Stamp {
        self.asks += 1;
        self.ask = Some(Ask {
            what,
            state: AskState::Sending,
        });
        Stamp {
            number: self.asks,
            session: self.session,
        }
    }

    /// Takes the answer to a request: `refused` is why the server did not do it. Returns
    /// true when the answer is the one to the newest request of the open page; only
    /// then the page tells the refusal, or takes what the request made.
    pub(crate) fn settle(&mut self, stamp: Stamp, what: AskKind, refused: Option<String>) -> bool {
        if self.answered(stamp) != Answered::Current {
            return false;
        }
        self.ask = refused.map(|reason| Ask {
            what,
            state: AskState::Refused(reason),
        });
        true
    }

    pub(crate) fn answered(&self, stamp: Stamp) -> Answered {
        if stamp.session != self.session {
            Answered::Gone
        } else if stamp.number != self.asks {
            Answered::Earlier
        } else {
            Answered::Current
        }
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

    /// Puts away what was told about the last request.
    pub fn dismiss_admin_ask(&mut self, cx: &mut Context<Self>) {
        self.admin.ask = None;
        cx.notify();
    }

    /// Puts away what the last request made. A code and a token are gone with it.
    pub fn dismiss_admin_made(&mut self, cx: &mut Context<Self>) {
        self.admin.made = None;
        cx.notify();
    }

    /// The collections of the client core that the window needs are held open: those of
    /// the open page, and the archived list of the workspace of an archived channel that
    /// the main area shows (ADR 0762). Every other collection is released.
    pub(super) fn follow_admin(
        &mut self,
        archived: Option<(ServerId, WorkspaceId)>,
        cx: &mut Context<Self>,
    ) {
        // A save of an export does not outlive its server or its account.
        self.end_stale_export_saves();
        let mut wanted: Vec<(ServerId, AdminPart)> = Vec::new();
        let snapshots = self.snapshots();
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
        if let Some((server_id, workspace_id)) = archived {
            let key = (server_id, AdminPart::Archived(workspace_id));
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
                // Only the answer to the newest request of the open page changes what
                // the page tells and holds. What an earlier or a late answer carries, a
                // code or a token included, is shown to nobody and is not kept; the
                // effect of its request arrives with the state.
                if this.admin.settle(stamp, what, refused)
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
        self.admin.ask = ask;
        self.admin.made = made;
        self.changed(cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        admin.made = Some(Made::Entered(Vec::new()));
        admin.show(server_page(server_id, AdminPage::Tokens));
        assert_eq!(admin.answered(first), Answered::Gone);
        assert_eq!(admin.answered(second), Answered::Gone);
        assert_eq!(admin.ask, None);
        assert_eq!(admin.made, None);
        let token = admin.begin(AskKind::CreateToken);
        assert_eq!(admin.answered(token), Answered::Current);

        // The closed layer takes no answer.
        admin.close();
        assert_eq!(admin.answered(token), Answered::Gone);
        assert_eq!(admin.ask, None);
        // A layer that is opened again does not take the answers of the one before.
        admin.show(server_page(server_id, AdminPage::Tokens));
        assert_eq!(admin.answered(token), Answered::Gone);
    }

    #[test]
    fn only_the_answer_to_the_newest_request_changes_what_the_page_tells() {
        let server_id = ServerId::generate();
        let mut admin = AdminState::default();
        admin.show(server_page(server_id, AdminPage::Tokens));
        // Two tokens are asked for; the answer to the second one arrives first.
        let first = admin.begin(AskKind::CreateToken);
        let second = admin.begin(AskKind::CreateToken);
        assert!(admin.settle(second, AskKind::CreateToken, None));
        assert_eq!(admin.ask, None);
        admin.made = Some(Made::Entered(Vec::new()));
        // The late answer to the first one takes nothing: its result would replace the
        // result of the second one.
        assert!(!admin.settle(first, AskKind::CreateToken, None));
        assert!(!admin.settle(first, AskKind::CreateToken, Some("No.".to_owned())));
        assert_eq!(admin.ask, None);
        assert!(admin.made.is_some());

        // A refusal of the newest request is told at its control.
        let third = admin.begin(AskKind::RevokeToken(AccessTokenId::generate()));
        assert_eq!(
            admin.ask.as_ref().map(|ask| &ask.state),
            Some(&AskState::Sending)
        );
        assert!(admin.settle(third, AskKind::CreateToken, Some("No.".to_owned())));
        assert_eq!(
            admin.ask,
            Some(Ask {
                what: AskKind::CreateToken,
                state: AskState::Refused("No.".to_owned()),
            })
        );
        // After the page changed, an answer takes nothing.
        let fourth = admin.begin(AskKind::CreateToken);
        admin.show(server_page(server_id, AdminPage::Invites));
        assert!(!admin.settle(fourth, AskKind::CreateToken, None));
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
        admin.made = Some(Made::Entered(Vec::new()));
        // The state of the server arrives after the layer opened.
        assert!(!admin.follow_account(None));
        assert!(!admin.follow_account(Some(first)));
        assert!(!admin.follow_account(Some(first)));
        assert!(admin.open.is_some() && admin.made.is_some());

        // Another account on the same server: the layer closes with what it held.
        assert!(admin.follow_account(Some(second)));
        assert_eq!(admin.open, None);
        assert_eq!(admin.made, None);
        assert_eq!(admin.ask, None);
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
