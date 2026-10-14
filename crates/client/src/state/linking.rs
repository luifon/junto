//! Device linking in the window (ADR 0782): the link of this device that waits on one
//! server, and the two roles of the link on all servers (ADR 0716). The flows of the
//! client core hold every rule; this module starts their actions and derives what the
//! window shows of their states.

use gpui::Context;
use junto_client_core::flow::access::{AccessAction, AccessState, Notice};
use junto_client_core::flow::link_all::{
    ApprovalProgress, ApproveDevicesAction, ApproveDevicesActionKind, ApproveDevicesFlow,
    ApproveDevicesState, LONGEST_TEXT, LinkDeviceAction, LinkDeviceActionKind, LinkDeviceFlow,
    LinkDeviceState, LinkOffer, LinkProgress, LinkRequestText, ServerApproval, link_code_is_plain,
};
use junto_client_core::flow::{FlowError, FlowView};
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

/// True when a text has at most the length of a link offer or a link request.
pub const fn fits(text: &str) -> bool {
    text.len() <= LONGEST_TEXT
}

/// What this device does in the link on all servers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkRole {
    /// It has the accounts: it gives the offer and approves the request.
    FirstDevice,
    /// It is new: it takes the offer and gives the request.
    NewDevice,
}

/// The link of the new device on one server, as the first device holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Approval {
    pub server_id: ServerId,
    /// The name of the server in the book of this device. Empty for a server that it
    /// has not saved.
    pub name: String,
    /// The account of this device on the server that the new device is added to.
    pub account_id: Option<AccountId>,
    pub progress: ApprovalProgress,
}

impl From<&ServerApproval> for Approval {
    fn from(approval: &ServerApproval) -> Self {
        Self {
            server_id: approval.server_id,
            name: approval.name.clone(),
            account_id: approval.account_id,
            progress: approval.progress.clone(),
        }
    }
}

/// The state of the flow of the first device, without the codes of the request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Approving {
    EnterRequest,
    Confirm {
        fingerprint: String,
        device_name: String,
        servers: Vec<Approval>,
    },
}

/// The flow of the first device as the window reads it.
pub type ApprovingView = FlowView<Approving, ApproveDevicesActionKind>;
/// The flow of the new device as the window reads it.
pub type RequestingView = FlowView<LinkDeviceState, LinkDeviceActionKind>;

fn approving(flow: &ApproveDevicesFlow) -> ApprovingView {
    let view = flow.view();
    FlowView {
        state: match &view.state {
            ApproveDevicesState::EnterRequest => Approving::EnterRequest,
            ApproveDevicesState::Confirm {
                fingerprint,
                device_name,
                servers,
            } => Approving::Confirm {
                fingerprint: fingerprint.clone(),
                device_name: device_name.clone(),
                servers: servers.iter().map(Approval::from).collect(),
            },
        },
        in_progress: view.in_progress,
        actions: view.actions,
        error: view.error,
    }
}

