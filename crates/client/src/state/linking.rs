//! Device linking in the window (ADR 0782): the link of this device that waits on one
//! server, and the two roles of the link with one code (ADR 0104, ADR 0716). The flows of
//! the client core hold every rule; this module starts their actions and derives what the
//! window shows of their states.

use gpui::Context;
use junto_client_core::flow::access::{AccessAction, AccessState, Notice};
use junto_client_core::flow::link_device::{
    ApprovalProgress, LinkNewDeviceAction, LinkNewDeviceActionKind, LinkNewDeviceFlow,
    LinkNewDeviceState, LinkProgress, LinkThisDeviceAction, LinkThisDeviceActionKind,
    LinkThisDeviceFlow, LinkThisDeviceState, link_code_is_plain,
};
use junto_client_core::flow::{FlowError, FlowView};
use junto_client_core::live::Notice as ServerNotice;
use junto_client_core::protocol::events::Event;
use junto_client_core::protocol::ids::{AccountId, ServerId};
use junto_client_core::protocol::secret::LinkCode;
use time::OffsetDateTime;

use super::AppState;
use super::account::shown_name;
use super::view::{ServerSnapshot, shown_address, shown_model};
use crate::text;

/// The link of this device on one server that waits for the approval of a logged-in
/// device (ADR 0112).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkWait {
    /// The code that the person types on the logged-in device. Absent when the server
    /// gave a code in another form than a link code has: such a code is not shown.
    pub code: Option<LinkCode>,
    /// The key of this device for the server, as the logged-in device shows it.
    pub fingerprint: String,
    pub expires_at: OffsetDateTime,
}

/// What a server that waits for a link shows.
pub fn link_wait(server: &ServerSnapshot) -> Option<LinkWait> {
    match &server.access {
        AccessState::WaitingForLink {
            code,
            public_key,
            expires_at,
        } => Some(LinkWait {
            code: link_code_is_plain(code).then(|| code.clone()),
            fingerprint: text::account::key_fingerprint(public_key),
            expires_at: *expires_at,
        }),
        _ => None,
    }
}

/// What this device does in a link.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkRole {
    /// It has the accounts: it takes the code of the new device and approves it.
    FirstDevice,
    /// It is new: it asks one server for a code.
    NewDevice,
}

/// One server of this device in the choice of a link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChoiceLine {
    pub server_id: ServerId,
    pub server: String,
    pub checked: bool,
    /// The link starts on this server, so it stays checked.
    pub locked: bool,
    /// Why the server cannot be linked now.
    pub unavailable: Option<String>,
}

/// Where the link of the new device on one server stands, as its line states it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApprovalShown {
    Waiting,
    Working,
    Approved,
    AlreadyLinked,
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalLine {
    pub server_id: ServerId,
    pub server: String,
    /// The account that the new device is added to: its name when the state of the
    /// server holds it, and its identifier otherwise.
    pub account: String,
    pub progress: ApprovalShown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FirstStep {
    EnterCode {
        choices: Vec<ChoiceLine>,
    },
    Confirm {
        device_name: String,
        /// The key of the new device on the first server, as the new device shows it.
        device_code: String,
        choices: Vec<ChoiceLine>,
    },
    Linking {
        device_name: String,
        lines: Vec<ApprovalLine>,
        /// No server waits for the new device any more.
        done: bool,
    },
}

/// The link as the device that approves shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FirstDeviceView {
    pub step: FirstStep,
    /// An action of the flow runs.
    pub busy: bool,
    pub error: Option<String>,
}

