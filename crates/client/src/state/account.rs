//! What the application does for the account layer (ADR 0781): which page is open, the
//! one request of the person that is on its way or was refused, the devices and the
//! recoveries of the account as the server listed them last, and the flow that approves a
//! new device.

use std::future::Future;
use std::path::PathBuf;

use gpui::Context;
use junto_client_core::api::{ApiError, ServerApi};
use junto_client_core::flow::approve_link::{
    ApproveLinkAction, ApproveLinkActionKind, ApproveLinkFlow, ApproveLinkState,
};
use junto_client_core::flow::{FlowError, FlowView};
use junto_client_core::live::Notice;
use junto_client_core::profile::{self, NAME_MAX_CHARS};
use junto_client_core::protocol::account::{Device, DeviceOrigin, RecoveryNotice};
use junto_client_core::protocol::events::Event;
use junto_client_core::protocol::ids::{AccountId, DeviceId, ServerId, WorkspaceId};
use junto_client_core::protocol::secret::LinkCode;
use time::OffsetDateTime;

use super::AppState;
use super::view::{ServerSnapshot, shown_model};
use crate::text;
use junto_client_core::flow::access::{AccessAction, AccessState};

/// The most devices of an account that the layer holds and lists. A server that lists
/// more gets the count of the others stated.
pub const MOST_DEVICES: usize = 200;

/// The most recoveries of an account that the layer holds and lists, the newest ones. A
/// server that lists more gets the count of the others stated.
pub const MOST_RECOVERIES: usize = 200;

/// The most characters of a typed link code that are sent. A link code has 8 symbols in
/// two groups (ADR 0110).
pub const LONGEST_TYPED_CODE: usize = 32;

/// A page of the account layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AccountPage {
    Profile,
    Devices,
    /// Every recovery of the account, also those that this device acknowledged
    /// (ADR 0112, ADR 0118).
    Recoveries,
    /// Leaving a workspace and deleting the account (ADR 0121).
    Departure,
}

impl AccountPage {
    /// True for a page that lists what it reads from the server when it opens.
    const fn reads_a_list(self) -> bool {
        matches!(self, Self::Devices | Self::Recoveries)
    }
}

/// What the account layer shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AccountOpen {
    pub server_id: ServerId,
    pub page: AccountPage,
}

/// What a request of the account layer asks for. A page shows the state of a request at
/// the control that made it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AskKind {
    SaveName,
    SetPicture,
    RemovePicture,
    RenameDevice(DeviceId),
    RemoveDevice(DeviceId),
    LeaveWorkspace(WorkspaceId),
    DeleteAccount,
}

/// Where a request stands. A request that the server did leaves no state: its effect
/// arrives with the state of the server, or with the list of devices.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AskState {
    Sending,
    /// Why it was not done, as the page states it.
    Refused(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ask {
    pub what: AskKind,
    pub state: AskState,
}

/// The flow that approves a new device, as an interface reads it.
pub type LinkFlowView = FlowView<ApproveLinkState, ApproveLinkActionKind>;

/// What the account layer holds beside the state of the servers.
#[derive(Default)]
pub struct AccountState {
    open: Option<AccountOpen>,
    /// The account of the server of the open layer. The layer and what it holds belong
    /// to that account.
    account: Option<AccountId>,
    /// The number of requests that the person made. An answer to an earlier request does
    /// not change what the page shows.
    asks: u64,
    /// The number of times the layer changed its page or was closed. An answer that
    /// arrives after such a change is dropped.
    session: u64,
    ask: Option<Ask>,
    /// The devices as the server listed them last, at most [`MOST_DEVICES`].
    devices: Option<Vec<Device>>,
    /// The number of devices that the server listed beyond the held ones.
    devices_hidden: usize,
    /// Why the last read of the devices failed.
    devices_failed: Option<String>,
    /// The recoveries as the server listed them last, the newest first, at most
    /// [`MOST_RECOVERIES`].
    recoveries: Option<Vec<RecoveryNotice>>,
    /// The number of recoveries that the server listed beyond the held ones.
    recoveries_hidden: usize,
    /// Why the last read of the recoveries failed.
    recoveries_failed: Option<String>,
    /// A read of the list of the open page is on its way. One read runs at a time.
    reading: bool,
    /// The list of the open page is to be read: the page opened, a request changed a
    /// device, or the server told a change. Changes that arrive while a read runs make one
    /// further read.
    stale: bool,
    /// The flow that approves a new device. It holds the typed code while it shows the
    /// key, and is dropped with the page.
    link: Option<ApproveLinkFlow>,
    #[cfg(feature = "test-support")]
    link_fixture: Option<LinkFlowView>,
}

/// The request that an answer belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Stamp {
    number: u64,
    session: u64,
}

impl AccountState {
    /// Shows a page. What the page before told about a request, the devices that it
    /// listed and the code that it looked up are put away, and a later answer to a
    /// request of that page is dropped.
    pub(crate) fn show(&mut self, open: AccountOpen) {
        if self.open == Some(open) {
            return;
        }
        if self.open.map(|held| held.server_id) != Some(open.server_id) {
            self.account = None;
        }
        self.end_page();
        self.open = Some(open);
        self.stale = open.page.reads_a_list();
    }

    pub(crate) fn close(&mut self) {
        self.end_page();
        self.open = None;
        self.account = None;
    }

    fn end_page(&mut self) {
        self.session += 1;
        self.ask = None;
        self.devices = None;
        self.devices_hidden = 0;
        self.devices_failed = None;
        self.recoveries = None;
        self.recoveries_hidden = 0;
        self.recoveries_failed = None;
        // A read that is on its way stays counted: the next page reads when it has ended.
        self.stale = false;
        self.link = None;
        #[cfg(feature = "test-support")]
        {
            self.link_fixture = None;
        }
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

    /// True while a request of the open page is on its way.
    pub(crate) fn sending(&self) -> bool {
        matches!(
            self.ask,
            Some(Ask {
                state: AskState::Sending,
                ..
            })
        )
    }

    /// Counts a request of the open page. A page sends one request at a time: while one
    /// is on its way no other starts. So the answer to a request is the answer to the
    /// newest request of its page, and its refusal is told.
    pub(crate) fn begin(&mut self, what: AskKind) -> Option<Stamp> {
        if self.sending() {
            return None;
        }
        self.asks += 1;
        self.ask = Some(Ask {
            what,
            state: AskState::Sending,
        });
        Some(Stamp {
            number: self.asks,
            session: self.session,
        })
    }

    /// Puts away what was told about the last request. A request that is on its way
    /// stays.
    pub(crate) fn dismiss(&mut self) {
        if !self.sending() {
            self.ask = None;
        }
    }

    /// Takes the answer to a request: `refused` is why it was not done. Returns true when
    /// the answer is the one to the newest request of the open page; only then the page
    /// tells the refusal.
    pub(crate) fn settle(&mut self, stamp: Stamp, what: AskKind, refused: Option<String>) -> bool {
        if stamp.session != self.session || stamp.number != self.asks {
            return false;
        }
        self.ask = refused.map(|reason| Ask {
            what,
            state: AskState::Refused(reason),
        });
        true
    }

    /// The server told a change of the devices of the account of `server_id`, or changes
    /// were missed. Returns true when the open page lists the devices of that server.
    pub(crate) fn devices_changed(&mut self, server_id: ServerId) -> bool {
        self.list_changed(server_id, AccountPage::Devices)
    }

    /// The list of `page` of the account of `server_id` is to be read again. Returns true
    /// when the open page is that page.
    fn list_changed(&mut self, server_id: ServerId, page: AccountPage) -> bool {
        let listed = self
            .open
            .is_some_and(|open| open.server_id == server_id && open.page == page);
        if listed {
            self.stale = true;
        }
        listed
    }

    /// True when a read of the list of the open page is to start now. One read runs at a
    /// time.
    fn takes_read(&mut self) -> bool {
        let wanted = self.open.is_some_and(|open| open.page.reads_a_list())
            && self.account.is_some()
            && self.stale
            && !self.reading;
        if wanted {
            self.stale = false;
            self.reading = true;
        }
        wanted
    }

    /// Takes the answer of a read of the devices. An answer for a page that is gone is
    /// dropped.
    fn devices_read(&mut self, session: u64, read: Result<Vec<Device>, String>) {
        self.reading = false;
        if session != self.session {
            return;
        }
        match read {
            Ok(mut devices) => {
                self.devices_hidden = devices.len().saturating_sub(MOST_DEVICES);
                devices.truncate(MOST_DEVICES);
                self.devices = Some(devices);
                self.devices_failed = None;
            }
            Err(reason) => self.devices_failed = Some(reason),
        }
    }

    /// Takes the answer of a read of the recoveries, which the server gives the oldest
    /// first. An answer for a page that is gone is dropped.
    fn recoveries_read(&mut self, session: u64, read: Result<Vec<RecoveryNotice>, String>) {
        self.reading = false;
        if session != self.session {
            return;
        }
        match read {
            Ok(mut recoveries) => {
                recoveries.reverse();
                self.recoveries_hidden = recoveries.len().saturating_sub(MOST_RECOVERIES);
                recoveries.truncate(MOST_RECOVERIES);
                self.recoveries = Some(recoveries);
                self.recoveries_failed = None;
            }
            Err(reason) => self.recoveries_failed = Some(reason),
        }
    }
}

/// The picture of the account as the server describes it (ADR 0119).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Picture {
    pub hash: String,
    pub bytes: u32,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfilePage {
    /// The name that the server holds for the account.
    pub name: String,
    pub picture: Option<Picture>,
}

/// One device of the account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceLine {
    pub id: DeviceId,
    /// Empty for a device without a name.
    pub name: String,
    /// The device of the session of this window.
    pub this_device: bool,
    /// How the key of the device came to the account.
    pub origin: DeviceOrigin,
    pub added: OffsetDateTime,
    pub last_login: Option<OffsetDateTime>,
}

