//! Linking a new device on every server of the first device in one flow (ADR 0104,
//! ADR 0716).
//!
//! The two devices exchange two texts, which the user carries. The first device gives an
//! offer: the servers on which it has an account. The new device adds each server, creates
//! a key per server, asks each server for a link code, and gives a request: one code and
//! one public key per server. The first device approves each link on the server that it
//! has saved under that identifier, for the account that the user saw, and only when that
//! server holds the same public key behind the code. The servers are worked on side by
//! side: each has its own progress, which is shown when it changes, and a server that is
//! slow or fails holds up no other.

use std::collections::BTreeSet;
use std::fmt;
use std::future::Future;
use std::sync::Arc;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use junto_protocol::error::ErrorCode;
use junto_protocol::ids::{AccountId, ServerId};
use junto_protocol::keys::PublicKey;
use junto_protocol::secret::LinkCode;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::Semaphore;
use tokio::task::JoinSet;
use zeroize::Zeroizing;

use super::access::{AccessAction, AccessState, Notice};
use super::add_server::{AddServerAction, AddServerState};
use super::approve_link::{Approval, approve, device_with_key};
use super::machine::{Machine, Outcome};
use super::{FlowError, FlowView};
use crate::address::ServerAddress;
use crate::book::{ConnectionMode, Membership, ServerEntry};
use crate::manager::Client;
use crate::profile;
use crate::secret::SecretBytes;

const OFFER_PREFIX: &str = "junto-link-offer-1.";
const REQUEST_PREFIX: &str = "junto-link-request-1.";
/// The longest text of an offer or a request, in bytes.
pub const LONGEST_TEXT: usize = 32 * 1024;
const MOST_SERVERS: usize = 64;
const LONGEST_LINK_CODE: usize = 32;
/// How many servers one step of a flow works on at the same time.
pub(crate) const SERVERS_AT_ONCE: usize = 4;

/// True for a link code in the form of ADR 0110: digits, upper-case letters and hyphens,
/// at most 32 characters. A code in another form is not shown, typed or passed on.
pub fn link_code_is_plain(code: &LinkCode) -> bool {
    let text = code.expose();
    !text.is_empty()
        && text.len() <= LONGEST_LINK_CODE
        && text
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'-')
}

/// The text of `content` in a buffer that is overwritten when it is dropped. `None` when
/// the text is longer than a text may be.
fn encode<T: Serialize>(prefix: &str, content: &T) -> Option<Zeroizing<String>> {
    let mut json = SecretBytes::with_capacity(1024);
    serde_json::to_writer(&mut json, content).ok()?;
    let length = prefix.len() + json.len().div_ceil(3) * 4;
    if length > LONGEST_TEXT + 3 {
        return None;
    }
    let mut text = Zeroizing::new(String::with_capacity(length));
    text.push_str(prefix);
    URL_SAFE_NO_PAD.encode_string(json.as_slice(), &mut text);
    (text.len() <= LONGEST_TEXT).then_some(text)
}

fn decode<T: for<'de> Deserialize<'de>>(
    prefix: &str,
    kind: &'static str,
    text: &str,
) -> Result<T, FlowError> {
    let invalid = || FlowError::InvalidBundle(kind);
    let text = text.trim();
    if text.len() > LONGEST_TEXT {
        return Err(invalid());
    }
    let encoded = text.strip_prefix(prefix).ok_or_else(invalid)?;
    let mut json = Zeroizing::new(vec![0u8; encoded.len()]);
    let length = URL_SAFE_NO_PAD
        .decode_slice(encoded, json.as_mut_slice())
        .map_err(|_| invalid())?;
    serde_json::from_slice(&json[..length]).map_err(|_| invalid())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OfferedServer {
    pub server_id: ServerId,
    pub address: ServerAddress,
}

/// The servers on which the first device has an account. The text holds no secret.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkOffer {
    pub servers: Vec<OfferedServer>,
}

impl LinkOffer {
    pub(crate) fn of(entries: impl Iterator<Item = ServerEntry>) -> Self {
        Self {
            servers: entries
                .filter(|entry| matches!(entry.membership, Membership::Member { .. }))
                .take(MOST_SERVERS)
                .map(|entry| OfferedServer {
                    server_id: entry.server_id,
                    address: entry.address,
                })
                .collect(),
        }
    }

    pub fn to_text(&self) -> String {
        encode(OFFER_PREFIX, self)
            .map(|text| text.as_str().to_owned())
            .unwrap_or_default()
    }