/// Where the link of this device on one server stands, as its line states it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkShown {
    Working,
    Waiting,
    Linked,
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkLine {
    pub server_id: ServerId,
    /// The name of the server when this device has saved it, and its address otherwise.
    pub server: String,
    pub progress: LinkShown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NewStep {
    EnterAddress,
    /// The first server waits for the approval; its code and the key of this device.
    Waiting {
        server: String,
        wait: Option<LinkWait>,
    },
    Linked {
        server: String,
        lines: Vec<LinkLine>,
        /// The offer of the other device was read, or the first server holds none.
        offer_read: bool,
        done: bool,
    },
}

/// The link as the new device shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewDeviceView {
    pub step: NewStep,
    pub busy: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkAllView {
    First(FirstDeviceView),
    New(NewDeviceView),
}

/// The flow of the device that approves, as the window reads it.
pub type ApprovingView = FlowView<LinkNewDeviceState, LinkNewDeviceActionKind>;
/// The flow of the new device, as the window reads it.
pub type RequestingView = FlowView<LinkThisDeviceState, LinkThisDeviceActionKind>;

/// The name of an account on a server: the name that the state of the server holds for
/// the account of this device, and the identifier for another account.
fn account_name(server: Option<&ServerSnapshot>, account_id: AccountId) -> String {
    server
        .and_then(shown_model)
        .filter(|model| model.account.id == account_id)
        .map(|model| shown_name(&model.account.name))
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| account_id.to_string())
}

fn server_name(servers: &[ServerSnapshot], server_id: ServerId, fallback: &str) -> String {
    servers
        .iter()
        .find(|server| server.id == server_id)
        .map_or_else(|| shown_name(fallback), |server| shown_name(&server.name))
}

/// What the device that approves shows, derived without a side effect.
pub fn first_device(servers: &[ServerSnapshot], flow: &ApprovingView) -> FirstDeviceView {
    let choices = |list: &[junto_client_core::flow::link_device::ServerChoice],
                   first: Option<ServerId>| {
        list.iter()
            .map(|choice| ChoiceLine {
                server_id: choice.server_id,
                server: server_name(servers, choice.server_id, &choice.name),
                checked: choice.checked,
                locked: Some(choice.server_id) == first,
                unavailable: choice.unavailable.as_ref().map(text::account::link_error),
            })
            .collect::<Vec<_>>()
    };
    let step = match &flow.state {
        LinkNewDeviceState::EnterCode { servers: list } => FirstStep::EnterCode {
            choices: choices(list, None),
        },
        LinkNewDeviceState::Confirm {
            servers: list,
            first,
            pending,
        } => FirstStep::Confirm {
            device_name: shown_name(&pending.device_name),
            device_code: text::account::key_fingerprint(&pending.public_key),
            choices: choices(list, Some(*first)),
        },
        LinkNewDeviceState::Linking {
            device,
            servers: approvals,
            ..
        } => {
            let lines: Vec<ApprovalLine> = approvals
                .iter()
                .map(|approval| {
                    let server = servers
                        .iter()
                        .find(|server| server.id == approval.server_id);
                    ApprovalLine {
                        server_id: approval.server_id,
                        server: server_name(servers, approval.server_id, &approval.name),
                        account: account_name(server, approval.account_id),
                        progress: match &approval.progress {
                            ApprovalProgress::Waiting => ApprovalShown::Waiting,
                            ApprovalProgress::Working => ApprovalShown::Working,
                            ApprovalProgress::Approved => ApprovalShown::Approved,
                            ApprovalProgress::AlreadyLinked => ApprovalShown::AlreadyLinked,
                            ApprovalProgress::Failed(error) => {
                                ApprovalShown::Failed(text::account::link_error(error))
                            }
                        },
                    }
                })
                .collect();
            FirstStep::Linking {
                device_name: shown_name(&device.name),
                done: !lines.iter().any(|line| {
                    matches!(
                        line.progress,
                        ApprovalShown::Waiting | ApprovalShown::Working
                    )
                }),
                lines,
            }
        }
    };
    FirstDeviceView {
        step,
        busy: flow.in_progress.is_some(),
        error: flow.error.as_ref().map(text::account::link_error),
    }
}

/// The line of one server on the new device. The application checks a link that waits
/// by itself (ADR 0730), so the state of the server tells an approval before the flow
/// is asked.
fn link_shown(progress: &LinkProgress, server: Option<&ServerSnapshot>) -> LinkShown {
    let access = server.map(|server| &server.access);
    match (progress, access) {
        (LinkProgress::Working, _) => LinkShown::Working,
        (LinkProgress::Linked, _) | (_, Some(AccessState::LoggedIn { .. })) => LinkShown::Linked,
        (
            LinkProgress::Waiting,
            Some(AccessState::NoAccount {
                notice: Some(Notice::LinkExpired),
            }),
        ) => LinkShown::Failed(text::account::link_error(&FlowError::Expired)),
        (LinkProgress::Waiting, _) => LinkShown::Waiting,
        (LinkProgress::Failed(error), _) => LinkShown::Failed(text::account::link_error(error)),
    }
}