/// The devices as the page lists them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceList {
    Reading,
    /// The first read failed, for this reason.
    Failed(String),
    Listed {
        rows: Vec<DeviceLine>,
        /// The account has one device: it cannot be removed (ADR 0112).
        only: bool,
        /// The devices that the server listed beyond the shown ones.
        hidden: usize,
        /// Why the last read failed. The rows are those of the read before.
        failed: Option<String>,
    },
}

/// The link of a new device, as the page shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkView {
    EnterCode {
        looking_up: bool,
        error: Option<String>,
    },
    /// The person compares the key with the one that the new device shows.
    Confirm {
        device_name: String,
        fingerprint: String,
        expires_at: OffsetDateTime,
        approving: bool,
        error: Option<String>,
    },
    /// The approval was sent, and nothing tells whether it took effect.
    OutcomeUnknown {
        device_name: String,
        fingerprint: String,
        checking: bool,
        error: Option<String>,
    },
    Approved {
        device_name: String,
    },
}

impl LinkView {
    /// True while an action of the flow runs.
    pub const fn busy(&self) -> bool {
        match self {
            Self::EnterCode { looking_up, .. } => *looking_up,
            Self::Confirm { approving, .. } => *approving,
            Self::OutcomeUnknown { checking, .. } => *checking,
            Self::Approved { .. } => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DevicesPage {
    pub list: DeviceList,
    /// Absent until the flow exists: the layer was opened a moment ago.
    pub link: Option<LinkView>,
}

/// One recovery of the account (ADR 0112).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryLine {
    pub id: junto_client_core::protocol::ids::RecoveryId,
    pub performed_at: OffsetDateTime,
    /// The host command of the server did it, and not the server owner (ADR 0114).
    pub by_host: bool,
    /// The recovery added the device of the session of this window.
    pub this_device: bool,
    /// This device has not acknowledged the notice of the recovery yet (ADR 0118).
    pub unseen: bool,
}

/// The recoveries as the page lists them, the newest first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecoveryList {
    Reading,
    /// The first read failed, for this reason.
    Failed(String),
    Listed {
        rows: Vec<RecoveryLine>,
        /// The older recoveries that the server listed beyond the shown ones.
        hidden: usize,
        /// Why the last read failed. The rows are those of the read before.
        failed: Option<String>,
    },
}

/// One workspace of which the account is a member, on the page that leaves it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DepartureWorkspace {
    pub id: WorkspaceId,
    pub name: String,
}

/// The page that leaves a workspace or deletes the account (ADR 0121).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeparturePage {
    /// The workspaces of which the account is a member, in the order of the server.
    pub workspaces: Vec<DepartureWorkspace>,
    /// The account is the server owner: it cannot leave its last workspace or delete
    /// itself before another account becomes the owner.
    pub owner: bool,
}

impl DeparturePage {
    /// Leaving this workspace makes the account inactive.
    pub fn leaves_the_server(&self, workspace_id: WorkspaceId) -> bool {
        self.workspaces.len() == 1 && self.workspaces[0].id == workspace_id
    }

    /// The account may leave this workspace now.
    pub fn may_leave(&self, workspace_id: WorkspaceId) -> bool {
        !(self.owner && self.leaves_the_server(workspace_id))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PageView {
    Profile(ProfilePage),
    Devices(DevicesPage),
    Recoveries(RecoveryList),
    Departure(DeparturePage),
}

/// The account layer as the window draws it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountView {
    pub server_id: ServerId,
    pub server_name: String,
    pub account_id: AccountId,
    pub selected: AccountPage,
    pub page: PageView,
    pub ask: Option<Ask>,
}

impl AccountView {
    pub fn asked(&self, what: AskKind) -> Option<&AskState> {
        self.ask
            .as_ref()
            .filter(|ask| ask.what == what)
            .map(|ask| &ask.state)
    }

    pub fn sending(&self, what: AskKind) -> bool {
        self.asked(what) == Some(&AskState::Sending)
    }

    /// True while a request of the page is on its way. The page sends no other one then.
    pub fn busy(&self) -> bool {
        matches!(
            self.ask,
            Some(Ask {
                state: AskState::Sending,
                ..
            })
        )
    }