    pub fn parse(text: &str) -> Result<Self, FlowError> {
        let offer: Self = decode(OFFER_PREFIX, "link offer", text)?;
        let ids: BTreeSet<ServerId> = offer
            .servers
            .iter()
            .map(|server| server.server_id)
            .collect();
        let addresses: BTreeSet<&str> = offer
            .servers
            .iter()
            .map(|server| server.address.as_str())
            .collect();
        let count = offer.servers.len();
        if count == 0 || count > MOST_SERVERS || ids.len() != count || addresses.len() != count {
            return Err(FlowError::InvalidBundle("link offer"));
        }
        Ok(offer)
    }
}

/// The link that the new device asked one server for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestedLink {
    pub server_id: ServerId,
    pub code: LinkCode,
    pub public_key: PublicKey,
}

/// What the new device asks the first device to approve.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkRequest {
    pub device_name: String,
    pub links: Vec<RequestedLink>,
}

impl LinkRequest {
    /// The text of the request. `None` when a code is not in the form of a link code or
    /// the text would be longer than a request may be.
    pub fn to_text(&self) -> Option<LinkRequestText> {
        if !self.links.iter().all(|link| link_code_is_plain(&link.code)) {
            return None;
        }
        encode(REQUEST_PREFIX, self).map(LinkRequestText)
    }

    pub fn parse(text: &LinkRequestText) -> Result<Self, FlowError> {
        let invalid = || FlowError::InvalidBundle("link request");
        let mut request: Self = decode(REQUEST_PREFIX, "link request", text.expose())?;
        request.device_name = profile::device_name(&request.device_name).map_err(|_| invalid())?;
        let ids: BTreeSet<ServerId> = request.links.iter().map(|link| link.server_id).collect();
        let count = request.links.len();
        let plain = request
            .links
            .iter()
            .all(|link| link_code_is_plain(&link.code));
        if count == 0 || count > MOST_SERVERS || ids.len() != count || !plain {
            return Err(invalid());
        }
        Ok(request)
    }

    /// A short text that both devices show, made from the servers and the public keys of
    /// the request. The user compares the two, so that a request that was replaced on its
    /// way is noticed before it is approved.
    pub fn fingerprint(&self) -> String {
        let mut keys: Vec<(ServerId, PublicKey)> = self
            .links
            .iter()
            .map(|link| (link.server_id, link.public_key))
            .collect();
        keys.sort_by_key(|(server_id, _)| *server_id);
        let mut hasher = Sha256::new();
        hasher.update(b"junto-link-request-v1\n");
        for (server_id, public_key) in keys {
            hasher.update(format!("{server_id}:{public_key}\n").as_bytes());
        }
        let digest = hasher.finalize();
        let groups: Vec<String> = digest
            .iter()
            .take(8)
            .collect::<Vec<_>>()
            .chunks(2)
            .map(|pair| pair.iter().map(|byte| format!("{byte:02x}")).collect())
            .collect();
        groups.join("-")
    }
}

/// The text of a link request. It holds link codes: formatting shows nothing of it, and
/// its buffer is overwritten when it is dropped.
#[derive(Clone, PartialEq, Eq)]
pub struct LinkRequestText(Zeroizing<String>);

impl LinkRequestText {
    /// Takes a text that the user carried from the new device.
    pub fn new(text: Zeroizing<String>) -> Self {
        Self(text)
    }

    /// The text that the user carries to the first device.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for LinkRequestText {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("LinkRequestText(redacted)")
    }
}

fn incomplete<S>(state: S, failed: usize, total: usize) -> Outcome<S> {
    if failed == 0 {
        Outcome::moved(state)
    } else {
        Outcome::failed_and_moved(state, FlowError::Incomplete { failed, total })
    }
}

/// Runs one piece of work per server, at most [`SERVERS_AT_ONCE`] at a time, and returns
/// when all have ended. No server waits for the end of another.
async fn side_by_side<T, F, Fut>(items: Vec<T>, work: F)
where
    T: Send + 'static,
    F: Fn(T) -> Fut,
    Fut: Future<Output = ()> + Send + 'static,
{
    let slots = Arc::new(Semaphore::new(SERVERS_AT_ONCE));
    let mut running = JoinSet::new();
    for item in items {
        let (slots, work) = (slots.clone(), work(item));
        running.spawn(async move {
            let _slot = slots.acquire_owned().await;
            work.await;
        });
    }
    while running.join_next().await.is_some() {}
}