/// What the new device shows, derived without a side effect.
pub fn new_device(servers: &[ServerSnapshot], flow: &RequestingView) -> NewDeviceView {
    let step = match &flow.state {
        LinkThisDeviceState::EnterAddress => NewStep::EnterAddress,
        LinkThisDeviceState::Waiting { first } => {
            let server = servers.iter().find(|server| server.id == *first);
            NewStep::Waiting {
                server: server.map_or_else(String::new, |server| shown_name(&server.name)),
                wait: server.and_then(link_wait),
            }
        }
        LinkThisDeviceState::Linked {
            first,
            servers: links,
            offer_read,
        } => {
            let lines: Vec<LinkLine> = links
                .iter()
                .map(|link| {
                    let server = servers.iter().find(|server| server.id == link.server_id);
                    LinkLine {
                        server_id: link.server_id,
                        server: match server {
                            Some(server) => shown_name(&server.name),
                            None => shown_address(link.address.as_str()),
                        },
                        progress: link_shown(&link.progress, server),
                    }
                })
                .collect();
            NewStep::Linked {
                server: server_name(servers, *first, ""),
                done: *offer_read && lines.iter().all(|line| line.progress == LinkShown::Linked),
                offer_read: *offer_read,
                lines,
            }
        }
    };
    NewDeviceView {
        step,
        busy: flow.in_progress.is_some(),
        error: flow.error.as_ref().map(text::account::link_error),
    }
}

/// What the layer of a link holds: the role of this device and its flow. The flow is
/// dropped with the layer.
#[derive(Default)]
pub struct LinkingState {
    open: Option<LinkRole>,
    approving: Option<LinkNewDeviceFlow>,
    requesting: Option<LinkThisDeviceFlow>,
    #[cfg(feature = "test-support")]
    fixture: Option<LinkAllView>,
}

impl AppState {
    /// What a server of the server book that waits for a device link shows.
    pub fn link_wait(&self, server_id: ServerId) -> Option<LinkWait> {
        self.snapshots()
            .iter()
            .find(|server| server.id == server_id)
            .and_then(link_wait)
    }

    /// Asks a server for a link code for this device (ADR 0112). A new request replaces
    /// the code of the request before. The result is in the state of the server.
    pub fn request_link(&mut self, server_id: ServerId, cx: &mut Context<Self>) {
        if let Some(server) = self.client.server(server_id) {
            let action = AccessAction::RequestLink {
                device_name: text::device_name(),
            };
            Self::run(server.dispatch_when_free(action), cx);
        }
    }

    pub fn link_all_role(&self) -> Option<LinkRole> {
        self.linking.open
    }

    /// Opens a link in one role. With `only`, the device that approves starts with only
    /// that server checked. A flow of the layer before is dropped.
    pub fn open_link_all(
        &mut self,
        role: LinkRole,
        only: Option<ServerId>,
        cx: &mut Context<Self>,
    ) {
        let first = role == LinkRole::FirstDevice;
        self.linking = LinkingState {
            open: Some(role),
            approving: first.then(|| self.client.link_new_device(only)),
            requesting: (!first).then(|| self.client.link_this_device()),
            #[cfg(feature = "test-support")]
            fixture: None,
        };
        cx.notify();
    }

    /// Closes the layer of a link. An action that runs goes to its end in the client
    /// core, and each server shows its own state afterwards.
    pub fn close_link_all(&mut self, cx: &mut Context<Self>) {
        self.linking = LinkingState::default();
        cx.notify();
    }

    /// The link as the window shows it.
    pub fn link_all_view(&self) -> Option<LinkAllView> {
        let role = self.linking.open?;
        #[cfg(feature = "test-support")]
        if let Some(fixture) = &self.linking.fixture {
            return Some(fixture.clone());
        }
        let servers = self.snapshots();
        Some(match role {
            LinkRole::FirstDevice => LinkAllView::First(first_device(
                &servers,
                &self.linking.approving.as_ref()?.view(),
            )),
            LinkRole::NewDevice => LinkAllView::New(new_device(
                &servers,
                &self.linking.requesting.as_ref()?.view(),
            )),
        })
    }

