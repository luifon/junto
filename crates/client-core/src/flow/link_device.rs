//! Linking a new device in one flow, with one code (ADR 0104, ADR 0716).
//!
//! The new device adds one server of the account by its address and asks it for a link
//! code. The person types that code once on a device of the account. That device looks the
//! code up on the servers that the person checked: the server that knows it is the first
//! server, and the person compares the key of the new device there. The approval on the
//! first server creates a relay with the list of the other checked servers, signed by the
//! approving device and encrypted to the new device. The new device adds each of these
//! servers, asks each for a link code with a key of its own, and answers through the relay
//! with each code and key. The approving device approves each link on its server only when
//! that server holds the same key behind the code. The servers are worked on side by side:
//! each has its own progress, and a server that is slow or fails holds up no other.

use std::collections::BTreeSet;
use std::future::Future;
use std::sync::{Arc, Mutex};

use junto_protocol::account::Device;
use junto_protocol::error::ErrorCode;
use junto_protocol::ids::{AccountId, ServerId};
use junto_protocol::keys::PublicKey;
use junto_protocol::secret::LinkCode;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

use super::access::{AccessAction, AccessState, Notice};
use super::add_server::{AddServerAction, AddServerState};
use super::approve_link::{Approval, PendingApproval, approve_with, device_with_key};
use super::machine::{Machine, Outcome, lock};
use super::relay::{
    self, Answer, Answered, AnsweredLink, Binding, Kind, LONGEST_REASON, MOST_SERVERS, Offer,
    OfferedServer, RelayFault,
};
use super::{FlowError, FlowView};
use crate::address::ServerAddress;
use crate::api::ApiError;
use crate::book::ConnectionMode;
use crate::manager::{Client, ServerHandle};
use crate::profile::{self, NAME_MAX_CHARS};

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

/// Runs one piece of work per item, at most [`SERVERS_AT_ONCE`] at a time, and returns
/// when all have ended. No piece waits for the end of another.
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

impl From<RelayFault> for FlowError {
    fn from(fault: RelayFault) -> Self {
        match fault {
            RelayFault::NotSigned => Self::NotSignedByDevice,
            RelayFault::Randomness => Self::Storage(fault.to_string()),
            RelayFault::Unreadable | RelayFault::TooLong => Self::InvalidResponse,
        }
    }
}

fn not_found(error: &ApiError) -> bool {
    error.code() == Some(ErrorCode::NotFound)
}

/// True for a failure that a later try with the same code can overcome: the server was not
/// reached, its answer was lost, or it was busy.
fn passing(error: &ApiError) -> bool {
    match error {
        ApiError::Unreachable(_) | ApiError::OutcomeUnknown | ApiError::InvalidResponse { .. } => {
            true
        }
        ApiError::Rejected(rejection) => {
            rejection.status >= 500
                || matches!(
                    rejection.code,
                    ErrorCode::RateLimited
                        | ErrorCode::TooManyAttempts
                        | ErrorCode::Unavailable
                        | ErrorCode::Internal
                )
        }
        _ => false,
    }
}

/// The reason of a failure as the other device shows it: one line, bounded.
fn reason_of(error: &FlowError) -> String {
    error
        .to_string()
        .chars()
        .filter(|character| !character.is_control())
        .take(LONGEST_REASON)
        .collect()
}

// The new device.