    pub fn refused(&self, what: AskKind) -> Option<&str> {
        match self.asked(what) {
            Some(AskState::Refused(reason)) => Some(reason),
            _ => None,
        }
    }
}

/// A name that a server gave, as text of another party: without control characters and
/// cut at the length of a name.
pub fn shown_name(name: &str) -> String {
    name.chars()
        .filter(|symbol| !symbol.is_control())
        .take(NAME_MAX_CHARS)
        .collect::<String>()
        .trim()
        .to_owned()
}

fn link_view(flow: &LinkFlowView) -> LinkView {
    let error = flow.error.as_ref().map(text::account::link_error);
    let busy = flow.in_progress.is_some();
    match &flow.state {
        ApproveLinkState::EnterCode => LinkView::EnterCode {
            looking_up: busy,
            error,
        },
        ApproveLinkState::Confirm(pending) => LinkView::Confirm {
            device_name: shown_name(&pending.device_name),
            fingerprint: text::account::key_fingerprint(&pending.public_key),
            expires_at: pending.expires_at,
            approving: busy,
            error,
        },
        ApproveLinkState::OutcomeUnknown(pending) => LinkView::OutcomeUnknown {
            device_name: shown_name(&pending.device_name),
            fingerprint: text::account::key_fingerprint(&pending.public_key),
            checking: busy,
            error,
        },
        ApproveLinkState::Approved { device } => LinkView::Approved {
            device_name: shown_name(&device.name),
        },
    }
}

fn device_list(state: &AccountState) -> DeviceList {
    match (&state.devices, &state.devices_failed) {
        (None, Some(reason)) => DeviceList::Failed(reason.clone()),
        (None, None) => DeviceList::Reading,
        (Some(devices), failed) => DeviceList::Listed {
            rows: devices
                .iter()
                .map(|device| DeviceLine {
                    id: device.id,
                    name: shown_name(&device.name),
                    this_device: device.current,
                    origin: device.origin,
                    added: device.created_at,
                    last_login: device.last_login_at,
                })
                .collect(),
            only: devices.len() + state.devices_hidden == 1,
            hidden: state.devices_hidden,
            failed: failed.clone(),
        },
    }
}

fn recovery_list(state: &AccountState, server: &ServerSnapshot) -> RecoveryList {
    let (device, unseen) = match &server.access {
        AccessState::LoggedIn {
            device_id,
            recoveries,
            ..
        } => (Some(*device_id), recoveries.as_slice()),
        _ => (None, &[][..]),
    };
    match (&state.recoveries, &state.recoveries_failed) {
        (None, Some(reason)) => RecoveryList::Failed(reason.clone()),
        (None, None) => RecoveryList::Reading,
        (Some(recoveries), failed) => RecoveryList::Listed {
            rows: recoveries
                .iter()
                .map(|recovery| RecoveryLine {
                    id: recovery.id,
                    performed_at: recovery.performed_at,
                    by_host: recovery.performed_by.is_none(),
                    this_device: device.is_some() && recovery.device_id == device,
                    unseen: unseen.iter().any(|notice| notice.id == recovery.id),
                })
                .collect(),
            hidden: state.recoveries_hidden,
            failed: failed.clone(),
        },
    }
}

/// The account layer, derived without a side effect. Absent while the layer is closed,
/// when its server is gone, and when the server shows no account or another account
/// than the one that the layer was opened for.
pub fn derive(
    servers: &[ServerSnapshot],
    state: &AccountState,
    link: Option<&LinkFlowView>,
) -> Option<AccountView> {
    let open = state.open?;
    let server = servers.iter().find(|server| server.id == open.server_id)?;
    let model = shown_model(server)?;
    if state
        .account
        .is_some_and(|account| account != model.account.id)
    {
        return None;
    }
    let page = match open.page {
        AccountPage::Profile => PageView::Profile(ProfilePage {
            name: shown_name(&model.account.name),
            picture: model.account.avatar.as_ref().map(|avatar| Picture {
                hash: avatar.hash.clone(),
                bytes: avatar.bytes,
                width: avatar.width,
                height: avatar.height,
            }),
        }),
        AccountPage::Devices => PageView::Devices(DevicesPage {
            list: device_list(state),
            link: link.map(link_view),
        }),
        AccountPage::Recoveries => PageView::Recoveries(recovery_list(state, server)),
        AccountPage::Departure => PageView::Departure(DeparturePage {
            workspaces: model
                .workspaces
                .iter()
                .filter(|workspace| {
                    model
                        .account
                        .workspace_ids
                        .contains(&workspace.workspace.id)
                })
                .map(|workspace| DepartureWorkspace {
                    id: workspace.workspace.id,
                    name: text::label(&workspace.workspace.name),
                })
                .collect(),
            owner: model.account.is_owner,
        }),
    };
    Some(AccountView {
        server_id: server.id,
        server_name: server.name.clone(),
        account_id: model.account.id,
        selected: open.page,
        page,
        ask: state.ask.clone(),
    })
}

/// Reads a file that the person chose as a picture. Only a regular file is read, and at
/// most one byte more than the largest picture, whatever the size of the file.
fn read_picture(path: &std::path::Path) -> Result<Vec<u8>, &'static str> {
    let bytes = profile::read_avatar_file(path).map_err(|_| text::account::PICTURE_NOT_READ)?;
    match profile::check_avatar(&bytes) {
        Ok(_) => Ok(bytes),
        Err(invalid) => Err(text::account::invalid_picture(invalid)),
    }
}

impl AppState {
    /// The account layer as the window shows it.
    pub fn account_view(&self) -> Option<AccountView> {
        self.account.open?;
        let link = self.account_link();
        derive(&self.snapshots(), &self.account, link.as_ref())
    }

    fn account_link(&self) -> Option<LinkFlowView> {
        #[cfg(feature = "test-support")]
        if let Some(fixture) = &self.account.link_fixture {
            return Some(fixture.clone());
        }
        self.account.link.as_ref().map(ApproveLinkFlow::view)
    }

    pub fn account_open(&self) -> Option<AccountOpen> {
        self.account.open
    }

    /// Shows a page of the account layer.
    pub fn open_account(&mut self, open: AccountOpen, cx: &mut Context<Self>) {
        self.account.show(open);
        self.changed(cx);
    }

    /// Closes the account layer. The code that it looked up is gone with it.
    pub fn close_account(&mut self, cx: &mut Context<Self>) {
        self.account.close();
        self.changed(cx);
    }

    /// Puts away what was told about the last request. A request that is on its way
    /// stays.
    pub fn dismiss_account_ask(&mut self, cx: &mut Context<Self>) {
        self.account.dismiss();
        cx.notify();
    }

    /// True while a request of the open page of the account layer is on its way. The
    /// page sends no other one then.
    pub fn account_busy(&self) -> bool {
        self.account.sending()
    }

    /// Runs after each change: the layer follows the account of its server, the flow of
    /// the devices page exists while the page is open, and a list that is to be read is
    /// read.
    pub(super) fn follow_account(&mut self, cx: &mut Context<Self>) {
        let Some(open) = self.account.open else {
            return;
        };
        let account = self
            .snapshots()
            .iter()
            .find(|server| server.id == open.server_id)
            .and_then(shown_model)
            .map(|model| model.account.id);
        if self.account.follow_account(account) || account.is_none() {
            return;
        }
        if !open.page.reads_a_list() {
            return;
        }
        let Some(server) = self.client.server(open.server_id) else {
            return;
        };
        if open.page == AccountPage::Devices && self.account.link.is_none() {
            self.account.link = Some(server.approve_link());
        }
        if !self.account.takes_read() {
            return;
        }
        let Some(account_id) = self.account.account else {
            return;
        };
        let session = self.account.session;
        let api = server.api_for(account_id);
        let failure = |error: ApiError| text::flow_error(&FlowError::from(&error));
        if open.page == AccountPage::Recoveries {
            cx.spawn(async move |this, cx| {
                let read = api.recoveries().await.map_err(failure);
                let _ = this.update(cx, |this, cx| {
                    this.account.recoveries_read(session, read);
                    this.changed(cx);
                });
            })
            .detach();
            return;
        }
        cx.spawn(async move |this, cx| {
            let read = api.devices().await.map_err(failure);
            let _ = this.update(cx, |this, cx| {
                this.account.devices_read(session, read);
                // A change that arrived during the read starts the next one.
                this.changed(cx);
            });
        })
        .detach();
    }

    /// A step of the event stream of a server: a change of the devices of the account,
    /// and a login of one of them, which changes its last login, reads the list of the
    /// open page again. `None` is a reader that fell behind.
    pub(super) fn account_noticed(
        &mut self,
        server_id: ServerId,
        notice: Option<&Notice>,
        cx: &mut Context<Self>,
    ) {
        let changed = match notice {
            None | Some(Notice::Reloaded { .. }) => true,
            Some(Notice::Event { event, .. }) => matches!(
                event.as_ref(),
                Event::DeviceAdded { .. }
                    | Event::DeviceRenamed { .. }
                    | Event::DeviceRemoved { .. }
                    | Event::DeviceLoggedIn { .. }
            ),
            Some(Notice::Ended { .. }) => false,
        };
        if changed && self.account.devices_changed(server_id) {
            self.follow_account(cx);
            cx.notify();
        }
        let recovered = match notice {
            None | Some(Notice::Reloaded { .. }) => true,
            Some(Notice::Event { event, .. }) => {
                matches!(event.as_ref(), Event::AccountRecovered { .. })
            }
            Some(Notice::Ended { .. }) => false,
        };
        if recovered
            && self
                .account
                .list_changed(server_id, AccountPage::Recoveries)
        {
            self.follow_account(cx);
            cx.notify();
        }
    }