    fn run_approving(&mut self, action: LinkNewDeviceAction, cx: &mut Context<Self>) {
        if let Some(flow) = &self.linking.approving {
            Self::run(flow.dispatch(action), cx);
            cx.notify();
        }
    }

    fn run_requesting(&mut self, action: LinkThisDeviceAction, cx: &mut Context<Self>) {
        if let Some(flow) = &self.linking.requesting {
            Self::run(flow.dispatch(action), cx);
            cx.notify();
        }
    }

    /// Checks or unchecks a server for the link of the new device.
    pub fn choose_link_server(
        &mut self,
        server_id: ServerId,
        checked: bool,
        cx: &mut Context<Self>,
    ) {
        self.run_approving(LinkNewDeviceAction::Choose { server_id, checked }, cx);
    }

    /// Looks the code of the new device up on every checked server. Nothing is approved.
    pub fn look_up_link_code(&mut self, code: String, cx: &mut Context<Self>) {
        let code = LinkCode::new(code);
        self.run_approving(LinkNewDeviceAction::LookUp { code }, cx);
    }

    /// Approves the new device on the first server, with the other checked servers.
    pub fn approve_new_device(&mut self, cx: &mut Context<Self>) {
        self.run_approving(LinkNewDeviceAction::Approve, cx);
    }

    /// Reads the answer of the new device and approves each link that it names.
    pub fn follow_new_device(&mut self, cx: &mut Context<Self>) {
        self.run_approving(LinkNewDeviceAction::Check, cx);
    }

    /// The new device asks the server at `address` for a link code.
    pub fn start_linking(&mut self, address: String, device_name: String, cx: &mut Context<Self>) {
        self.run_requesting(
            LinkThisDeviceAction::Start {
                address,
                device_name,
            },
            cx,
        );
    }

    /// The new device asks for a new code on its first server.
    pub fn new_link_code(&mut self, cx: &mut Context<Self>) {
        self.run_requesting(LinkThisDeviceAction::NewCode, cx);
    }

    /// The new device checks its link: after the approval it reads the offer, links on
    /// its servers and answers.
    pub fn check_this_device(&mut self, cx: &mut Context<Self>) {
        self.run_requesting(LinkThisDeviceAction::Check, cx);
    }

    /// Moves the flow of the new device on when the state of a server tells that it
    /// should: the first server linked this device, or a code of another server expired.
    pub(super) fn reconcile_linking(&mut self, cx: &mut Context<Self>) {
        let Some(flow) = &self.linking.requesting else {
            return;
        };
        let view = flow.view();
        if view.in_progress.is_some() || !view.actions.contains(&LinkThisDeviceActionKind::Check) {
            return;
        }
        let access = |server_id: ServerId| {
            self.client
                .server(server_id)
                .map(|server| server.access().state)
        };
        let due = match &view.state {
            LinkThisDeviceState::Waiting { first } => {
                matches!(access(*first), Some(AccessState::LoggedIn { .. }))
            }
            LinkThisDeviceState::Linked { servers, .. } => servers.iter().any(|server| {
                server.progress == LinkProgress::Waiting
                    && matches!(
                        access(server.server_id),
                        Some(AccessState::NoAccount {
                            notice: Some(Notice::LinkExpired)
                        })
                    )
            }),
            LinkThisDeviceState::EnterAddress => false,
        };
        if due {
            self.check_this_device(cx);
        }
    }

    /// A change of the relay of a link on its first server: the device that approves
    /// reads the answer of the new device (ADR 0104).
    pub(super) fn linking_noticed(
        &mut self,
        server_id: ServerId,
        notice: &ServerNotice,
        cx: &mut Context<Self>,
    ) {
        let ServerNotice::Event { event, .. } = notice else {
            return;
        };
        if !matches!(event.as_ref(), Event::DeviceLinkRelayChanged { .. }) {
            return;
        }
        if let Some(flow) = &self.linking.approving
            && let LinkNewDeviceState::Linking { first, .. } = flow.view().state
            && first == server_id
        {
            self.follow_new_device(cx);
        }
    }