/// Where the link of this device on one server of the offer stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkProgress {
    /// The client is asking the server now.
    Working,
    /// A link code was issued and waits for the approval of the other device.
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
pub enum LinkThisDeviceState {
    EnterAddress,
    /// The first server gave a link code, which the person types on the other device. The
    /// code, its end and the key of this device are in the access state of that server.
    Waiting {
        first: ServerId,
    },
    /// The first server linked this device. `servers` are the other servers that the
    /// other device named; empty while its offer was not read, and when it named none.
    Linked {
        first: ServerId,
        servers: Vec<ServerLink>,
        /// The offer was read, or the first server holds none.
        offer_read: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkThisDeviceAction {
    /// Adds the server at `address` when it is not saved, and asks it for a link code.
    Start {
        address: String,
        device_name: String,
    },
    /// Asks the first server whether the other device approved; after the approval, reads
    /// the offer, asks the servers of the offer for links, answers, and asks every server
    /// that waits whether it linked this device.
    Check,
    /// Asks the first server for a new link code, which replaces the earlier one.
    NewCode,
    Reset,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LinkThisDeviceActionKind {
    Start,
    Check,
    NewCode,
    Reset,
}

impl LinkThisDeviceAction {
    pub fn kind(&self) -> LinkThisDeviceActionKind {
        match self {
            Self::Start { .. } => LinkThisDeviceActionKind::Start,
            Self::Check => LinkThisDeviceActionKind::Check,
            Self::NewCode => LinkThisDeviceActionKind::NewCode,
            Self::Reset => LinkThisDeviceActionKind::Reset,
        }
    }
}

pub fn link_this_device_actions(
    state: &LinkThisDeviceState,
) -> &'static [LinkThisDeviceActionKind] {
    use LinkThisDeviceActionKind as K;
    match state {
        LinkThisDeviceState::EnterAddress => &[K::Start],
        LinkThisDeviceState::Waiting { .. } => &[K::Check, K::NewCode, K::Reset],
        LinkThisDeviceState::Linked { .. } => &[K::Check, K::Reset],
    }
}

/// The flow of the new device.
#[derive(Clone)]
pub struct LinkThisDeviceFlow {
    client: Client,
    machine: Arc<Machine<LinkThisDeviceState, LinkThisDeviceActionKind>>,
    /// The device name that the links carry, and the last answer that was written.
    memory: Arc<Mutex<Memory>>,
    /// Held while an answer is written, so that a later answer is never overwritten by an
    /// earlier one.
    writing: Arc<tokio::sync::Mutex<()>>,
}

#[derive(Default)]
struct Memory {
    device_name: String,
    answered: Option<Answer>,
    /// The key of the device that approved this one, from its offer.
    old_key: Option<PublicKey>,
}

impl LinkThisDeviceFlow {
    pub(crate) fn new(client: Client) -> Self {
        Self {
            client,
            machine: Arc::new(Machine::new(
                LinkThisDeviceState::EnterAddress,
                link_this_device_actions,
            )),
            memory: Arc::default(),
            writing: Arc::default(),
        }
    }

    pub fn view(&self) -> FlowView<LinkThisDeviceState, LinkThisDeviceActionKind> {
        self.machine.view()
    }

    pub fn dispatch(
        &self,
        action: LinkThisDeviceAction,
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

    fn device_name(&self) -> String {
        lock(&self.memory).device_name.clone()
    }

    async fn perform(
        &self,
        before: LinkThisDeviceState,
        action: LinkThisDeviceAction,
    ) -> Outcome<LinkThisDeviceState> {
        match (action, before) {
            (LinkThisDeviceAction::Reset, _) => {
                *lock(&self.memory) = Memory::default();
                Outcome::moved(LinkThisDeviceState::EnterAddress)
            }
            (
                LinkThisDeviceAction::Start {
                    address,
                    device_name,
                },
                _,
            ) => {
                let device_name = match profile::device_name(&device_name) {
                    Ok(name) => name,
                    Err(error) => return Outcome::failed(error),
                };
                let handle = match self.saved_or_added(&address).await {
                    Ok(handle) => handle,
                    Err(error) => return Outcome::failed(error),
                };
                if let Err(error) = handle.connect_when_free().await {
                    return Outcome::failed(error);
                }
                if matches!(handle.access().state, AccessState::LoggedIn { .. }) {
                    return Outcome::failed(FlowError::AlreadyLinked);
                }
                lock(&self.memory).device_name.clone_from(&device_name);
                match request_link(&handle, &device_name).await {
                    Ok(()) => Outcome::moved(LinkThisDeviceState::Waiting { first: handle.id() }),
                    Err(error) => Outcome::failed(error),
                }
            }
            (LinkThisDeviceAction::NewCode, LinkThisDeviceState::Waiting { first }) => {
                let Some(handle) = self.client.server(first) else {
                    return Outcome::failed(FlowError::UnknownServer);
                };
                match request_link(&handle, &self.device_name()).await {
                    Ok(()) => Outcome::moved(LinkThisDeviceState::Waiting { first }),
                    Err(error) => Outcome::failed(error),
                }
            }
            (LinkThisDeviceAction::Check, LinkThisDeviceState::Waiting { first }) => {
                match check(&self.client, first).await {
                    LinkProgress::Linked => {
                        let linked = LinkThisDeviceState::Linked {
                            first,
                            servers: Vec::new(),
                            offer_read: false,
                        };
                        self.machine.update(|state| *state = linked);
                        self.client.notify();
                        self.work_on_offer(first, Vec::new(), false).await
                    }
                    LinkProgress::Waiting | LinkProgress::Working => {
                        Outcome::moved(LinkThisDeviceState::Waiting { first })
                    }
                    LinkProgress::Failed(error) => Outcome::failed(error),
                }
            }
            (
                LinkThisDeviceAction::Check,
                LinkThisDeviceState::Linked {
                    first,
                    servers,
                    offer_read,
                },
            ) => self.work_on_offer(first, servers, offer_read).await,
            (LinkThisDeviceAction::Check | LinkThisDeviceAction::NewCode, _) => {
                Outcome::failed(FlowError::NotAvailable)
            }
        }
    }

    /// The saved server with this address, or the server at this address, added.
    async fn saved_or_added(&self, address: &str) -> Result<ServerHandle, FlowError> {
        let parsed = ServerAddress::parse(address.trim())
            .map_err(|error| FlowError::InvalidInput(error.to_string()))?;
        if let Some(handle) = self
            .client
            .servers()
            .into_iter()
            .find(|handle| handle.entry().address == parsed)
        {
            return Ok(handle);
        }
        let adding = self.client.add_server();
        adding
            .dispatch(AddServerAction::Fetch {
                address: parsed.as_str().to_owned(),
            })
            .await?;
        let AddServerState::Preview(preview) = adding.view().state else {
            return Err(FlowError::NotAvailable);
        };
        adding
            .dispatch(AddServerAction::Save {
                connection_mode: ConnectionMode::default(),
            })
            .await?;
        self.client
            .server(preview.server_id)
            .ok_or(FlowError::UnknownServer)
    }

    /// Shows the progress of one server at once.
    fn publish(&self, server_id: ServerId, progress: LinkProgress) {
        self.machine.update(|state| {
            if let LinkThisDeviceState::Linked { servers, .. } = state
                && let Some(server) = servers
                    .iter_mut()
                    .find(|server| server.server_id == server_id)
            {
                server.progress = progress;
            }
        });
        self.client.notify();
    }

    async fn work_on_offer(
        &self,
        first: ServerId,
        mut servers: Vec<ServerLink>,
        mut offer_read: bool,
    ) -> Outcome<LinkThisDeviceState> {
        let Some(handle) = self.client.server(first) else {
            return Outcome::failed(FlowError::UnknownServer);
        };
        if !offer_read {
            match self.read_offer(&handle).await {
                Ok(Some(offered)) => {
                    servers = offered
                        .into_iter()
                        .map(|offered| ServerLink {
                            server_id: offered.server_id,
                            address: offered.address,
                            progress: LinkProgress::Working,
                        })
                        .collect();
                    offer_read = true;
                }
                Ok(None) => offer_read = true,
                Err(error) => {
                    let state = LinkThisDeviceState::Linked {
                        first,
                        servers,
                        offer_read,
                    };
                    return Outcome::failed_and_moved(state, error);
                }
            }
        }

        // A server that failed, and one whose code expired, gets another try; one that
        // waits is asked whether it linked this device.
        let mut work = Vec::new();
        for server in &mut servers {
            let ask = match &server.progress {
                LinkProgress::Linked => continue,
                LinkProgress::Working | LinkProgress::Failed(_) => true,
                LinkProgress::Waiting => false,
            };
            if ask {
                server.progress = LinkProgress::Working;
            }
            work.push((
                OfferedServer {
                    server_id: server.server_id,
                    address: server.address.clone(),
                },
                ask,
            ));
        }
        let shown = LinkThisDeviceState::Linked {
            first,
            servers,
            offer_read,
        };
        self.machine.update(|state| *state = shown);
        self.client.notify();

        // Each result goes to the other device as soon as it is known, so that a slow
        // server holds up the approval of no other (ADR 0716). A write that fails here is
        // tried again with the last answer below.
        let device_name = self.device_name();
        side_by_side(work, |(offered, ask)| {
            let (flow, device_name, handle) = (self.clone(), device_name.clone(), handle.clone());
            async move {
                let mut progress = if ask {
                    flow.ask(&offered, &device_name).await
                } else {
                    check(&flow.client, offered.server_id).await
                };
                if progress == LinkProgress::Failed(FlowError::Expired) {
                    progress = flow.ask(&offered, &device_name).await;
                }
                flow.publish(offered.server_id, progress);
                let _ = flow.answer_now(&handle, first).await;
            }
        })
        .await;

        let answered = self.answer_now(&handle, first).await;
        let LinkThisDeviceState::Linked { servers, .. } = self.machine.state() else {
            return Outcome::failed(FlowError::Stopped);
        };
        let state = LinkThisDeviceState::Linked {
            first,
            servers,
            offer_read,
        };
        match answered {
            Ok(()) => Outcome::moved(state),
            Err(error) => Outcome::failed_and_moved(state, error),
        }
    }

    /// The servers of the offer of the other device; `None` when the first server holds
    /// no offer for this device.
    async fn read_offer(
        &self,
        handle: &ServerHandle,
    ) -> Result<Option<Vec<OfferedServer>>, FlowError> {
        let read = handle.api().read_relay_offer().await;
        let offer = match read {
            Ok(offer) => offer,
            Err(error) if not_found(&error) => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let key = handle.device_key()?;
        let binding = Binding {
            first: handle.id(),
            new_device: key.public_key(),
        };
        let opened: Offer = relay::open(
            Kind::Offer,
            binding,
            &key,
            offer.old_public_key,
            &offer.offer,
        )?;
        lock(&self.memory).old_key = Some(offer.old_public_key);
        let mut seen = BTreeSet::from([handle.id()]);
        Ok(Some(
            opened
                .servers
                .into_iter()
                .filter(|offered| seen.insert(offered.server_id))
                .take(MOST_SERVERS)
                .collect(),
        ))
    }

    /// Writes the answer of the servers as they stand now, one write at a time.
    async fn answer_now(&self, handle: &ServerHandle, first: ServerId) -> Result<(), FlowError> {
        let _writing = self.writing.lock().await;
        let LinkThisDeviceState::Linked { servers, .. } = self.machine.state() else {
            return Ok(());
        };
        self.answer(handle, first, &servers).await?;
        self.client.notify();
        Ok(())
    }

    /// Writes the answer for the other device when it differs from the last one written.
    async fn answer(
        &self,
        handle: &ServerHandle,
        first: ServerId,
        servers: &[ServerLink],
    ) -> Result<(), FlowError> {
        if servers.is_empty() {
            return Ok(());
        }
        let links: Vec<AnsweredLink> = servers
            .iter()
            .filter_map(|server| {
                let answered = match &server.progress {
                    LinkProgress::Working => return None,
                    LinkProgress::Linked => Answered::Linked,
                    LinkProgress::Failed(error) => Answered::Failed {
                        reason: reason_of(error),
                    },
                    LinkProgress::Waiting => match self
                        .client
                        .server(server.server_id)
                        .map(|handle| handle.access().state)
                    {
                        Some(AccessState::WaitingForLink {
                            code, public_key, ..
                        }) if link_code_is_plain(&code) => Answered::Waiting { code, public_key },
                        _ => Answered::Failed {
                            reason: reason_of(&FlowError::NotAvailable),
                        },
                    },
                };
                Some(AnsweredLink {
                    server_id: server.server_id,
                    answered,
                })
            })
            .collect();
        let answer = Answer { links };
        if lock(&self.memory).answered.as_ref() == Some(&answer) {
            return Ok(());
        }
        let Some(old_key) = lock(&self.memory).old_key else {
            return Ok(());
        };
        let key = handle.device_key()?;
        let binding = Binding {
            first,
            new_device: key.public_key(),
        };
        let sealed = relay::seal(Kind::Answer, binding, &key, old_key, &answer)?;
        match handle.api().answer_relay(sealed).await {
            Ok(()) => {}
            // The device that approved ended the relay: nobody reads an answer any more.
            Err(error) if not_found(&error) => {}
            Err(error) => return Err(error.into()),
        }
        lock(&self.memory).answered = Some(answer);
        Ok(())
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
        // server at this moment is waited for (ADR 0712).
        handle.connect_when_free().await?;
        if matches!(handle.access().state, AccessState::LoggedIn { .. }) {
            return Ok(LinkProgress::Linked);
        }
        request_link(&handle, device_name).await?;
        Ok(LinkProgress::Waiting)
    }
}

async fn request_link(handle: &ServerHandle, device_name: &str) -> Result<(), FlowError> {
    handle.connect_when_free().await?;
    handle
        .dispatch_when_free(AccessAction::RequestLink {
            device_name: device_name.to_owned(),
        })
        .await
}

/// Asks a server that waits for a link whether it linked this device.
async fn check(client: &Client, server_id: ServerId) -> LinkProgress {
    let Some(handle) = client.server(server_id) else {
        return LinkProgress::Failed(FlowError::UnknownServer);
    };
    if !matches!(handle.access().state, AccessState::LoggedIn { .. })
        && let Err(error) = handle.dispatch_when_free(AccessAction::Check).await
    {
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

// The device that approves.

/// A server of this device, as the person checks it for the link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerChoice {
    pub server_id: ServerId,
    pub name: String,
    /// The account of this device there, which the new device is added to.
    pub account_id: Option<AccountId>,
    pub checked: bool,
    /// Why the server cannot be linked now. Such a server is unchecked.
    pub unavailable: Option<FlowError>,
}

/// Where the link of the new device on one server stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApprovalProgress {
    /// The new device has not answered for this server yet.
    Waiting,
    /// The client is asking the server now.
    Working,
    Approved,
    /// The new device has an account on this server already.
    AlreadyLinked,
    Failed(FlowError),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerApproval {
    pub server_id: ServerId,
    pub name: String,
    pub account_id: AccountId,
    pub progress: ApprovalProgress,
    /// The code of the last answer that was tried and settled, so that a new code of the
    /// same server is tried and a settled code is not tried again. A try that failed for a
    /// passing reason leaves it empty, so that the next check tries the code again.
    tried: Option<LinkCode>,
    /// The last approval may have added the device without an answer that said so; the
    /// next try reads the list of devices first.
    uncertain: bool,
}

/// What a try on another server leaves for the next check.
enum Tried {
    Settled,
    Again,
    Uncertain,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkNewDeviceState {
    EnterCode {
        servers: Vec<ServerChoice>,
    },
    /// The code was found on `first`. The person compares the key of the new device with
    /// the one that the new device shows, and then approves. `first` stays checked.
    Confirm {
        servers: Vec<ServerChoice>,
        first: ServerId,
        pending: PendingApproval,
    },
    /// The first server linked the new device. The other servers follow its answers.
    Linking {
        first: ServerId,
        device: Device,
        /// The key that the person compared. Every message of the relay is bound to it and
        /// checked against it, whatever key a server names.
        compared: PublicKey,
        servers: Vec<ServerApproval>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkNewDeviceAction {
    Choose {
        server_id: ServerId,
        checked: bool,
    },
    /// Looks the code up on every checked server.
    LookUp {
        code: LinkCode,
    },
    /// Approves the key on the first server, with the other checked servers in the relay.
    Approve,
    /// Reads the answer of the new device and approves each link that it names, when its
    /// server holds the same key behind the code.
    Check,
    Reset,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LinkNewDeviceActionKind {
    Choose,
    LookUp,
    Approve,
    Check,
    Reset,
}

impl LinkNewDeviceAction {
    pub fn kind(&self) -> LinkNewDeviceActionKind {
        match self {
            Self::Choose { .. } => LinkNewDeviceActionKind::Choose,
            Self::LookUp { .. } => LinkNewDeviceActionKind::LookUp,
            Self::Approve => LinkNewDeviceActionKind::Approve,
            Self::Check => LinkNewDeviceActionKind::Check,
            Self::Reset => LinkNewDeviceActionKind::Reset,
        }
    }
}

pub fn link_new_device_actions(state: &LinkNewDeviceState) -> &'static [LinkNewDeviceActionKind] {
    use LinkNewDeviceActionKind as K;
    match state {
        LinkNewDeviceState::EnterCode { .. } => &[K::Choose, K::LookUp],
        LinkNewDeviceState::Confirm { .. } => &[K::Choose, K::Approve, K::Reset],
        LinkNewDeviceState::Linking { .. } => &[K::Check, K::Reset],
    }
}

/// The flow of the device that approves.
#[derive(Clone)]
pub struct LinkNewDeviceFlow {
    client: Client,
    machine: Arc<Machine<LinkNewDeviceState, LinkNewDeviceActionKind>>,
    only: Option<ServerId>,
}

fn unavailable(handle: &ServerHandle) -> Option<FlowError> {
    match handle.access().state {
        AccessState::LoggedIn { .. } => None,
        _ => Some(FlowError::NotLoggedIn),
    }
}

impl LinkNewDeviceFlow {
    /// With `only`, only that server starts checked.
    pub(crate) fn new(client: Client, only: Option<ServerId>) -> Self {
        let flow = Self {
            client,
            machine: Arc::new(Machine::new(
                LinkNewDeviceState::EnterCode {
                    servers: Vec::new(),
                },
                link_new_device_actions,
            )),
            only,
        };
        let servers = flow.choices(&[]);
        flow.machine
            .update(|state| *state = LinkNewDeviceState::EnterCode { servers });
        flow
    }

    /// The servers of this device in the order of the book, with what the person chose
    /// before kept.
    fn choices(&self, before: &[ServerChoice]) -> Vec<ServerChoice> {
        self.client
            .servers()
            .iter()
            .map(|handle| {
                let server_id = handle.id();
                let unavailable = unavailable(handle);
                let wanted = before
                    .iter()
                    .find(|choice| choice.server_id == server_id)
                    .map_or_else(
                        || self.only.is_none_or(|only| only == server_id),
                        |choice| choice.checked || choice.unavailable.is_some(),
                    );
                ServerChoice {
                    server_id,
                    name: handle.entry().name,
                    account_id: handle.account(),
                    checked: wanted && unavailable.is_none(),
                    unavailable,
                }
            })
            .collect()
    }

    pub fn view(&self) -> FlowView<LinkNewDeviceState, LinkNewDeviceActionKind> {
        self.machine.view()
    }

    pub fn dispatch(
        &self,
        action: LinkNewDeviceAction,
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

    async fn perform(
        &self,
        before: LinkNewDeviceState,
        action: LinkNewDeviceAction,
    ) -> Outcome<LinkNewDeviceState> {
        match (action, before) {
            (LinkNewDeviceAction::Reset, _) => Outcome::moved(LinkNewDeviceState::EnterCode {
                servers: self.choices(&[]),
            }),
            (LinkNewDeviceAction::Choose { server_id, checked }, mut state) => {
                let servers = match &mut state {
                    LinkNewDeviceState::Confirm { first, .. } if *first == server_id => {
                        return Outcome::failed(FlowError::NotAvailable);
                    }
                    LinkNewDeviceState::EnterCode { servers }
                    | LinkNewDeviceState::Confirm { servers, .. } => servers,
                    LinkNewDeviceState::Linking { .. } => {
                        return Outcome::failed(FlowError::NotAvailable);
                    }
                };
                if let Some(choice) = servers
                    .iter_mut()
                    .find(|choice| choice.server_id == server_id)
                {
                    choice.checked = checked && choice.unavailable.is_none();
                }
                Outcome::moved(state)
            }
            (LinkNewDeviceAction::LookUp { code }, LinkNewDeviceState::EnterCode { servers }) => {
                self.look_up(code, servers).await
            }
            (
                LinkNewDeviceAction::Approve,
                LinkNewDeviceState::Confirm {
                    servers,
                    first,
                    pending,
                },
            ) => self.approve_first(servers, first, pending).await,
            (
                LinkNewDeviceAction::Check,
                LinkNewDeviceState::Linking {
                    first,
                    device,
                    compared,
                    servers,
                },
            ) => self.follow(first, device, compared, servers).await,
            (
                LinkNewDeviceAction::LookUp { .. }
                | LinkNewDeviceAction::Approve
                | LinkNewDeviceAction::Check,
                _,
            ) => Outcome::failed(FlowError::NotAvailable),
        }
    }

    async fn look_up(
        &self,
        code: LinkCode,
        before: Vec<ServerChoice>,
    ) -> Outcome<LinkNewDeviceState> {
        let servers = self.choices(&before);
        let asked: Vec<(usize, ServerChoice)> = servers
            .iter()
            .filter(|choice| choice.checked)
            .cloned()
            .enumerate()
            .collect();
        if asked.is_empty() {
            return Outcome::failed_and_moved(
                LinkNewDeviceState::EnterCode { servers },
                FlowError::NoServerChosen,
            );
        }
        let found = Arc::new(Mutex::new(Vec::new()));
        side_by_side(asked, |(order, choice)| {
            let (client, found, code) = (self.client.clone(), found.clone(), code.clone());
            async move {
                let result = match (client.server(choice.server_id), choice.account_id) {
                    (Some(handle), Some(account_id)) => handle
                        .api_for(account_id)
                        .look_up_device_link(code.clone())
                        .await
                        .map(|pending| (account_id, pending))
                        .map_err(FlowError::from),
                    _ => Err(FlowError::NotLoggedIn),
                };
                lock(&found).push((order, choice.server_id, result));
            }
        })
        .await;
        let mut found = std::mem::take(&mut *lock(&found));
        found.sort_by_key(|(order, _, _)| *order);
        let mut first_error = None;
        for (_, server_id, result) in found {
            match result {
                Ok((account_id, pending)) => {
                    let pending = PendingApproval {
                        code,
                        public_key: pending.public_key,
                        device_name: pending
                            .device_name
                            .chars()
                            .filter(|character| !character.is_control())
                            .take(NAME_MAX_CHARS)
                            .collect(),
                        expires_at: pending.expires_at,
                        account_id,
                    };
                    return Outcome::moved(LinkNewDeviceState::Confirm {
                        servers,
                        first: server_id,
                        pending,
                    });
                }
                Err(error) => {
                    first_error.get_or_insert(error);
                }
            }
        }
        Outcome::failed_and_moved(
            LinkNewDeviceState::EnterCode { servers },
            first_error.unwrap_or(FlowError::NotAvailable),
        )
    }

    async fn approve_first(
        &self,
        servers: Vec<ServerChoice>,
        first: ServerId,
        pending: PendingApproval,
    ) -> Outcome<LinkNewDeviceState> {
        let confirm = || LinkNewDeviceState::Confirm {
            servers: servers.clone(),
            first,
            pending: pending.clone(),
        };
        let Some(handle) = self.client.server(first) else {
            return Outcome::failed_and_moved(confirm(), FlowError::UnknownServer);
        };
        let others: Vec<&ServerChoice> = servers
            .iter()
            .filter(|choice| choice.checked && choice.server_id != first)
            .collect();
        // The relay holds a bounded offer; more servers are refused before the approval,
        // and none is left out without a word.
        if others.len() > MOST_SERVERS {
            return Outcome::failed_and_moved(
                confirm(),
                FlowError::TooManyServers {
                    most: MOST_SERVERS + 1,
                },
            );
        }
        let relays = handle.entry().capabilities.device_link_relay;
        let alone = others.is_empty();
        let offer = if alone || !relays {
            None
        } else {
            let offer = Offer {
                servers: others
                    .iter()
                    .filter_map(|choice| {
                        let entry = self.client.server(choice.server_id)?.entry();
                        Some(OfferedServer {
                            server_id: entry.server_id,
                            address: entry.address,
                        })
                    })
                    .collect(),
            };
            let sealed = handle.device_key().and_then(|key| {
                let binding = Binding {
                    first,
                    new_device: pending.public_key,
                };
                relay::seal(Kind::Offer, binding, &key, pending.public_key, &offer)
                    .map_err(FlowError::from)
            });
            match sealed {
                Ok(sealed) => Some(sealed),
                Err(error) => return Outcome::failed_and_moved(confirm(), error),
            }
        };
        let api = handle.api_for(pending.account_id);
        let device = match approve_with(&api, pending.code.clone(), pending.public_key, offer).await
        {
            Approval::Approved(device) => *device,
            Approval::Refused(error) if needs_a_new_code(&error) => {
                return Outcome::failed_and_moved(
                    LinkNewDeviceState::EnterCode {
                        servers: self.choices(&servers),
                    },
                    FlowError::from(error),
                );
            }
            Approval::Refused(error) | Approval::Unknown(error) => {
                return Outcome::failed_and_moved(confirm(), FlowError::from(error));
            }
        };
        // A server that names another key than the compared one added a device that the
        // person did not compare: nothing of its relay is trusted.
        if device.public_key != pending.public_key {
            let _ = api.end_relay(device.id).await;
            return Outcome::failed_and_moved(
                LinkNewDeviceState::EnterCode {
                    servers: self.choices(&servers),
                },
                FlowError::OtherKeyApproved,
            );
        }
        let mut lines = vec![ServerApproval {
            server_id: first,
            name: handle.entry().name,
            account_id: pending.account_id,
            progress: ApprovalProgress::Approved,
            tried: None,
            uncertain: false,
        }];
        for choice in others {
            let Some(account_id) = choice.account_id else {
                continue;
            };
            lines.push(ServerApproval {
                server_id: choice.server_id,
                name: choice.name.clone(),
                account_id,
                progress: if relays {
                    ApprovalProgress::Waiting
                } else {
                    ApprovalProgress::Failed(FlowError::RelayNotAvailable)
                },
                tried: None,
                uncertain: false,
            });
        }
        let state = LinkNewDeviceState::Linking {
            first,
            device,
            compared: pending.public_key,
            servers: lines,
        };
        // Without other servers the first server alone was asked for, and it linked.
        if relays || alone {
            Outcome::moved(state)
        } else {
            Outcome::failed_and_moved(state, FlowError::RelayNotAvailable)
        }
    }

    fn publish(&self, approval: ServerApproval) {
        self.machine.update(|state| {
            if let LinkNewDeviceState::Linking { servers, .. } = state
                && let Some(server) = servers
                    .iter_mut()
                    .find(|server| server.server_id == approval.server_id)
            {
                *server = approval;
            }
        });
        self.client.notify();
    }

    async fn follow(
        &self,
        first: ServerId,
        device: Device,
        compared: PublicKey,
        mut servers: Vec<ServerApproval>,
    ) -> Outcome<LinkNewDeviceState> {
        let open = |servers| LinkNewDeviceState::Linking {
            first,
            device: device.clone(),
            compared,
            servers,
        };
        // A line that failed can still get a new code from the new device, so the relay
        // ends only when every server linked the new device.
        let open_lines = |servers: &[ServerApproval]| {
            servers.iter().any(|server| {
                !matches!(
                    server.progress,
                    ApprovalProgress::Approved | ApprovalProgress::AlreadyLinked
                )
            })
        };
        let Some(handle) = self.client.server(first) else {
            return Outcome::failed_and_moved(open(servers), FlowError::UnknownServer);
        };
        let first_account = servers
            .iter()
            .find(|server| server.server_id == first)
            .map(|server| server.account_id);
        let Some(first_account) = first_account else {
            return Outcome::failed_and_moved(open(servers), FlowError::NotAvailable);
        };
        let api = handle.api_for(first_account);
        if !open_lines(&servers) {
            let _ = api.end_relay(device.id).await;
            return Outcome::moved(open(servers));
        }
        let answer = match api.read_relay_answer(device.id).await {
            Ok(answer) => answer,
            Err(error) if not_found(&error) => {
                for server in &mut servers {
                    if matches!(
                        server.progress,
                        ApprovalProgress::Waiting | ApprovalProgress::Failed(_)
                    ) {
                        server.progress = ApprovalProgress::Failed(FlowError::Expired);
                    }
                }
                return Outcome::failed_and_moved(open(servers), FlowError::Expired);
            }
            Err(error) => return Outcome::failed_and_moved(open(servers), FlowError::from(error)),
        };
        // The answer must be signed by the key that the person compared.
        if answer.new_public_key != compared {
            return Outcome::failed_and_moved(open(servers), FlowError::NotSignedByDevice);
        }
        let Some(sealed) = answer.answer else {
            return Outcome::moved(open(servers));
        };
        let opened = handle.device_key().and_then(|key| {
            let binding = Binding {
                first,
                new_device: compared,
            };
            relay::open::<Answer>(Kind::Answer, binding, &key, compared, &sealed)
                .map_err(FlowError::from)
        });
        let answer = match opened {
            Ok(answer) => answer,
            Err(error) => return Outcome::failed_and_moved(open(servers), error),
        };

        let mut work = Vec::new();
        for server in &mut servers {
            if server.server_id == first {
                continue;
            }
            let Some(link) = answer
                .links
                .iter()
                .find(|link| link.server_id == server.server_id)
            else {
                continue;
            };
            match (&link.answered, &server.progress) {
                (_, ApprovalProgress::Approved | ApprovalProgress::AlreadyLinked) => {}
                (Answered::Linked, _) => server.progress = ApprovalProgress::AlreadyLinked,
                (Answered::Failed { reason }, _) => {
                    server.progress = ApprovalProgress::Failed(FlowError::OnNewDevice(
                        reason.chars().take(LONGEST_REASON).collect(),
                    ));
                }
                (Answered::Waiting { code, public_key }, _) => {
                    if server.tried.as_ref() != Some(code) && link_code_is_plain(code) {
                        // Set while the try runs, so that a check at the same time does not
                        // try the code twice; a passing failure clears it again.
                        server.progress = ApprovalProgress::Working;
                        server.tried = Some(code.clone());
                        work.push((server.clone(), code.clone(), *public_key));
                    }
                }
            }
        }
        self.machine.update(|state| *state = open(servers.clone()));
        self.client.notify();

        side_by_side(work, |(mut server, code, public_key)| {
            let flow = self.clone();
            async move {
                let (progress, tried) = flow.approve_other(&server, code, public_key).await;
                server.progress = progress;
                match tried {
                    Tried::Settled => server.uncertain = false,
                    Tried::Again => server.tried = None,
                    Tried::Uncertain => {
                        server.tried = None;
                        server.uncertain = true;
                    }
                }
                flow.publish(server);
            }
        })
        .await;

        let LinkNewDeviceState::Linking { servers, .. } = self.machine.state() else {
            return Outcome::failed(FlowError::Stopped);
        };
        if !open_lines(&servers) {
            let _ = api.end_relay(device.id).await;
        }
        let failed = servers
            .iter()
            .filter(|server| matches!(server.progress, ApprovalProgress::Failed(_)))
            .count();
        let total = servers.len();
        let state = open(servers);
        if failed == 0 {
            Outcome::moved(state)
        } else {
            Outcome::failed_and_moved(state, FlowError::Incomplete { failed, total })
        }
    }

    /// Approves the link of the new device on another server, for the account of its line,
    /// when that server holds the key of the answer behind the code.
    async fn approve_other(
        &self,
        server: &ServerApproval,
        code: LinkCode,
        public_key: PublicKey,
    ) -> (ApprovalProgress, Tried) {
        let failed = |error: ApiError| {
            let tried = if passing(&error) {
                Tried::Again
            } else {
                Tried::Settled
            };
            (ApprovalProgress::Failed(error.into()), tried)
        };
        let Some(handle) = self.client.server(server.server_id) else {
            return (
                ApprovalProgress::Failed(FlowError::UnknownServer),
                Tried::Settled,
            );
        };
        let api = handle.api_for(server.account_id);
        // An earlier approval may have added the device; then its code is spent, and the
        // list of devices tells the truth before anything is repeated.
        if server.uncertain {
            match device_with_key(&api, public_key).await {
                Ok(Some(_)) => return (ApprovalProgress::Approved, Tried::Settled),
                Ok(None) => {}
                Err(error) => return (ApprovalProgress::Failed(error.into()), Tried::Uncertain),
            }
        }
        match api.look_up_device_link(code.clone()).await {
            Ok(pending) if pending.public_key == public_key => {}
            Ok(_) => {
                return (
                    ApprovalProgress::Failed(FlowError::refused(ErrorCode::PublicKeyMismatch)),
                    Tried::Settled,
                );
            }
            Err(error) => return failed(error),
        }
        match approve_with(&api, code, public_key, None).await {
            Approval::Approved(_) => (ApprovalProgress::Approved, Tried::Settled),
            Approval::Refused(error) => failed(error),
            Approval::Unknown(error) => (ApprovalProgress::Failed(error.into()), Tried::Uncertain),
        }
    }
}

/// True when the code cannot be approved any more as it was shown.
fn needs_a_new_code(error: &ApiError) -> bool {
    matches!(
        error.code(),
        Some(ErrorCode::LinkCodeInvalid | ErrorCode::PublicKeyMismatch)
    ) || matches!(error, ApiError::AccountChanged { .. })
}

impl Client {
    /// The flow in which this device is the new one: it asks one server of the account for
    /// a link code and then links on the servers that the other device names (ADR 0104).
    pub fn link_this_device(&self) -> LinkThisDeviceFlow {
        LinkThisDeviceFlow::new(self.clone())
    }

    /// The flow in which this device approves a new one. With `only`, only that server
    /// starts checked; the person can check the others.
    pub fn link_new_device(&self, only: Option<ServerId>) -> LinkNewDeviceFlow {
        LinkNewDeviceFlow::new(self.clone(), only)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_link_code_is_short_upper_case_text() {
        assert!(link_code_is_plain(&LinkCode::new("ABCD-EFGH")));
        assert!(!link_code_is_plain(&LinkCode::new("abcd-efgh")));
        assert!(!link_code_is_plain(&LinkCode::new("")));
        assert!(!link_code_is_plain(&LinkCode::new("A".repeat(33))));
        assert!(!link_code_is_plain(&LinkCode::new("ABCD EFGH")));
    }

    #[test]
    fn a_reason_for_the_other_device_is_one_bounded_line() {
        let reason = reason_of(&FlowError::InvalidInput(format!("a\nb{}", "c".repeat(300))));
        assert!(!reason.contains('\n'));
        assert_eq!(reason.chars().count(), LONGEST_REASON);
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