    /// Reads the recoveries again after a read that failed.
    pub fn reload_recoveries(&mut self, cx: &mut Context<Self>) {
        if let Some(open) = self.account.open {
            self.account
                .list_changed(open.server_id, AccountPage::Recoveries);
            self.changed(cx);
        }
    }

    /// Reads the devices again after a read that failed.
    pub fn reload_devices(&mut self, cx: &mut Context<Self>) {
        if let Some(open) = self.account.open {
            self.account.devices_changed(open.server_id);
            self.changed(cx);
        }
    }

    /// Sends one request of the account layer for the account that the layer belongs to.
    /// Nothing is sent while another request of the page is on its way. While it is on
    /// its way the page shows it as sending; a refusal is stated at its control; `done`
    /// runs when the server did it and the page is still the one that asked.
    fn account_ask<T, F>(
        &mut self,
        what: AskKind,
        request: impl FnOnce(ServerApi) -> F,
        done: impl FnOnce(&mut Self, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) where
        T: 'static,
        F: Future<Output = Result<T, ApiError>> + 'static,
    {
        let Some((server, account_id)) = self
            .account
            .open
            .and_then(|open| self.client.server(open.server_id))
            .zip(self.account.account)
        else {
            return;
        };
        let Some(stamp) = self.account.begin(what) else {
            return;
        };
        let asked = request(server.api_for(account_id));
        cx.spawn(async move |this, cx| {
            let answer = asked.await;
            let _ = this.update(cx, |this, cx| {
                let refused = answer
                    .as_ref()
                    .err()
                    .map(|error| text::account::refusal(what, error));
                if this.account.settle(stamp, what, refused) && answer.is_ok() {
                    done(this, cx);
                }
                this.changed(cx);
            });
        })
        .detach();
        cx.notify();
    }

    /// States a refusal of the client at the control of `what`, without a request.
    fn refuse_account_ask(&mut self, what: AskKind, reason: &str, cx: &mut Context<Self>) {
        if let Some(stamp) = self.account.begin(what) {
            self.account.settle(stamp, what, Some(reason.to_owned()));
        }
        cx.notify();
    }

    /// Leaves a workspace of the server of the layer (ADR 0121). After the last workspace
    /// the account is inactive: the server ends its sessions, and a check of the server
    /// moves it to that state at once.
    pub fn leave_workspace(&mut self, workspace_id: WorkspaceId, cx: &mut Context<Self>) {
        let Some(server_id) = self.account.open.map(|open| open.server_id) else {
            return;
        };
        self.account_ask(
            AskKind::LeaveWorkspace(workspace_id),
            move |api| async move { api.leave_workspace(workspace_id).await },
            move |this, cx| {
                if let Some(server) = this.client.server(server_id) {
                    Self::run(server.dispatch_when_free(AccessAction::Check), cx);
                }
            },
            cx,
        );
    }

    /// Deletes the account on the server of the layer (ADR 0121). The key of this device
    /// no longer belongs to an account, so the server leaves the book with its key.
    pub fn delete_account(&mut self, cx: &mut Context<Self>) {
        let Some((server, account_id)) = self
            .account
            .open
            .and_then(|open| self.client.server(open.server_id))
            .zip(self.account.account)
        else {
            return;
        };
        let what = AskKind::DeleteAccount;
        let Some(stamp) = self.account.begin(what) else {
            return;
        };
        let owner = junto_client_core::book::Owner {
            entry: server.entry().key(),
            account_id,
        };
        self.pending.deleting.insert(owner);
        let server_id = server.id();
        let api = server.api_for(account_id);
        let removing = self.client.remove_server_when_free(server_id);
        cx.spawn(async move |this, cx| match api.delete_account().await {
            Ok(()) => {
                let marked = this
                    .update(cx, |this, cx| {
                        let current = this
                            .client
                            .server(server_id)
                            .is_some_and(|server| server.entry().key() == owner.entry);
                        let marked = if current {
                            if this.account.account == Some(account_id)
                                && this.account.settle(stamp, what, None)
                            {
                                this.account.close();
                            }
                            this.pending.begin_removal(owner.entry)
                        } else {
                            false
                        };
                        this.changed(cx);
                        marked
                    })
                    .unwrap_or(false);
                let removed = cx.background_executor().spawn(removing).await;
                let _ = this.update(cx, |this, cx| {
                    this.pending.deleting.remove(&owner);
                    if marked {
                        this.removed_entry(owner.entry, removed, cx);
                    }
                    this.changed(cx);
                });
            }
            Err(error) => {
                let _ = this.update(cx, |this, cx| {
                    this.pending.deleting.remove(&owner);
                    if this
                        .client
                        .server(server_id)
                        .is_some_and(|server| server.entry().key() == owner.entry)
                        && this.account.account == Some(account_id)
                    {
                        this.account.settle(
                            stamp,
                            what,
                            Some(text::account::refusal(what, &error)),
                        );
                    }
                    this.changed(cx);
                });
            }
        })
        .detach();
        cx.notify();
    }

    /// Sets the name of the account on the server of the layer (ADR 0119). The name is
    /// checked by the rule of the server before anything is sent.
    pub fn save_account_name(&mut self, name: &str, cx: &mut Context<Self>) {
        match profile::account_name(name) {
            Err(invalid) => {
                let reason = text::account::invalid_name(&invalid);
                self.refuse_account_ask(AskKind::SaveName, reason, cx);
            }
            Ok(name) => self.account_ask(
                AskKind::SaveName,
                move |api| async move { api.update_profile(&name).await },
                |_, _| {},
                cx,
            ),
        }
    }

    /// Sends the file at `path` as the picture of the account. The file is read on a
    /// background thread, and its size and its first bytes are checked before a request:
    /// a file that no server stores is not sent. Nothing is read or sent while another
    /// request of the page is on its way.
    pub fn set_account_picture(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let what = AskKind::SetPicture;
        let Some((server, account_id)) = self
            .account
            .open
            .and_then(|open| self.client.server(open.server_id))
            .zip(self.account.account)
        else {
            return;
        };
        let Some(stamp) = self.account.begin(what) else {
            return;
        };
        let api = server.api_for(account_id);
        let reading = cx
            .background_executor()
            .spawn(async move { read_picture(&path) });
        cx.spawn(async move |this, cx| {
            let refused = match reading.await {
                Err(reason) => Some(reason.to_owned()),
                Ok(bytes) => match api.set_avatar(bytes).await {
                    Ok(_) => None,
                    Err(ApiError::Avatar(invalid)) => {
                        Some(text::account::invalid_picture(invalid).to_owned())
                    }
                    Err(error) => Some(text::account::refusal(what, &error)),
                },
            };
            let _ = this.update(cx, |this, cx| {
                this.account.settle(stamp, what, refused);
                this.changed(cx);
            });
        })
        .detach();
        cx.notify();
    }

    pub fn remove_account_picture(&mut self, cx: &mut Context<Self>) {
        self.account_ask(
            AskKind::RemovePicture,
            |api| async move { api.remove_avatar().await },
            |_, _| {},
            cx,
        );
    }

    /// Gives a device of the account a name. An empty name is a name.
    pub fn rename_device(&mut self, device_id: DeviceId, name: &str, cx: &mut Context<Self>) {
        let what = AskKind::RenameDevice(device_id);
        match profile::device_name(name) {
            Err(invalid) => {
                let reason = text::account::invalid_name(&invalid);
                self.refuse_account_ask(what, reason, cx);
            }
            Ok(name) => self.account_ask(
                what,
                move |api| async move { api.rename_device(device_id, name).await },
                |this, _| this.account.stale = true,
                cx,
            ),
        }
    }

    /// Removes a device of the account. Its sessions end (ADR 0112).
    pub fn remove_device(&mut self, device_id: DeviceId, cx: &mut Context<Self>) {
        self.account_ask(
            AskKind::RemoveDevice(device_id),
            move |api| async move { api.remove_device(device_id).await },
            |this, _| this.account.stale = true,
            cx,
        );
    }

    /// Runs one action of the flow that approves a new device. Its result is in the
    /// state of the flow, which belongs to the open page.
    fn run_account_link(&mut self, action: ApproveLinkAction, cx: &mut Context<Self>) {
        let Some(flow) = self.account.link.clone() else {
            return;
        };
        let session = self.account.session;
        let running = flow.dispatch(action);
        cx.spawn(async move |this, cx| {
            let _ = running.await;
            let _ = this.update(cx, |this, cx| {
                // An approval adds a device: the list is read again also when the event
                // of the server was missed.
                if this.account.session == session
                    && matches!(flow.view().state, ApproveLinkState::Approved { .. })
                {
                    this.account.stale = true;
                }
                this.changed(cx);
            });
        })
        .detach();
        cx.notify();
    }

    /// Asks the server for the key and the device name behind a link code (ADR 0112).
    /// The server reads a code without regard to case, hyphens and spaces (ADR 0110).
    pub fn look_up_link(&mut self, code: &str, cx: &mut Context<Self>) {
        let code: String = code.trim().chars().take(LONGEST_TYPED_CODE).collect();
        self.run_account_link(
            ApproveLinkAction::LookUp {
                code: LinkCode::new(code),
            },
            cx,
        );
    }

    /// Approves the key that the page shows.
    pub fn approve_link(&mut self, cx: &mut Context<Self>) {
        self.run_account_link(ApproveLinkAction::Approve, cx);
    }

    /// Finds out whether an approval without an answer took effect.
    pub fn check_link(&mut self, cx: &mut Context<Self>) {
        self.run_account_link(ApproveLinkAction::Check, cx);
    }

    /// Ends the link that the page shows and returns to the entry of a code. The flow
    /// and the code that it held are dropped.
    pub fn reset_link(&mut self, cx: &mut Context<Self>) {
        let server = self
            .account
            .open
            .and_then(|open| self.client.server(open.server_id));
        self.account.link = server.map(|server| server.approve_link());
        #[cfg(feature = "test-support")]
        {
            self.account.link_fixture = None;
        }
        cx.notify();
    }

    /// Draws these devices, this state of a request and this state of the link in place
    /// of what a server told. The screen tool uses it.
    #[cfg(feature = "test-support")]
    pub fn show_account_fixture(
        &mut self,
        devices: Option<Result<Vec<Device>, String>>,
        ask: Option<Ask>,
        link: Option<LinkFlowView>,
        cx: &mut Context<Self>,
    ) {
        let session = self.account.session;
        self.account.stale = false;
        if let Some(read) = devices {
            self.account.devices_read(session, read);
        }
        self.account.ask = ask;
        self.account.link_fixture = link;
        self.changed(cx);
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
pub(crate) mod tests {
    use std::sync::Arc;

    use junto_client_core::Reach;
    use junto_client_core::flow::access::AccessState;
    use junto_client_core::flow::approve_link::PendingApproval;
    use junto_client_core::live::StreamStatus;
    use junto_client_core::model::ServerModel;
    use junto_client_core::profile::InvalidAvatar;
    use junto_client_core::protocol::account::{Avatar, ImageType, OwnAccount};
    use junto_client_core::protocol::error::ErrorCode;
    use junto_client_core::protocol::events::Cursor;
    use junto_client_core::protocol::keys::PublicKey;
    use junto_client_core::protocol::snapshot::Snapshot;

    use super::*;

    #[test]
    fn the_last_workspace_leaves_the_server_and_the_owner_keeps_it() {
        let workspace = |name: &str| DepartureWorkspace {
            id: WorkspaceId::generate(),
            name: name.to_owned(),
        };
        let (studio, lab) = (workspace("Studio"), workspace("Lab"));
        let two = DeparturePage {
            workspaces: vec![studio.clone(), lab.clone()],
            owner: true,
        };
        assert!(!two.leaves_the_server(studio.id));
        assert!(
            two.may_leave(studio.id),
            "the owner stays a member of another"
        );
        let one = DeparturePage {
            workspaces: vec![lab.clone()],
            owner: false,
        };
        assert!(one.leaves_the_server(lab.id));
        assert!(one.may_leave(lab.id));
        let owner = DeparturePage { owner: true, ..one };
        assert!(!owner.may_leave(lab.id));
        assert!(!owner.leaves_the_server(studio.id));
    }

    pub(crate) fn at() -> OffsetDateTime {
        OffsetDateTime::from_unix_timestamp(1_791_626_640).unwrap()
    }

    pub(crate) fn server(server_id: ServerId, account_id: AccountId, name: &str) -> ServerSnapshot {
        let snapshot = Snapshot {
            cursor: Cursor {
                epoch: uuid(1),
                position: 1,
            },
            account: OwnAccount {
                id: account_id,
                name: name.to_owned(),
                avatar: None,
                is_owner: false,
                server_role_ids: Vec::new(),
                server_permissions: Vec::new(),
                workspace_ids: Vec::new(),
            },
            workspaces: Vec::new(),
            own_call: None,
            direct_calls: Vec::new(),
            call_invitations: Vec::new(),
            sent_call_invitations: Vec::new(),
            call_moves: Vec::new(),
            entry_requests: Vec::new(),
            direct_mutes: Vec::new(),
            messages: Vec::new(),
            threads: Vec::new(),
            threads_more: false,
            threads_next: None,
            notifications: Vec::new(),
            notification_settings: Vec::new(),
        };
        ServerSnapshot {
            id: server_id,
            name: "Acme Studio".to_owned(),
            address: "acme.example".to_owned(),
            keep_connected: true,
            reach: Reach::Unknown,
            access: AccessState::LoggedIn {
                account_id,
                device_id: DeviceId::generate(),
                session_expires_at: at(),
                recoveries: Vec::new(),
                previous_account: None,
            },
            in_progress: None,
            error: None,
            stream: StreamStatus::Live,
            model: Some(Arc::new(ServerModel::from_snapshot(
                crate::state::fixtures::stated(snapshot),
            ))),
        }
    }

    fn uuid<T: std::str::FromStr>(number: u32) -> T
    where
        T::Err: std::fmt::Debug,
    {
        format!("0198f1c2-7d3a-7b1e-9c55-{number:012x}")
            .parse()
            .unwrap()
    }

    fn device(name: &str, current: bool) -> Device {
        Device {
            id: DeviceId::generate(),
            name: name.to_owned(),
            public_key: PublicKey::from_bytes([3; 32]),
            origin: DeviceOrigin::Invite,
            created_at: at(),
            last_login_at: current.then(at),
            current,
        }
    }

    fn open(server_id: ServerId, page: AccountPage) -> AccountOpen {
        AccountOpen { server_id, page }
    }

    fn pending(account_id: AccountId) -> PendingApproval {
        PendingApproval {
            code: LinkCode::new("ABCD-EFGH"),
            public_key: PublicKey::from_bytes([7; 32]),
            device_name: "Junto on Linux".to_owned(),
            expires_at: at(),
            account_id,
        }
    }

    fn flow(state: ApproveLinkState) -> LinkFlowView {
        FlowView {
            state,
            in_progress: None,
            actions: Vec::new(),
            error: None,
        }
    }

    #[test]
    fn an_answer_belongs_to_the_page_and_to_the_request_that_asked() {
        let server_id = ServerId::generate();
        let mut state = AccountState::default();
        state.show(open(server_id, AccountPage::Profile));
        let first = state.begin(AskKind::RemovePicture).unwrap();
        // The same page shown again keeps its request.
        state.show(open(server_id, AccountPage::Profile));
        assert_eq!(
            state.ask.as_ref().map(|ask| &ask.state),
            Some(&AskState::Sending)
        );
        // The answer to the request of the page changes what the page tells.
        assert!(state.settle(first, AskKind::RemovePicture, Some("No.".to_owned())));
        assert_eq!(
            state.ask,
            Some(Ask {
                what: AskKind::RemovePicture,
                state: AskState::Refused("No.".to_owned())
            })
        );

        // Another page: the refusal of the page before is put away, and its late answers
        // are for nobody.
        let third = state.begin(AskKind::SaveName).unwrap();
        state.show(open(server_id, AccountPage::Devices));
        assert_eq!(state.ask, None);
        assert!(!state.settle(third, AskKind::SaveName, Some("No.".to_owned())));
        assert_eq!(state.ask, None);

        // A closed layer takes no answer, and neither does the layer that is opened next.
        let removal = state
            .begin(AskKind::RemoveDevice(DeviceId::generate()))
            .unwrap();
        state.close();
        assert!(!state.settle(removal, AskKind::SaveName, None));
        state.show(open(server_id, AccountPage::Devices));
        assert!(!state.settle(removal, AskKind::SaveName, Some("No.".to_owned())));
        assert_eq!(state.ask, None);

        // The page of another server takes no answer of the first one.
        let asked = state.begin(AskKind::SaveName).unwrap();
        state.show(open(ServerId::generate(), AccountPage::Devices));
        assert!(!state.settle(asked, AskKind::SaveName, Some("No.".to_owned())));
        assert_eq!(state.ask, None);
    }

    #[test]
    fn the_layer_belongs_to_the_account_that_it_was_opened_for() {
        let server_id = ServerId::generate();
        let (first, second) = (AccountId::generate(), AccountId::generate());
        let mut state = AccountState::default();
        // A closed layer follows nothing.
        assert!(!state.follow_account(Some(first)));

        state.show(open(server_id, AccountPage::Devices));
        assert!(!state.follow_account(Some(first)));
        state.devices_read(state.session, Ok(vec![device("Laptop", true)]));
        let asked = state.begin(AskKind::SaveName).unwrap();
        assert!(!state.follow_account(Some(first)));
        assert!(state.open.is_some() && state.devices.is_some());

        // Another account on the same server: the layer closes with what it held.
        assert!(state.follow_account(Some(second)));
        assert_eq!(state.open, None);
        assert_eq!(state.devices, None);
        assert_eq!(state.ask, None);
        assert!(!state.settle(asked, AskKind::SaveName, Some("No.".to_owned())));

        // The server shows no account any more: the layer closes too.
        state.show(open(server_id, AccountPage::Profile));
        assert!(!state.follow_account(Some(second)));
        assert!(state.follow_account(None));
        assert_eq!(state.open, None);

        // The view is absent for a server that shows another account than the bound one.
        state.show(open(server_id, AccountPage::Profile));
        assert!(!state.follow_account(Some(first)));
        let servers = [server(server_id, second, "Bruno")];
        assert_eq!(derive(&servers, &state, None), None);
        let servers = [server(server_id, first, "Ada")];
        assert!(derive(&servers, &state, None).is_some());
        // A server that left the server book has no layer.
        assert_eq!(derive(&[], &state, None), None);
    }

    #[test]
    fn the_profile_page_shows_what_the_server_holds() {
        let (server_id, account_id) = (ServerId::generate(), AccountId::generate());
        let mut state = AccountState::default();
        assert_eq!(
            derive(&[server(server_id, account_id, "Ada")], &state, None),
            None
        );
        state.show(open(server_id, AccountPage::Profile));
        let mut shown = server(server_id, account_id, "Ada\u{7}\n Lovelace ");
        let view = derive(std::slice::from_ref(&shown), &state, None).unwrap();
        assert_eq!(view.account_id, account_id);
        assert_eq!(view.server_name, "Acme Studio");
        assert_eq!(view.selected, AccountPage::Profile);
        // The name is text of the server: no control character reaches the window.
        assert_eq!(
            view.page,
            PageView::Profile(ProfilePage {
                name: "Ada Lovelace".to_owned(),
                picture: None,
            })
        );

        let mut model = ServerModel::clone(shown.model.as_ref().unwrap());
        model.account.avatar = Some(Avatar {
            hash: "ab".repeat(32),
            media_type: ImageType::Png,
            bytes: 2048,
            width: 64,
            height: 48,
        });
        shown.model = Some(Arc::new(model));
        let view = derive(&[shown], &state, None).unwrap();
        let PageView::Profile(page) = view.page else {
            panic!("the profile page is open");
        };
        assert_eq!(
            page.picture,
            Some(Picture {
                hash: "ab".repeat(32),
                bytes: 2048,
                width: 64,
                height: 48,
            })
        );
    }

    #[test]
    fn a_request_is_told_at_its_control_only() {
        let (server_id, account_id) = (ServerId::generate(), AccountId::generate());
        let mut state = AccountState::default();
        state.show(open(server_id, AccountPage::Profile));
        let stamp = state.begin(AskKind::SetPicture).unwrap();
        let servers = [server(server_id, account_id, "Ada")];
        let view = derive(&servers, &state, None).unwrap();
        assert!(view.sending(AskKind::SetPicture) && view.busy());
        assert!(!view.sending(AskKind::SaveName));
        assert_eq!(view.refused(AskKind::SetPicture), None);
        state.settle(stamp, AskKind::SetPicture, Some("Too large.".to_owned()));
        let view = derive(&servers, &state, None).unwrap();
        assert_eq!(view.refused(AskKind::SetPicture), Some("Too large."));
        assert_eq!(view.refused(AskKind::SaveName), None);
        assert_eq!(view.refused(AskKind::RemovePicture), None);
        assert!(!view.sending(AskKind::SetPicture) && !view.busy());
    }

    #[test]
    fn the_devices_are_listed_as_the_server_gave_them() {
        let (server_id, account_id) = (ServerId::generate(), AccountId::generate());
        let servers = [server(server_id, account_id, "Ada")];
        let mut state = AccountState::default();
        state.show(open(server_id, AccountPage::Devices));
        state.follow_account(Some(account_id));
        let devices = |state: &AccountState| match derive(&servers, state, None).unwrap().page {
            PageView::Devices(page) => page,
            PageView::Profile(_) | PageView::Recoveries(_) | PageView::Departure(_) => {
                panic!("the devices page is open")
            }
        };
        assert_eq!(devices(&state).list, DeviceList::Reading);
        assert_eq!(devices(&state).link, None);

        // A first read that failed is stated in place of the rows.
        assert!(state.takes_read());
        state.devices_read(state.session, Err("No answer.".to_owned()));
        assert_eq!(
            devices(&state).list,
            DeviceList::Failed("No answer.".to_owned())
        );

        // One device: it is the only one, and it cannot be removed.
        let own = device("Laptop\u{1b}[2J", true);
        state.devices_read(state.session, Ok(vec![own.clone()]));
        let DeviceList::Listed {
            rows,
            only,
            hidden,
            failed,
        } = devices(&state).list
        else {
            panic!("the devices are listed");
        };
        assert!(only);
        assert_eq!((hidden, failed), (0, None));
        assert_eq!(
            rows,
            [DeviceLine {
                id: own.id,
                name: "Laptop[2J".to_owned(),
                this_device: true,
                origin: DeviceOrigin::Invite,
                added: at(),
                last_login: Some(at()),
            }]
        );

        // Two devices, in the order of the server. A device without a name has an empty
        // name; the page names it.
        let other = device("", false);
        state.devices_read(state.session, Ok(vec![own.clone(), other.clone()]));
        let DeviceList::Listed { rows, only, .. } = devices(&state).list else {
            panic!("the devices are listed");
        };
        assert!(!only);
        assert_eq!(rows.len(), 2);
        assert_eq!(
            (rows[1].id, rows[1].name.as_str(), rows[1].this_device),
            (other.id, "", false)
        );
        assert_eq!(rows[1].last_login, None);

        // A later read that failed keeps the rows of the read before and states the
        // failure.
        state.devices_read(state.session, Err("No answer.".to_owned()));
        let DeviceList::Listed { rows, failed, .. } = devices(&state).list else {
            panic!("the devices are listed");
        };
        assert_eq!(rows.len(), 2);
        assert_eq!(failed.as_deref(), Some("No answer."));

        // An answer of a read for a page that is gone is dropped.
        let before = state.session;
        state.show(open(server_id, AccountPage::Profile));
        state.show(open(server_id, AccountPage::Devices));
        state.devices_read(before, Ok(vec![own]));
        assert_eq!(devices(&state).list, DeviceList::Reading);
    }

    #[test]
    fn a_server_that_lists_very_many_devices_fills_a_bounded_list() {
        let (server_id, account_id) = (ServerId::generate(), AccountId::generate());
        let mut state = AccountState::default();
        state.show(open(server_id, AccountPage::Devices));
        state.follow_account(Some(account_id));
        let many: Vec<Device> = (0..MOST_DEVICES + 30)
            .map(|_| device("Phone", false))
            .collect();
        state.devices_read(state.session, Ok(many));
        assert_eq!(state.devices.as_ref().map(Vec::len), Some(MOST_DEVICES));
        let servers = [server(server_id, account_id, "Ada")];
        let PageView::Devices(page) = derive(&servers, &state, None).unwrap().page else {
            panic!("the devices page is open");
        };
        let DeviceList::Listed {
            rows, only, hidden, ..
        } = page.list
        else {
            panic!("the devices are listed");
        };
        assert_eq!((rows.len(), hidden, only), (MOST_DEVICES, 30, false));
    }

    #[test]
    fn the_recoveries_are_listed_the_newest_first_with_what_this_device_has_seen() {
        let (server_id, account_id) = (ServerId::generate(), AccountId::generate());
        let mut snapshot = server(server_id, account_id, "Ada");
        let this_device = DeviceId::generate();
        let recovery = |minutes: i64, device_id: Option<DeviceId>, by_host: bool| RecoveryNotice {
            id: junto_client_core::protocol::ids::RecoveryId::generate(),
            device_id,
            performed_by: (!by_host).then(AccountId::generate),
            performed_at: at() + time::Duration::minutes(minutes),
        };
        let older = recovery(1, None, true);
        let newer = recovery(2, Some(this_device), false);
        snapshot.access = AccessState::LoggedIn {
            account_id,
            device_id: this_device,
            session_expires_at: at(),
            recoveries: vec![newer.clone()],
            previous_account: None,
        };
        let servers = [snapshot];
        let mut state = AccountState::default();
        state.show(open(server_id, AccountPage::Recoveries));
        state.follow_account(Some(account_id));
        let list = |state: &AccountState| match derive(&servers, state, None).unwrap().page {
            PageView::Recoveries(list) => list,
            _ => panic!("the recoveries page is open"),
        };
        assert_eq!(list(&state), RecoveryList::Reading);

        // The page reads once; the server lists the oldest first.
        assert!(state.takes_read());
        assert!(!state.takes_read());
        state.recoveries_read(state.session, Ok(vec![older.clone(), newer.clone()]));
        let RecoveryList::Listed {
            rows,
            hidden,
            failed,
        } = list(&state)
        else {
            panic!("the recoveries are listed");
        };
        assert_eq!((hidden, failed), (0, None));
        assert_eq!(
            rows.iter().map(|row| row.id).collect::<Vec<_>>(),
            [newer.id, older.id]
        );
        assert!(rows[0].this_device && rows[0].unseen && !rows[0].by_host);
        assert!(!rows[1].this_device && !rows[1].unseen && rows[1].by_host);

        // A recovery that the server tells reads the list again; the devices page does
        // not read for it.
        assert!(state.list_changed(server_id, AccountPage::Recoveries));
        assert!(!state.devices_changed(server_id));
        assert!(state.takes_read());
        state.recoveries_read(state.session, Err("unreachable".to_owned()));
        let RecoveryList::Listed { rows, failed, .. } = list(&state) else {
            panic!("the rows of the read before stay");
        };
        assert_eq!((rows.len(), failed.as_deref()), (2, Some("unreachable")));

        // A server that lists very many recoveries fills the layer with the newest ones.
        let many: Vec<RecoveryNotice> = (0..MOST_RECOVERIES as i64 + 3)
            .map(|minute| recovery(minute, None, false))
            .collect();
        let newest = many.last().unwrap().id;
        assert!(state.list_changed(server_id, AccountPage::Recoveries));
        assert!(state.takes_read());
        state.recoveries_read(state.session, Ok(many));
        let RecoveryList::Listed { rows, hidden, .. } = list(&state) else {
            panic!("the recoveries are listed");
        };
        assert_eq!(
            (rows.len(), hidden, rows[0].id),
            (MOST_RECOVERIES, 3, newest)
        );
    }

    #[test]
    fn the_list_is_read_once_at_a_time_and_again_after_a_change() {
        let (server_id, account_id) = (ServerId::generate(), AccountId::generate());
        let mut state = AccountState::default();
        // The profile page reads no devices, and a change of the devices is not for it.
        state.show(open(server_id, AccountPage::Profile));
        state.follow_account(Some(account_id));
        assert!(!state.takes_read());
        assert!(!state.devices_changed(server_id));
        assert!(!state.takes_read());

        // The devices page reads when it opens, once.
        state.show(open(server_id, AccountPage::Devices));
        assert!(state.takes_read());
        assert!(!state.takes_read());
        // Changes that the server tells during the read make one further read, after
        // the first one has ended.
        assert!(state.devices_changed(server_id));
        assert!(state.devices_changed(server_id));
        assert!(!state.takes_read());
        state.devices_read(state.session, Ok(Vec::new()));
        assert!(state.takes_read());
        assert!(!state.takes_read());
        state.devices_read(state.session, Ok(Vec::new()));
        assert!(!state.takes_read());

        // A change on another server reads nothing.
        assert!(!state.devices_changed(ServerId::generate()));
        assert!(!state.takes_read());
        // A read that failed is not repeated by itself.
        assert!(state.devices_changed(server_id));
        assert!(state.takes_read());
        state.devices_read(state.session, Err("No answer.".to_owned()));
        assert!(!state.takes_read());

        // A page that opens while the read of the page before is on its way reads after
        // that read has ended, and takes nothing of its answer.
        assert!(state.devices_changed(server_id));
        assert!(state.takes_read());
        let before = state.session;
        state.show(open(server_id, AccountPage::Profile));
        state.show(open(server_id, AccountPage::Devices));
        assert!(!state.takes_read());
        state.devices_read(before, Ok(vec![device("Laptop", true)]));
        assert_eq!(state.devices, None);
        assert!(state.takes_read());
        assert!(!state.takes_read());

        // Without an account nothing is read.
        let mut unbound = AccountState::default();
        unbound.show(open(server_id, AccountPage::Devices));
        assert!(!unbound.takes_read());
    }

    #[test]
    fn the_link_of_a_new_device_shows_each_state_of_its_flow() {
        let account_id = AccountId::generate();
        assert_eq!(
            link_view(&flow(ApproveLinkState::EnterCode)),
            LinkView::EnterCode {
                looking_up: false,
                error: None
            }
        );
        let mut looking = flow(ApproveLinkState::EnterCode);
        looking.in_progress = Some(ApproveLinkActionKind::LookUp);
        assert!(link_view(&looking).busy());
        let mut refused = flow(ApproveLinkState::EnterCode);
        refused.error = Some(FlowError::refused(ErrorCode::LinkCodeInvalid));
        assert_eq!(
            link_view(&refused),
            LinkView::EnterCode {
                looking_up: false,
                error: Some(text::account::link_error(&FlowError::refused(
                    ErrorCode::LinkCodeInvalid
                ))),
            }
        );

        // The key of the new device is shown as its fingerprint, and the name that the
        // new device gave as text of another party.
        let mut asked = pending(account_id);
        asked.device_name = "Junto\non Linux".to_owned();
        let confirm = link_view(&flow(ApproveLinkState::Confirm(asked.clone())));
        assert_eq!(
            confirm,
            LinkView::Confirm {
                device_name: "Juntoon Linux".to_owned(),
                fingerprint: text::account::key_fingerprint(&asked.public_key),
                expires_at: at(),
                approving: false,
                error: None,
            }
        );
        // No state of the link shows the code in its formatted form.
        assert!(!format!("{confirm:?}").contains("ABCD"));

        let mut approving = flow(ApproveLinkState::Confirm(asked.clone()));
        approving.in_progress = Some(ApproveLinkActionKind::Approve);
        assert!(link_view(&approving).busy());

        let mut unknown = flow(ApproveLinkState::OutcomeUnknown(asked.clone()));
        unknown.error = Some(FlowError::Unavailable);
        assert_eq!(
            link_view(&unknown),
            LinkView::OutcomeUnknown {
                device_name: "Juntoon Linux".to_owned(),
                fingerprint: text::account::key_fingerprint(&asked.public_key),
                checking: false,
                error: Some(text::flow_error(&FlowError::Unavailable)),
            }
        );

        let linked = link_view(&flow(ApproveLinkState::Approved {
            device: device("Junto on Linux", false),
        }));
        assert_eq!(
            linked,
            LinkView::Approved {
                device_name: "Junto on Linux".to_owned()
            }
        );
        assert!(!linked.busy());
    }

    #[test]
    fn a_file_is_checked_by_its_size_and_its_first_bytes() {
        let png = b"\x89PNG\r\n\x1a\n";
        let mut small = png.to_vec();
        small.resize(1024, 0);
        assert_eq!(profile::check_avatar(&small).err(), None);
        // 300 KiB with the first bytes of a PNG image.
        let mut large = png.to_vec();
        large.resize(300 * 1024, 0);
        assert_eq!(
            profile::check_avatar(&large).err(),
            Some(InvalidAvatar::TooLarge)
        );
        assert_eq!(
            profile::check_avatar(b"not an image").err(),
            Some(InvalidAvatar::Unsupported)
        );

        // A file is read up to one byte beyond the bound, and no further.
        let directory =
            std::env::temp_dir().join(format!("junto-client12-account-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let file = directory.join("large.png");
        let mut huge = png.to_vec();
        huge.resize(4 * profile::AVATAR_MAX_BYTES, 0);
        std::fs::write(&file, &huge).unwrap();
        assert_eq!(
            read_picture(&file),
            Err(text::account::invalid_picture(InvalidAvatar::TooLarge))
        );
        std::fs::write(&file, &small).unwrap();
        assert_eq!(read_picture(&file).map(|bytes| bytes.len()), Ok(1024));
        std::fs::write(&file, b"text").unwrap();
        assert_eq!(
            read_picture(&file),
            Err(text::account::invalid_picture(InvalidAvatar::Unsupported))
        );
        assert_eq!(
            read_picture(&directory.join("absent.png")),
            Err(text::account::PICTURE_NOT_READ)
        );
        std::fs::remove_dir_all(&directory).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn only_a_regular_file_is_read_as_a_picture() {
        let directory = tempfile::Builder::new()
            .prefix("junto-client12-fix-")
            .tempdir()
            .unwrap();
        let picture = directory.path().join("picture.png");
        std::fs::write(&picture, b"\x89PNG\r\n\x1a\n-image-").unwrap();
        assert!(read_picture(&picture).is_ok());
        // A link to a picture is not followed, and a directory is no picture.
        let link = directory.path().join("link.png");
        std::os::unix::fs::symlink(&picture, &link).unwrap();
        assert_eq!(read_picture(&link), Err(text::account::PICTURE_NOT_READ));
        assert_eq!(
            read_picture(directory.path()),
            Err(text::account::PICTURE_NOT_READ)
        );

        // A pipe that nobody writes to keeps no read waiting.
        let pipe = directory.path().join("pipe.png");
        let made = std::process::Command::new("mkfifo")
            .arg(&pipe)
            .status()
            .unwrap();
        assert!(made.success());
        let (tell, told) = std::sync::mpsc::channel();
        let reader = {
            let pipe = pipe.clone();
            std::thread::spawn(move || {
                let _ = tell.send(read_picture(&pipe));
            })
        };
        let read = told.recv_timeout(std::time::Duration::from_secs(5)).ok();
        if read.is_none() {
            // The open of the reader waits for a writer: this one lets the thread end.
            drop(std::fs::OpenOptions::new().write(true).open(&pipe));
        }
        reader.join().unwrap();
        assert_eq!(read, Some(Err(text::account::PICTURE_NOT_READ)));
    }

    #[test]
    fn a_page_sends_one_request_at_a_time() {
        let server_id = ServerId::generate();
        let mut state = AccountState::default();
        state.show(open(server_id, AccountPage::Profile));
        let name = state.begin(AskKind::SaveName).unwrap();
        // A second request of the page does not start while the first one is on its
        // way, and the first one keeps its place.
        assert_eq!(state.begin(AskKind::SetPicture), None);
        assert_eq!(state.begin(AskKind::SaveName), None);
        assert_eq!(
            state.ask,
            Some(Ask {
                what: AskKind::SaveName,
                state: AskState::Sending
            })
        );
        // What the page tells about a request that is on its way is not put away.
        state.dismiss();
        assert!(state.sending());
        // The refusal of the first request is told at its control.
        assert!(state.settle(name, AskKind::SaveName, Some("No.".to_owned())));
        assert_eq!(
            state.ask,
            Some(Ask {
                what: AskKind::SaveName,
                state: AskState::Refused("No.".to_owned())
            })
        );
        // A refusal is put away, and the next request starts.
        state.dismiss();
        assert_eq!(state.ask, None);
        let picture = state.begin(AskKind::SetPicture).unwrap();
        assert!(state.settle(picture, AskKind::SetPicture, None));
        assert!(!state.sending());

        // A page that opens while a request of the page before is on its way sends its
        // own request, and takes no answer of the page before.
        let before = state.begin(AskKind::SaveName).unwrap();
        state.show(open(server_id, AccountPage::Devices));
        let removal = AskKind::RemoveDevice(DeviceId::generate());
        assert!(state.begin(removal).is_some());
        assert!(!state.settle(before, AskKind::SaveName, Some("No.".to_owned())));
        assert_eq!(
            state.ask,
            Some(Ask {
                what: removal,
                state: AskState::Sending
            })
        );
    }

    #[test]
    fn a_name_of_a_server_is_shown_without_control_characters_and_cut() {
        assert_eq!(shown_name("Ada"), "Ada");
        assert_eq!(shown_name("  Ada\u{0}\r\n"), "Ada");
        assert_eq!(shown_name(&"x".repeat(500)).chars().count(), NAME_MAX_CHARS);
        assert_eq!(shown_name(""), "");
    }
}