    /// Draws this state of a link in place of what the flows hold. The screen tool uses
    /// it.
    #[cfg(feature = "test-support")]
    pub fn show_link_all_fixture(&mut self, view: LinkAllView, cx: &mut Context<Self>) {
        self.linking = LinkingState {
            open: Some(match view {
                LinkAllView::First(_) => LinkRole::FirstDevice,
                LinkAllView::New(_) => LinkRole::NewDevice,
            }),
            fixture: Some(view),
            ..LinkingState::default()
        };
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use junto_client_core::protocol::keys::PublicKey;

    use super::super::account::tests::{at, server};
    use super::*;

    fn waiting(code: &str) -> AccessState {
        AccessState::WaitingForLink {
            code: LinkCode::new(code),
            public_key: PublicKey::from_bytes([7; 32]),
            expires_at: at(),
        }
    }

    fn requesting(state: LinkThisDeviceState) -> RequestingView {
        FlowView {
            state,
            in_progress: None,
            actions: Vec::new(),
            error: None,
        }
    }

    #[test]
    fn a_server_that_waits_for_a_link_shows_its_code_its_key_and_its_expiry() {
        let (server_id, account_id) = (ServerId::generate(), AccountId::generate());
        let mut shown = server(server_id, account_id, "Ada");
        assert_eq!(link_wait(&shown), None);
        shown.access = waiting("K7QM-2XHD");
        let wait = link_wait(&shown).unwrap();
        assert_eq!(wait.code.as_ref().map(LinkCode::expose), Some("K7QM-2XHD"));
        assert_eq!(
            wait.fingerprint,
            text::account::key_fingerprint(&PublicKey::from_bytes([7; 32]))
        );
        assert_eq!(wait.expires_at, at());
        // The formatted form of the wait holds no code.
        assert!(!format!("{wait:?}").contains("K7QM"));

        // A code in another form than a link code has is not shown.
        for odd in ["k7qm-2xhd", "K7QM 2XHD", "", &"A".repeat(33), "K7QM\n2XHD"] {
            shown.access = waiting(odd);
            let wait = link_wait(&shown).unwrap();
            assert_eq!(wait.code, None, "{odd:?}");
            assert!(!wait.fingerprint.is_empty());
        }
    }

    #[test]
    fn the_new_device_shows_its_code_and_then_one_line_per_server() {
        let (acme, harbor) = (ServerId::generate(), ServerId::generate());
        let ada = AccountId::generate();
        let mut first = server(acme, ada, "Ada");
        first.access = waiting("K7QM-2XHD");
        let view = new_device(
            std::slice::from_ref(&first),
            &requesting(LinkThisDeviceState::Waiting { first: acme }),
        );
        let NewStep::Waiting { server, wait } = view.step else {
            panic!("no wait");
        };
        assert_eq!(server, "Acme Studio");
        assert_eq!(
            wait.and_then(|wait| wait.code)
                .as_ref()
                .map(LinkCode::expose),
            Some("K7QM-2XHD")
        );

        first.access = AccessState::LoggedIn {
            account_id: ada,
            device_id: junto_client_core::protocol::ids::DeviceId::generate(),
            session_expires_at: at(),
            recoveries: Vec::new(),
            previous_account: None,
        };
        let reading = new_device(
            std::slice::from_ref(&first),
            &requesting(LinkThisDeviceState::Linked {
                first: acme,
                servers: Vec::new(),
                offer_read: false,
            }),
        );
        assert!(matches!(
            reading.step,
            NewStep::Linked {
                offer_read: false,
                done: false,
                ..
            }
        ));

        let links = vec![junto_client_core::flow::link_device::ServerLink {
            server_id: harbor,
            address: junto_client_core::address::ServerAddress::parse("harbor.example.test")
                .unwrap(),
            progress: LinkProgress::Waiting,
        }];
        let linking = new_device(
            std::slice::from_ref(&first),
            &requesting(LinkThisDeviceState::Linked {
                first: acme,
                servers: links,
                offer_read: true,
            }),
        );
        let NewStep::Linked { lines, done, .. } = linking.step else {
            panic!("not linked");
        };
        assert!(!done);
        assert_eq!(lines[0].server, "harbor.example.test");
        assert_eq!(lines[0].progress, LinkShown::Waiting);
    }
}