/// Where the link of this device on one server stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkProgress {
    /// The client is asking the server now.
    Working,
    /// A link code was issued and waits for the approval of the first device.
    Waiting,
    Linked,
    Failed(FlowError),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerLink {
    pub server_id: ServerId,
    pub address: ServerAddress,
    pub progress: LinkProgress,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkDeviceState {
    EnterOffer,
    Requested {
        device_name: String,
        servers: Vec<ServerLink>,
        /// The request for the first device, with every link that waits. Absent when no
        /// link waits and while an action runs.
        request: Option<LinkRequestText>,
        fingerprint: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkDeviceAction {
    /// Takes the offer of the first device and asks every server in it for a link.
    Start {
        offer: String,
        device_name: String,
    },
    /// Asks every server that waits whether the first device approved.
    Check,
    /// Asks again on every server that is not linked: the ones that failed get another
    /// try, and the ones that wait get a new code.
    Retry,
    Reset,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LinkDeviceActionKind {
    Start,
    Check,
    Retry,
    Reset,
}

impl LinkDeviceAction {
    pub fn kind(&self) -> LinkDeviceActionKind {
        match self {
            Self::Start { .. } => LinkDeviceActionKind::Start,
            Self::Check => LinkDeviceActionKind::Check,
            Self::Retry => LinkDeviceActionKind::Retry,
            Self::Reset => LinkDeviceActionKind::Reset,
        }
    }
}

pub fn link_device_actions(state: &LinkDeviceState) -> &'static [LinkDeviceActionKind] {
    use LinkDeviceActionKind as K;
    match state {
        LinkDeviceState::EnterOffer => &[K::Start],
        LinkDeviceState::Requested { .. } => &[K::Check, K::Retry, K::Reset],
    }
}

/// The flow of the new device.
#[derive(Clone)]
pub struct LinkDeviceFlow {
    client: Client,
    machine: Arc<Machine<LinkDeviceState, LinkDeviceActionKind>>,
}

impl LinkDeviceFlow {
    pub(crate) fn new(client: Client) -> Self {
        Self {
            client,
            machine: Arc::new(Machine::new(
                LinkDeviceState::EnterOffer,
                link_device_actions,
            )),
        }
    }

    pub fn view(&self) -> FlowView<LinkDeviceState, LinkDeviceActionKind> {
        self.machine.view()
    }

    pub fn dispatch(
        &self,
        action: LinkDeviceAction,
    ) -> impl Future<Output = Result<(), FlowError>> + Send + 'static + use<> {
        let flow = self.clone();
        let work = self.client.runtime().run(async move {
            let (before, run) = flow.machine.begin(action.kind())?;
            flow.client.notify();
            let outcome = flow.perform(before, action).await;
            let result = run.finish(outcome);
            flow.client.notify();
            result
        });
        async move { work.await? }
    }

    /// Shows the progress of one server at once.
    fn publish(&self, server_id: ServerId, progress: LinkProgress) {
        self.machine.update(|state| {
            if let LinkDeviceState::Requested { servers, .. } = state
                && let Some(server) = servers
                    .iter_mut()
                    .find(|server| server.server_id == server_id)
            {
                server.progress = progress;
            }
        });
        self.client.notify();
    }

    async fn perform(
        &self,
        before: LinkDeviceState,
        action: LinkDeviceAction,
    ) -> Outcome<LinkDeviceState> {
        let (device_name, mut servers, again) = match (action, before) {
            (LinkDeviceAction::Reset, _) => return Outcome::moved(LinkDeviceState::EnterOffer),
            (LinkDeviceAction::Start { offer, device_name }, _) => {
                let offer = match LinkOffer::parse(&offer) {
                    Ok(offer) => offer,
                    Err(error) => return Outcome::failed(error),
                };
                let device_name = match profile::device_name(&device_name) {
                    Ok(name) => name,
                    Err(error) => return Outcome::failed(error),
                };
                let servers = offer
                    .servers
                    .into_iter()
                    .map(|offered| ServerLink {
                        server_id: offered.server_id,
                        address: offered.address,
                        progress: LinkProgress::Working,
                    })
                    .collect();
                (device_name, servers, true)
            }
            (
                LinkDeviceAction::Check,
                LinkDeviceState::Requested {
                    device_name,
                    servers,
                    ..
                },
            ) => (device_name, servers, false),
            (
                LinkDeviceAction::Retry,
                LinkDeviceState::Requested {
                    device_name,
                    servers,
                    ..
                },
            ) => (device_name, servers, true),
            (LinkDeviceAction::Check | LinkDeviceAction::Retry, _) => {
                return Outcome::failed(FlowError::NotAvailable);
            }
        };

        let mut work = Vec::new();
        for server in &mut servers {
            let wanted = match (again, &server.progress) {
                (_, LinkProgress::Linked) => false,
                (true, _) => true,
                (false, progress) => *progress == LinkProgress::Waiting,
            };
            if wanted {
                if again {
                    server.progress = LinkProgress::Working;
                }
                work.push(OfferedServer {
                    server_id: server.server_id,
                    address: server.address.clone(),
                });
            }
        }
        let shown = LinkDeviceState::Requested {
            device_name: device_name.clone(),
            servers,
            request: None,
            fingerprint: None,
        };
        self.machine.update(|state| *state = shown);
        self.client.notify();

        side_by_side(work, |offered| {
            let (flow, device_name) = (self.clone(), device_name.clone());
            async move {
                let progress = if again {
                    flow.ask(&offered, &device_name).await
                } else {
                    flow.check(offered.server_id).await
                };
                flow.publish(offered.server_id, progress);
            }
        })
        .await;

        let LinkDeviceState::Requested { servers, .. } = self.machine.state() else {
            return Outcome::failed(FlowError::Stopped);
        };
        self.requested(device_name, servers)
    }

    /// The state with the request that holds every link that waits now. A server whose
    /// code is not in the form of a link code, or whose link would make the request
    /// longer than a request may be, fails alone and is left out.
    fn requested(
        &self,
        device_name: String,
        mut servers: Vec<ServerLink>,
    ) -> Outcome<LinkDeviceState> {
        let mut request = LinkRequest {
            device_name: device_name.clone(),
            links: Vec::new(),
        };
        for server in &mut servers {
            if server.progress != LinkProgress::Waiting {
                continue;
            }
            let waiting = self
                .client
                .server(server.server_id)
                .map(|handle| handle.access().state);
            let Some(AccessState::WaitingForLink {
                code, public_key, ..
            }) = waiting
            else {
                server.progress = LinkProgress::Failed(FlowError::NotAvailable);
                continue;
            };
            request.links.push(RequestedLink {
                server_id: server.server_id,
                code,
                public_key,
            });
            if request.to_text().is_none() {
                request.links.pop();
                server.progress = LinkProgress::Failed(FlowError::InvalidResponse);
            }
        }
        let text = (!request.links.is_empty())
            .then(|| request.to_text())
            .flatten();
        let failed = servers
            .iter()
            .filter(|server| matches!(server.progress, LinkProgress::Failed(_)))
            .count();
        let total = servers.len();
        let state = LinkDeviceState::Requested {
            device_name,
            fingerprint: text.as_ref().map(|_| request.fingerprint()),
            request: text,
            servers,
        };
        incomplete(state, failed, total)
    }

    /// Saves the offered server when it is not saved, and asks it for a link code.
    async fn ask(&self, offered: &OfferedServer, device_name: &str) -> LinkProgress {
        match self.ask_server(offered, device_name).await {
            Ok(progress) => progress,
            Err(error) => LinkProgress::Failed(error),
        }
    }

    async fn ask_server(
        &self,
        offered: &OfferedServer,
        device_name: &str,
    ) -> Result<LinkProgress, FlowError> {
        let handle = match self.client.server(offered.server_id) {
            Some(handle) if handle.entry().address == offered.address => handle,
            Some(_) => {
                return Err(FlowError::AlreadySaved {
                    server_id: offered.server_id,
                });
            }
            None => {
                let adding = self.client.add_server();
                adding
                    .dispatch(AddServerAction::Fetch {
                        address: offered.address.as_str().to_owned(),
                    })
                    .await?;
                match adding.view().state {
                    AddServerState::Preview(preview) if preview.server_id == offered.server_id => {}
                    _ => return Err(FlowError::OtherServer),
                }
                adding
                    .dispatch(AddServerAction::Save {
                        connection_mode: ConnectionMode::default(),
                    })
                    .await?;
                self.client
                    .server(offered.server_id)
                    .ok_or(FlowError::UnknownServer)?
            }
        };
        // The person asked for this. A check that the client runs by itself on the same
        // server at this moment is waited for (ADR 0712). The flow of the server refuses
        // the request for a link in a state that does not accept it.
        handle.connect_when_free().await?;
        if matches!(handle.access().state, AccessState::LoggedIn { .. }) {
            return Ok(LinkProgress::Linked);
        }
        handle
            .dispatch_when_free(AccessAction::RequestLink {
                device_name: device_name.to_owned(),
            })
            .await?;
        Ok(LinkProgress::Waiting)
    }

    async fn check(&self, server_id: ServerId) -> LinkProgress {
        let Some(handle) = self.client.server(server_id) else {
            return LinkProgress::Failed(FlowError::UnknownServer);
        };
        if let Err(error) = handle.dispatch_when_free(AccessAction::Check).await {
            return LinkProgress::Failed(error);
        }
        match handle.access().state {
            AccessState::LoggedIn { .. } => LinkProgress::Linked,
            AccessState::WaitingForLink { .. } => LinkProgress::Waiting,
            AccessState::NoAccount {
                notice: Some(Notice::LinkExpired),
            } => LinkProgress::Failed(FlowError::Expired),
            _ => LinkProgress::Failed(FlowError::NotAvailable),
        }
    }
}

/// Where the approval of the new device on one server stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApprovalProgress {
    /// The client is asking the server now.
    Working,
    /// The server holds the public key of the request behind the code. Nothing was
    /// approved yet.
    Ready,
    Approved,
    Failed(FlowError),
    /// The approval was sent and nothing tells whether it took effect.
    Unknown,
    /// This device has another account on the server than the one that the user saw.
    /// Nothing is approved here until the user confirms the account `now`.
    AccountChanged {
        now: AccountId,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerApproval {
    pub server_id: ServerId,
    /// The name of the server in the book of this device. Empty for a server that this
    /// device has not saved.
    pub name: String,
    pub public_key: PublicKey,
    /// The account of this device on the server that the new device is added to. The
    /// interface shows it with the server. It is fixed at the first lookup and changes
    /// only through the action that confirms another account.
    pub account_id: Option<AccountId>,
    pub progress: ApprovalProgress,
    code: LinkCode,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApproveDevicesState {
    EnterRequest,
    /// The interface shows the fingerprint, the device name and every server with its
    /// account and public key. The user compares the fingerprint with the one on the new
    /// device.
    Confirm {
        fingerprint: String,
        device_name: String,
        servers: Vec<ServerApproval>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApproveDevicesAction {
    /// Takes the request of the new device and looks every link up on its server.
    LookUp {
        request: LinkRequestText,
    },
    /// Approves every link that is ready, each for the account that the state shows. A
    /// server that failed after its account was shown is looked up again and approved in
    /// the same step, so this is also how the failed ones are resumed. A server whose
    /// account was not shown yet is only looked up.
    Approve,
    /// Accepts, for one server, the account that this device has there now in place of
    /// the one that the state showed. The link is looked up again and not approved.
    ConfirmAccount {
        server_id: ServerId,
    },
    Reset,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ApproveDevicesActionKind {
    LookUp,
    Approve,
    ConfirmAccount,
    Reset,
}

impl ApproveDevicesAction {
    pub fn kind(&self) -> ApproveDevicesActionKind {
        match self {
            Self::LookUp { .. } => ApproveDevicesActionKind::LookUp,
            Self::Approve => ApproveDevicesActionKind::Approve,
            Self::ConfirmAccount { .. } => ApproveDevicesActionKind::ConfirmAccount,
            Self::Reset => ApproveDevicesActionKind::Reset,
        }
    }
}

pub fn approve_devices_actions(state: &ApproveDevicesState) -> &'static [ApproveDevicesActionKind] {
    use ApproveDevicesActionKind as K;
    match state {
        ApproveDevicesState::EnterRequest => &[K::LookUp],
        ApproveDevicesState::Confirm { .. } => {
            &[K::Approve, K::ConfirmAccount, K::LookUp, K::Reset]
        }
    }
}

fn key_mismatch() -> FlowError {
    FlowError::refused(ErrorCode::PublicKeyMismatch)
}

/// The progress after a failure. A change of the account is a state of its own, because
/// it needs the user and not another try.
fn failed(error: FlowError) -> ApprovalProgress {
    match error {
        FlowError::AccountChanged { now, .. } => ApprovalProgress::AccountChanged { now },
        other => ApprovalProgress::Failed(other),
    }
}

/// What one step does with one server.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Step {
    LookUp,
    Approve,
    LookUpThenApprove,
}

/// The flow of the first device.
#[derive(Clone)]
pub struct ApproveDevicesFlow {
    client: Client,
    machine: Arc<Machine<ApproveDevicesState, ApproveDevicesActionKind>>,
}

impl ApproveDevicesFlow {
    pub(crate) fn new(client: Client) -> Self {
        Self {
            client,
            machine: Arc::new(Machine::new(
                ApproveDevicesState::EnterRequest,
                approve_devices_actions,
            )),
        }
    }

    pub fn view(&self) -> FlowView<ApproveDevicesState, ApproveDevicesActionKind> {
        self.machine.view()
    }

    pub fn dispatch(
        &self,
        action: ApproveDevicesAction,
    ) -> impl Future<Output = Result<(), FlowError>> + Send + 'static + use<> {
        let flow = self.clone();
        let work = self.client.runtime().run(async move {
            let (before, run) = flow.machine.begin(action.kind())?;
            flow.client.notify();
            let outcome = flow.perform(before, action).await;
            let result = run.finish(outcome);
            flow.client.notify();
            result
        });
        async move { work.await? }
    }

    /// Shows the result of one server at once.
    fn publish(&self, approval: ServerApproval) {
        self.machine.update(|state| {
            if let ApproveDevicesState::Confirm { servers, .. } = state
                && let Some(server) = servers
                    .iter_mut()
                    .find(|server| server.server_id == approval.server_id)
            {
                *server = approval;
            }
        });
        self.client.notify();
    }

    async fn perform(
        &self,
        before: ApproveDevicesState,
        action: ApproveDevicesAction,
    ) -> Outcome<ApproveDevicesState> {
        let approves = action.kind() == ApproveDevicesActionKind::Approve;
        let (fingerprint, device_name, servers, work) = match (action, before) {
            (ApproveDevicesAction::Reset, _) => {
                return Outcome::moved(ApproveDevicesState::EnterRequest);
            }
            (ApproveDevicesAction::LookUp { request }, _) => {
                let request = match LinkRequest::parse(&request) {
                    Ok(request) => request,
                    Err(error) => return Outcome::failed(error),
                };
                let fingerprint = request.fingerprint();
                let servers: Vec<ServerApproval> = request
                    .links
                    .into_iter()
                    .map(|link| ServerApproval {
                        server_id: link.server_id,
                        name: String::new(),
                        public_key: link.public_key,
                        account_id: None,
                        progress: ApprovalProgress::Working,
                        code: link.code,
                    })
                    .collect();
                let work = servers
                    .iter()
                    .map(|server| (server.clone(), Step::LookUp))
                    .collect();
                (fingerprint, request.device_name, servers, work)
            }
            (
                ApproveDevicesAction::Approve,
                ApproveDevicesState::Confirm {
                    fingerprint,
                    device_name,
                    servers,
                },
            ) => {
                let work = servers
                    .iter()
                    .filter_map(|server| {
                        let step = match (&server.progress, server.account_id) {
                            (ApprovalProgress::Ready | ApprovalProgress::Unknown, Some(_)) => {
                                Step::Approve
                            }
                            (ApprovalProgress::Failed(_), Some(_)) => Step::LookUpThenApprove,
                            (ApprovalProgress::Failed(_), None) => Step::LookUp,
                            _ => return None,
                        };
                        Some((server.clone(), step))
                    })
                    .collect();
                (fingerprint, device_name, servers, work)
            }
            (
                ApproveDevicesAction::ConfirmAccount { server_id },
                ApproveDevicesState::Confirm {
                    fingerprint,
                    device_name,
                    mut servers,
                },
            ) => {
                let changed = servers.iter_mut().find_map(|server| match server.progress {
                    ApprovalProgress::AccountChanged { now } if server.server_id == server_id => {
                        server.account_id = Some(now);
                        server.progress = ApprovalProgress::Working;
                        Some(server.clone())
                    }
                    _ => None,
                });
                let Some(changed) = changed else {
                    return Outcome::failed(FlowError::NotAvailable);
                };
                (
                    fingerprint,
                    device_name,
                    servers,
                    vec![(changed, Step::LookUp)],
                )
            }
            (ApproveDevicesAction::Approve | ApproveDevicesAction::ConfirmAccount { .. }, _) => {
                return Outcome::failed(FlowError::NotAvailable);
            }
        };

        let shown = ApproveDevicesState::Confirm {
            fingerprint,
            device_name,
            servers,
        };
        self.machine.update(|state| *state = shown);
        self.client.notify();

        side_by_side(work, |(mut approval, step)| {
            let flow = self.clone();
            async move {
                if step != Step::Approve {
                    flow.look_up(&mut approval).await;
                }
                if step != Step::LookUp && approval.progress == ApprovalProgress::Ready
                    || step == Step::Approve && approval.progress == ApprovalProgress::Unknown
                {
                    flow.approve(&mut approval).await;
                }
                flow.publish(approval);
            }
        })
        .await;

        let state = self.machine.state();
        let ApproveDevicesState::Confirm { servers, .. } = &state else {
            return Outcome::failed(FlowError::Stopped);
        };
        let done = |progress: &ApprovalProgress| match progress {
            ApprovalProgress::Approved => true,
            ApprovalProgress::Ready => !approves,
            _ => false,
        };
        let failed = servers
            .iter()
            .filter(|server| !done(&server.progress))
            .count();
        let total = servers.len();
        incomplete(state, failed, total)
    }

    /// Looks one link up on the server that this device saved under its identifier. The
    /// address comes from the book of this device, never from the request.
    async fn look_up(&self, approval: &mut ServerApproval) {
        approval.progress = match self.looked_up(approval).await {
            Ok(progress) => progress,
            Err(error) => failed(error),
        };
    }

    async fn looked_up(
        &self,
        approval: &mut ServerApproval,
    ) -> Result<ApprovalProgress, FlowError> {
        let handle = self
            .client
            .server(approval.server_id)
            .ok_or(FlowError::UnknownServer)?;
        approval.name = handle.entry().name;
        if handle.account().is_none() {
            handle.connect().await?;
        }
        let current = handle.account().ok_or(FlowError::NotLoggedIn)?;
        let account_id = *approval.account_id.get_or_insert(current);
        if account_id != current {
            return Ok(ApprovalProgress::AccountChanged { now: current });
        }
        let api = handle.api_for(account_id);
        if device_with_key(&api, approval.public_key).await?.is_some() {
            return Ok(ApprovalProgress::Approved);
        }
        let pending = api.look_up_device_link(approval.code.clone()).await?;
        if pending.public_key != approval.public_key {
            return Err(key_mismatch());
        }
        Ok(ApprovalProgress::Ready)
    }

    /// Approves one link for the account that the state showed, and for no other.
    async fn approve(&self, approval: &mut ServerApproval) {
        let (Some(handle), Some(account_id)) =
            (self.client.server(approval.server_id), approval.account_id)
        else {
            approval.progress = ApprovalProgress::Failed(FlowError::UnknownServer);
            return;
        };
        let api = handle.api_for(account_id);
        approval.progress = match approve(&api, approval.code.clone(), approval.public_key).await {
            Approval::Approved(_) => ApprovalProgress::Approved,
            Approval::Refused(error) => failed(error.into()),
            Approval::Unknown(_) => ApprovalProgress::Unknown,
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> LinkRequest {
        LinkRequest {
            device_name: "Phone".to_owned(),
            links: vec![
                RequestedLink {
                    server_id: "0198f1c2-7d3a-7b1e-9c55-000000000002".parse().unwrap(),
                    code: LinkCode::new("ABCD-EFGH"),
                    public_key: PublicKey::from_bytes([2; 32]),
                },
                RequestedLink {
                    server_id: "0198f1c2-7d3a-7b1e-9c55-000000000001".parse().unwrap(),
                    code: LinkCode::new("JKMN-PQRS"),
                    public_key: PublicKey::from_bytes([1; 32]),
                },
            ],
        }
    }

    fn text(content: &str) -> LinkRequestText {
        LinkRequestText::new(Zeroizing::new(content.to_owned()))
    }

    #[test]
    fn an_offer_and_a_request_are_read_back_from_their_texts() {
        let offer = LinkOffer {
            servers: vec![OfferedServer {
                server_id: ServerId::generate(),
                address: ServerAddress::parse("acme.example.test").unwrap(),
            }],
        };
        let offer_text = offer.to_text();
        assert!(offer_text.starts_with("junto-link-offer-1."));
        assert_eq!(
            LinkOffer::parse(&format!("  {offer_text}\n")).unwrap(),
            offer
        );

        let request = request();
        let request_text = request.to_text().unwrap();
        assert!(request_text.expose().starts_with("junto-link-request-1."));
        assert_eq!(LinkRequest::parse(&request_text).unwrap(), request);
        assert_eq!(format!("{request_text:?}"), "LinkRequestText(redacted)");
        assert!(!format!("{request:?}").contains("ABCD"));
    }

    #[test]
    fn other_texts_are_refused() {
        let offer = FlowError::InvalidBundle("link offer");
        let request_error = FlowError::InvalidBundle("link request");
        let request = request();
        for content in [
            "",
            "hello",
            "junto-link-offer-1.!!!",
            "junto-link-offer-1.e30",
        ] {
            assert_eq!(LinkOffer::parse(content), Err(offer.clone()), "{content}");
        }
        assert_eq!(
            LinkOffer::parse(request.to_text().unwrap().expose()),
            Err(offer.clone())
        );
        let empty_offer = encode(OFFER_PREFIX, &LinkOffer { servers: vec![] }).unwrap();
        assert_eq!(LinkOffer::parse(&empty_offer), Err(offer.clone()));
        let not_local = r#"{"servers":[{"server_id":"0198f1c2-7d3a-7b1e-9c55-000000000001","address":"http://acme.example.test"}]}"#;
        let insecure = format!("{OFFER_PREFIX}{}", URL_SAFE_NO_PAD.encode(not_local));
        assert_eq!(LinkOffer::parse(&insecure), Err(offer.clone()));
        assert_eq!(
            LinkOffer::parse(&format!("{OFFER_PREFIX}{}", "A".repeat(LONGEST_TEXT))),
            Err(offer)
        );

        let mut twice = request.clone();
        twice.links[1].server_id = twice.links[0].server_id;
        let mut control = request.clone();
        control.device_name = "a\nb".to_owned();
        let mut empty = request;
        empty.links.clear();
        for invalid in [twice, control, empty] {
            assert_eq!(
                LinkRequest::parse(&invalid.to_text().unwrap()),
                Err(request_error.clone())
            );
        }
    }

    #[test]
    fn a_link_code_is_short_upper_case_text() {
        for code in ["ABCD-EFGH", "LINK-0001", "A", &"Z".repeat(32)] {
            assert!(link_code_is_plain(&LinkCode::new(code)), "{code}");
        }
        for code in [
            "",
            "abcd-efgh",
            "ABCD EFGH",
            "ABCD\nEFGH",
            "ÀBCD",
            &"A".repeat(33),
        ] {
            assert!(!link_code_is_plain(&LinkCode::new(code)), "{code:?}");
        }
    }

    #[test]
    fn a_request_with_a_code_in_another_form_or_over_the_limit_has_no_text() {
        let mut odd = request();
        odd.links[0].code = LinkCode::new("abcd");
        assert!(odd.to_text().is_none());
        let json = serde_json::to_vec(&odd).unwrap();
        let carried = text(&format!("{REQUEST_PREFIX}{}", URL_SAFE_NO_PAD.encode(json)));
        assert_eq!(
            LinkRequest::parse(&carried),
            Err(FlowError::InvalidBundle("link request"))
        );

        let mut large = request();
        large.device_name = "n".repeat(LONGEST_TEXT);
        assert!(large.to_text().is_none());
        let mut full = request();
        full.links = (0..MOST_SERVERS)
            .map(|_| RequestedLink {
                server_id: ServerId::generate(),
                code: LinkCode::new("Z".repeat(LONGEST_LINK_CODE)),
                public_key: PublicKey::from_bytes([7; 32]),
            })
            .collect();
        full.device_name = "n".repeat(64);
        let longest = full.to_text().unwrap();
        assert!(longest.expose().len() <= LONGEST_TEXT);
        assert_eq!(LinkRequest::parse(&longest).unwrap(), full);
    }

    #[test]
    fn the_fingerprint_names_the_servers_and_the_keys_and_not_their_order() {
        let request = request();
        let fingerprint = request.fingerprint();
        assert_eq!(fingerprint.len(), 19);
        assert_eq!(fingerprint.split('-').count(), 4);

        let mut reordered = request.clone();
        reordered.links.reverse();
        reordered.links[0].code = LinkCode::new("OTHER");
        reordered.device_name = "Other".to_owned();
        assert_eq!(reordered.fingerprint(), fingerprint);

        let mut other_key = request.clone();
        other_key.links[0].public_key = PublicKey::from_bytes([9; 32]);
        assert_ne!(other_key.fingerprint(), fingerprint);
        let mut other_server = request;
        other_server.links[0].server_id = ServerId::generate();
        assert_ne!(other_server.fingerprint(), fingerprint);
    }

    #[tokio::test(start_paused = true)]
    async fn work_on_several_servers_is_bounded_and_no_piece_waits_for_an_earlier_one() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let running = Arc::new(AtomicUsize::new(0));
        let most = Arc::new(AtomicUsize::new(0));
        let finished = Arc::new(std::sync::Mutex::new(Vec::new()));
        side_by_side((0..10u64).collect(), |index| {
            let (running, most, finished) = (running.clone(), most.clone(), finished.clone());
            async move {
                let now = running.fetch_add(1, Ordering::SeqCst) + 1;
                most.fetch_max(now, Ordering::SeqCst);
                let pause = if index == 0 { 1000 } else { 10 };
                tokio::time::sleep(std::time::Duration::from_millis(pause)).await;
                running.fetch_sub(1, Ordering::SeqCst);
                finished.lock().unwrap().push(index);
            }
        })
        .await;
        assert_eq!(most.load(Ordering::SeqCst), SERVERS_AT_ONCE);
        let finished = finished.lock().unwrap();
        assert_eq!(finished.len(), 10);
        assert_eq!(finished.last(), Some(&0));
    }
}