/// Where the approval of the new device on one server stands, as its line states it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApprovalShown {
    Working,
    /// The server holds the key of the request behind the code. Nothing is approved.
    Ready,
    Approved,
    Failed(String),
    /// The approval was sent and nothing tells whether it took effect.
    Unknown,
    /// This device has another account on the server than the one of the line: the
    /// account that it has now, as the line names it.
    AccountChanged(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalLine {
    pub server_id: ServerId,
    /// Empty for a server that this device has not saved.
    pub server: String,
    /// The account that the new device is added to: its name when the state of the
    /// server holds it, and its identifier otherwise. Absent until the first lookup.
    pub account: Option<String>,
    pub progress: ApprovalShown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FirstStep {
    EnterRequest,
    Confirm {
        fingerprint: String,
        device_name: String,
        lines: Vec<ApprovalLine>,
        /// A line is ready, unknown or failed: "Approve" does something.
        can_approve: bool,
        all_approved: bool,
    },
}

/// The link on all servers as the first device shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FirstDeviceView {
    /// The text of the offer. Absent when this device has no account on any server.
    pub offer: Option<String>,
    /// The names of the servers of the offer.
    pub offered: Vec<String>,
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
    EnterOffer,
    Requested {
        device_name: String,
        /// The request for the first device, while a link waits for its approval.
        request: Option<LinkRequestText>,
        fingerprint: Option<String>,
        lines: Vec<LinkLine>,
        all_linked: bool,
    },
}

/// The link on all servers as the new device shows it.
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

/// What the first device shows, derived without a side effect.
pub fn first_device(
    servers: &[ServerSnapshot],
    offer: Option<&LinkOffer>,
    flow: &ApprovingView,
) -> FirstDeviceView {
    let found = |server_id: ServerId| servers.iter().find(|server| server.id == server_id);
    let offered: Vec<String> = offer
        .map(|offer| {
            offer
                .servers
                .iter()
                .map(|offered| match found(offered.server_id) {
                    Some(server) => shown_name(&server.name),
                    None => shown_address(offered.address.as_str()),
                })
                .collect()
        })
        .unwrap_or_default();
    let text = offer
        .filter(|offer| !offer.servers.is_empty())
        .map(LinkOffer::to_text)
        .filter(|text| !text.is_empty());
    let busy = flow.in_progress.is_some();
    let step = match &flow.state {
        Approving::EnterRequest => FirstStep::EnterRequest,
        Approving::Confirm {
            fingerprint,
            device_name,
            servers: approvals,
        } => {
            let lines: Vec<ApprovalLine> = approvals
                .iter()
                .map(|approval| {
                    let server = found(approval.server_id);
                    ApprovalLine {
                        server_id: approval.server_id,
                        server: shown_name(&approval.name),
                        account: approval
                            .account_id
                            .map(|account_id| account_name(server, account_id)),
                        progress: match &approval.progress {
                            ApprovalProgress::Working => ApprovalShown::Working,
                            ApprovalProgress::Ready => ApprovalShown::Ready,
                            ApprovalProgress::Approved => ApprovalShown::Approved,
                            ApprovalProgress::Failed(error) => {
                                ApprovalShown::Failed(text::account::link_error(error))
                            }
                            ApprovalProgress::Unknown => ApprovalShown::Unknown,
                            ApprovalProgress::AccountChanged { now } => {
                                ApprovalShown::AccountChanged(account_name(server, *now))
                            }
                        },
                    }
                })
                .collect();
            FirstStep::Confirm {
                fingerprint: fingerprint.clone(),
                device_name: shown_name(device_name),
                can_approve: !busy
                    && lines.iter().any(|line| {
                        matches!(
                            line.progress,
                            ApprovalShown::Ready
                                | ApprovalShown::Unknown
                                | ApprovalShown::Failed(_)
                        )
                    }),
                all_approved: lines
                    .iter()
                    .all(|line| line.progress == ApprovalShown::Approved),
                lines,
            }
        }
    };
    FirstDeviceView {
        offer: text,
        offered,
        step,
        busy,
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
        LinkDeviceState::EnterOffer => NewStep::EnterOffer,
        LinkDeviceState::Requested {
            device_name,
            servers: links,
            request,
            fingerprint,
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
            let waits = lines.iter().any(|line| line.progress == LinkShown::Waiting);
            NewStep::Requested {
                device_name: shown_name(device_name),
                request: request.clone().filter(|_| waits),
                fingerprint: fingerprint.clone().filter(|_| waits),
                all_linked: lines.iter().all(|line| line.progress == LinkShown::Linked),
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

/// What the layer of the link on all servers holds: the role of this device and its flow.
/// The flow holds the request with its codes, and is dropped with the layer.
#[derive(Default)]
pub struct LinkingState {
    open: Option<LinkRole>,
    /// The servers on which this device has an account, as they were when the layer
    /// opened.
    offer: Option<LinkOffer>,
    approving: Option<ApproveDevicesFlow>,
    requesting: Option<LinkDeviceFlow>,
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

    /// Opens the link on all servers in one role. A flow of the layer before is dropped.
    pub fn open_link_all(&mut self, role: LinkRole, cx: &mut Context<Self>) {
        let first = role == LinkRole::FirstDevice;
        self.linking = LinkingState {
            open: Some(role),
            offer: first.then(|| self.client.link_offer()),
            approving: first.then(|| self.client.approve_devices()),
            requesting: (!first).then(|| self.client.link_this_device()),
            #[cfg(feature = "test-support")]
            fixture: None,
        };
        cx.notify();
    }

    /// Closes the link on all servers. The request that the flow held is dropped. An
    /// action that runs goes to its end in the client core, and each server shows its
    /// own state afterwards.
    pub fn close_link_all(&mut self, cx: &mut Context<Self>) {
        self.linking = LinkingState::default();
        cx.notify();
    }

    /// The link on all servers as the window shows it.
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
                self.linking.offer.as_ref(),
                &approving(self.linking.approving.as_ref()?),
            )),
            LinkRole::NewDevice => LinkAllView::New(new_device(
                &servers,
                &self.linking.requesting.as_ref()?.view(),
            )),
        })
    }

    fn run_approving(&mut self, action: ApproveDevicesAction, cx: &mut Context<Self>) {
        if let Some(flow) = &self.linking.approving {
            Self::run(flow.dispatch(action), cx);
            cx.notify();
        }
    }

    fn run_requesting(&mut self, action: LinkDeviceAction, cx: &mut Context<Self>) {
        if let Some(flow) = &self.linking.requesting {
            Self::run(flow.dispatch(action), cx);
            cx.notify();
        }
    }

    /// The first device takes the request of the new device and looks each link up on
    /// its server. Nothing is approved.
    pub fn look_up_request(&mut self, request: String, cx: &mut Context<Self>) {
        let request = LinkRequestText::new(request.into());
        self.run_approving(ApproveDevicesAction::LookUp { request }, cx);
    }

    /// The first device approves every link that is ready, each for the account of its
    /// line (ADR 0716).
    pub fn approve_devices(&mut self, cx: &mut Context<Self>) {
        self.run_approving(ApproveDevicesAction::Approve, cx);
    }

    /// The first device accepts, for one server, the account that it has there now.
    pub fn use_account(&mut self, server_id: ServerId, cx: &mut Context<Self>) {
        self.run_approving(ApproveDevicesAction::ConfirmAccount { server_id }, cx);
    }

    /// The new device takes the offer and asks each of its servers for a link.
    pub fn start_linking(&mut self, offer: String, device_name: String, cx: &mut Context<Self>) {
        self.run_requesting(LinkDeviceAction::Start { offer, device_name }, cx);
    }

    /// The new device asks every server that waits whether the first device approved.
    pub fn check_links(&mut self, cx: &mut Context<Self>) {
        self.run_requesting(LinkDeviceAction::Check, cx);
    }

    /// The new device asks again on every server that is not linked.
    pub fn retry_links(&mut self, cx: &mut Context<Self>) {
        self.run_requesting(LinkDeviceAction::Retry, cx);
    }

    /// Draws this state of the link on all servers in place of what the flows hold. The
    /// screen tool uses it.
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
    use junto_client_core::address::ServerAddress;
    use junto_client_core::flow::link_all::{OfferedServer, ServerLink};
    use junto_client_core::protocol::error::ErrorCode;
    use junto_client_core::protocol::keys::PublicKey;

    use super::super::account::tests::{at, server};
    use super::*;

    fn address(host: &str) -> ServerAddress {
        ServerAddress::parse(host).unwrap()
    }

    fn waiting(code: &str) -> AccessState {
        AccessState::WaitingForLink {
            code: LinkCode::new(code),
            public_key: PublicKey::from_bytes([7; 32]),
            expires_at: at(),
        }
    }

    fn approving(state: Approving) -> ApprovingView {
        FlowView {
            state,
            in_progress: None,
            actions: Vec::new(),
            error: None,
        }
    }

    fn requesting(state: LinkDeviceState) -> RequestingView {
        FlowView {
            state,
            in_progress: None,
            actions: Vec::new(),
            error: None,
        }
    }

    fn request_text() -> LinkRequestText {
        LinkRequestText::new("junto-link-request-1.SECRET".to_owned().into())
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
    fn a_text_of_more_than_32_kib_does_not_fit() {
        assert!(fits(""));
        assert!(fits(&"a".repeat(32 * 1024)));
        assert!(!fits(&"a".repeat(32 * 1024 + 1)));
    }

    #[test]
    fn the_first_device_shows_its_offer_and_then_one_line_per_server() {
        let (acme, harbor, lab, pier) = (
            ServerId::generate(),
            ServerId::generate(),
            ServerId::generate(),
            ServerId::generate(),
        );
        let (ada, other) = (AccountId::generate(), AccountId::generate());
        let mut second = server(harbor, ada, "Ada H.");
        second.name = "Harbor".to_owned();
        let servers = [server(acme, ada, "Ada"), second];
        let offer = LinkOffer {
            servers: vec![
                OfferedServer {
                    server_id: acme,
                    address: address("acme.example.test"),
                },
                OfferedServer {
                    server_id: harbor,
                    address: address("harbor.example.test"),
                },
            ],
        };

        // Step 1 and 2: the offer, and the field for the request.
        let view = first_device(&servers, Some(&offer), &approving(Approving::EnterRequest));
        assert_eq!(view.offer, Some(offer.to_text()));
        assert!(
            view.offer
                .as_ref()
                .unwrap()
                .starts_with("junto-link-offer-1.")
        );
        assert_eq!(view.offered, ["Acme Studio", "Harbor"]);
        assert_eq!(view.step, FirstStep::EnterRequest);
        assert!(!view.busy && view.error.is_none());

        // A device without an account has nothing to offer.
        let none = LinkOffer {
            servers: Vec::new(),
        };
        let empty = first_device(&servers, Some(&none), &approving(Approving::EnterRequest));
        assert_eq!((empty.offer, empty.offered.len()), (None, 0));

        // Step 3: every state of a line.
        let approval = |server_id, name: &str, account_id, progress| Approval {
            server_id,
            name: name.to_owned(),
            account_id,
            progress,
        };
        let mut flow = approving(Approving::Confirm {
            fingerprint: "1a2b-3c4d-5e6f-7a8b".to_owned(),
            device_name: "Junto on\u{0} Linux".to_owned(),
            servers: vec![
                approval(acme, "Acme Studio", Some(ada), ApprovalProgress::Ready),
                approval(
                    harbor,
                    "Harbor",
                    Some(ada),
                    ApprovalProgress::AccountChanged { now: other },
                ),
                approval(
                    lab,
                    "",
                    None,
                    ApprovalProgress::Failed(FlowError::UnknownServer),
                ),
                approval(pier, "Pier", Some(other), ApprovalProgress::Unknown),
            ],
        });
        flow.error = Some(FlowError::Incomplete {
            failed: 3,
            total: 4,
        });
        let view = first_device(&servers, Some(&offer), &flow);
        assert_eq!(
            view.error.as_deref(),
            Some("3 of 4 servers did not succeed. Their lines state why.")
        );
        let FirstStep::Confirm {
            fingerprint,
            device_name,
            lines,
            can_approve,
            all_approved,
        } = view.step
        else {
            panic!("the request was looked up");
        };
        assert_eq!(fingerprint, "1a2b-3c4d-5e6f-7a8b");
        assert_eq!(device_name, "Junto on Linux");
        assert!(can_approve && !all_approved);
        assert_eq!(
            lines,
            [
                ApprovalLine {
                    server_id: acme,
                    server: "Acme Studio".to_owned(),
                    // The account of this device on the server is named.
                    account: Some("Ada".to_owned()),
                    progress: ApprovalShown::Ready,
                },
                ApprovalLine {
                    server_id: harbor,
                    server: "Harbor".to_owned(),
                    account: Some("Ada H.".to_owned()),
                    // Another account than the one of this device has no name here.
                    progress: ApprovalShown::AccountChanged(other.to_string()),
                },
                ApprovalLine {
                    server_id: lab,
                    server: String::new(),
                    account: None,
                    progress: ApprovalShown::Failed(text::account::link_error(
                        &FlowError::UnknownServer
                    )),
                },
                ApprovalLine {
                    server_id: pier,
                    server: "Pier".to_owned(),
                    account: Some(other.to_string()),
                    progress: ApprovalShown::Unknown,
                },
            ]
        );

        // While an action runs, "Approve" does nothing; the lines change one by one.
        let working = |progress: [ApprovalProgress; 2], busy: bool| {
            let mut flow = approving(Approving::Confirm {
                fingerprint: String::new(),
                device_name: String::new(),
                servers: vec![
                    approval(acme, "Acme Studio", Some(ada), progress[0].clone()),
                    approval(harbor, "Harbor", Some(ada), progress[1].clone()),
                ],
            });
            flow.in_progress = busy.then_some(ApproveDevicesActionKind::Approve);
            match first_device(&servers, Some(&offer), &flow).step {
                FirstStep::Confirm {
                    lines,
                    can_approve,
                    all_approved,
                    ..
                } => (
                    lines
                        .into_iter()
                        .map(|line| line.progress)
                        .collect::<Vec<_>>(),
                    can_approve,
                    all_approved,
                ),
                FirstStep::EnterRequest => panic!("the request was looked up"),
            }
        };
        assert_eq!(
            working(
                [ApprovalProgress::Approved, ApprovalProgress::Working],
                true
            ),
            (
                vec![ApprovalShown::Approved, ApprovalShown::Working],
                false,
                false
            )
        );
        assert!(!working([ApprovalProgress::Approved, ApprovalProgress::Ready], true).1);
        assert_eq!(
            working(
                [ApprovalProgress::Approved, ApprovalProgress::Approved],
                false
            ),
            (
                vec![ApprovalShown::Approved, ApprovalShown::Approved],
                false,
                true
            )
        );
        // A line that waits for "Use this account" is not approved by "Approve".
        assert!(
            !working(
                [
                    ApprovalProgress::Approved,
                    ApprovalProgress::AccountChanged { now: other }
                ],
                false
            )
            .1
        );
    }

    #[test]
    fn the_new_device_shows_its_request_and_one_line_per_server() {
        let (acme, harbor, lab) = (
            ServerId::generate(),
            ServerId::generate(),
            ServerId::generate(),
        );
        let account = AccountId::generate();
        let view = new_device(&[], &requesting(LinkDeviceState::EnterOffer));
        assert_eq!(view.step, NewStep::EnterOffer);

        let link = |server_id, host: &str, progress| ServerLink {
            server_id,
            address: address(host),
            progress,
        };
        let requested = |links: Vec<ServerLink>| {
            requesting(LinkDeviceState::Requested {
                device_name: "Junto on Linux".to_owned(),
                servers: links,
                request: Some(request_text()),
                fingerprint: Some("1a2b-3c4d-5e6f-7a8b".to_owned()),
            })
        };
        let mut saved = server(acme, account, "Ada");
        saved.access = waiting("K7QM-2XHD");
        let mut flow = requested(vec![
            link(acme, "acme.example.test", LinkProgress::Waiting),
            link(harbor, "harbor.example.test", LinkProgress::Working),
            link(
                lab,
                "lab.example.test",
                LinkProgress::Failed(FlowError::refused(ErrorCode::DeviceLinksFull)),
            ),
        ]);
        flow.error = Some(FlowError::Incomplete {
            failed: 1,
            total: 3,
        });
        flow.in_progress = Some(LinkDeviceActionKind::Start);
        let view = new_device(std::slice::from_ref(&saved), &flow);
        assert!(view.busy);
        assert_eq!(
            view.error.as_deref(),
            Some("1 of 3 servers did not succeed. Its line states why.")
        );
        // The formatted form of the view shows nothing of the request.
        assert!(!format!("{view:?}").contains("SECRET"));
        let NewStep::Requested {
            device_name,
            request,
            fingerprint,
            lines,
            all_linked,
        } = view.step
        else {
            panic!("the links were asked for");
        };
        assert_eq!(device_name, "Junto on Linux");
        assert_eq!(
            request.as_ref().map(LinkRequestText::expose),
            Some("junto-link-request-1.SECRET")
        );
        assert_eq!(fingerprint.as_deref(), Some("1a2b-3c4d-5e6f-7a8b"));
        assert!(!all_linked);
        assert_eq!(
            lines,
            [
                LinkLine {
                    server_id: acme,
                    // A saved server has its name, and another one its address.
                    server: "Acme Studio".to_owned(),
                    progress: LinkShown::Waiting,
                },
                LinkLine {
                    server_id: harbor,
                    server: "harbor.example.test".to_owned(),
                    progress: LinkShown::Working,
                },
                LinkLine {
                    server_id: lab,
                    server: "lab.example.test".to_owned(),
                    progress: LinkShown::Failed(text::account::link_error(&FlowError::refused(
                        ErrorCode::DeviceLinksFull
                    ))),
                },
            ]
        );

        // The application checks a waiting link by itself: a server that shows the login
        // is linked before the flow is asked, and a link that expired has failed.
        let progress = |access: AccessState, held: LinkProgress| {
            let mut shown = server(acme, account, "Ada");
            shown.access = access;
            let flow = requested(vec![link(acme, "acme.example.test", held)]);
            match new_device(&[shown], &flow).step {
                NewStep::Requested {
                    lines,
                    request,
                    all_linked,
                    ..
                } => (lines[0].progress.clone(), request.is_some(), all_linked),
                NewStep::EnterOffer => panic!("the links were asked for"),
            }
        };
        let logged_in = server(acme, account, "Ada").access;
        assert_eq!(
            progress(logged_in.clone(), LinkProgress::Waiting),
            (LinkShown::Linked, false, true)
        );
        assert_eq!(
            progress(logged_in.clone(), LinkProgress::Failed(FlowError::Busy)),
            (LinkShown::Linked, false, true)
        );
        assert_eq!(
            progress(logged_in, LinkProgress::Working).0,
            LinkShown::Working
        );
        assert_eq!(
            progress(
                AccessState::NoAccount {
                    notice: Some(Notice::LinkExpired)
                },
                LinkProgress::Waiting
            ),
            (
                LinkShown::Failed(text::account::link_error(&FlowError::Expired)),
                false,
                false
            )
        );
        // The request is shown while a link waits for its approval.
        assert_eq!(
            progress(waiting("K7QM-2XHD"), LinkProgress::Waiting),
            (LinkShown::Waiting, true, false)
        );
        assert_eq!(
            progress(waiting("K7QM-2XHD"), LinkProgress::Linked),
            (LinkShown::Linked, false, true)
        );
    }
}
