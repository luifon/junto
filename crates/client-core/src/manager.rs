//! The client: the saved servers, and for each of them its own state, session and access
//! flow (ADR 0101, ADR 0107). A server that fails or cannot be reached changes its own
//! state only.

use std::future::Future;
use std::io;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use junto_protocol::account::RecoveryNotice;
use junto_protocol::error::ErrorCode;
use junto_protocol::ids::{AccountId, ServerId};
use junto_protocol::invite::JoinRequestState;
use junto_protocol::keys::PublicKey;
use time::OffsetDateTime;
use tokio::sync::watch;

use crate::address::ServerAddress;
use crate::api::session::{Authority, AuthorityFuture, Outline, send_with_token};
use crate::api::{
    AccessLoss, ApiClient, ApiError, ErrorCategory, ExpectedAccount, HttpResponse,
    ReqwestTransport, RetryPolicy, ServerApi, Transport, TransportError, server_info,
};
use crate::book::{
    BookError, ConnectionMode, EntryKey, EventCursor, Membership, ProfileSync, ServerBook,
    ServerEntry,
};
use crate::events::{ConnectionLevel, EventStreams, StreamTarget};
use crate::flow::access::{
    self, AccessAction, AccessState, ActionKind, Notice, Probe, Settled, after_probe,
    after_request_status,
};
use crate::flow::add_server::AddServerFlow;
use crate::flow::approve_link::ApproveLinkFlow;
use crate::flow::link_all::{
    ApproveDevicesFlow, LinkDeviceFlow, LinkOffer, SERVERS_AT_ONCE, link_code_is_plain,
};
use crate::flow::machine::lock;
use crate::flow::{FlowError, FlowView};
use crate::keys::{self, DeviceKey, FileKeyStore, KeyStore, KeyStoreError};
use crate::paths::DataDir;
use crate::profile::ProfileStore;
use crate::profile_sync::{ProfileUpdate, ProfileUploader, WaitsForServerOperation};
use crate::runtime::Runtime;
use crate::secret::SecretToken;
use crate::storage::StorageError;
use crate::version::{ApiVersion, Incompatibility, SupportedVersions};

/// How a client is opened. Every part except the data directory has a default.
pub struct ClientOptions {
    pub data_dir: PathBuf,
    pub transport: Option<Arc<dyn Transport>>,
    pub key_store: Option<Arc<dyn KeyStore>>,
    pub versions: SupportedVersions,
    pub retry: RetryPolicy,
    pub runtime: Option<Runtime>,
    /// What sends the profile preset to a new account. The default waits for the server
    /// operation (ADR 0105).
    pub profile_uploader: Option<Arc<dyn ProfileUploader>>,
}

impl ClientOptions {
    pub fn new(data_dir: impl Into<PathBuf>) -> Self {
        Self {
            data_dir: data_dir.into(),
            transport: None,
            key_store: None,
            versions: SupportedVersions::THIS_BUILD,
            retry: RetryPolicy::default(),
            runtime: None,
            profile_uploader: None,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum OpenError {
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error(transparent)]
    Transport(#[from] TransportError),
    #[error("the runtime of the client could not start: {0}")]
    Runtime(#[from] io::Error),
}

/// What the client knows about the reachability of one server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reach {
    /// The client has not contacted the server since it started.
    Unknown,
    Reachable {
        version: ApiVersion,
    },
    Unreachable,
    /// The server and this client share no API version (ADR 0201). Only this server is
    /// unavailable.
    Incompatible(Incompatibility),
    /// The address answers with another server identifier than the saved one. The client
    /// sends this address no key proof and no session token.
    OtherServer {
        reported: ServerId,
    },
}

pub(crate) struct Core {
    pub(crate) data_dir: DataDir,
    pub(crate) book: ServerBook,
    pub(crate) profile: ProfileStore,
    pub(crate) keys: Arc<dyn KeyStore>,
    pub(crate) transport: Arc<dyn Transport>,
    pub(crate) versions: SupportedVersions,
    pub(crate) policy: RetryPolicy,
    pub(crate) runtime: Runtime,
    uploader: Arc<dyn ProfileUploader>,
    /// Runs between the snapshot and the write of a save. Only tests set it, to fix the
    /// order of two saves.
    #[cfg(test)]
    before_write: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    selected: Mutex<Option<ServerId>>,
    streams: Mutex<Option<Arc<dyn EventStreams>>>,
    changes: watch::Sender<u64>,
}

impl Core {
    pub(crate) fn notify(&self) {
        self.changes.send_modify(|generation| *generation += 1);
    }
}

pub(crate) struct Inner {
    pub(crate) core: Arc<Core>,
    servers: Mutex<Vec<Arc<Server>>>,
}

impl Inner {
    pub(crate) fn adopt(&self, entry: ServerEntry) -> ServerId {
        let server = Server::new(self.core.clone(), entry);
        let id = server.id;
        lock(&self.servers).push(server);
        self.core.notify();
        id
    }
}

/// The client of one data directory.
#[derive(Clone)]
pub struct Client {
    pub(crate) inner: Arc<Inner>,
}

impl Client {
    /// Opens the data directory and reads the server book and the profile preset. A file
    /// that cannot be read stops the start. No server is contacted.
    pub fn open(options: ClientOptions) -> Result<Self, OpenError> {
        let data_dir = DataDir::open(options.data_dir)?;
        let book = ServerBook::new(data_dir.clone());
        let entries = book.entries()?;
        let profile = ProfileStore::new(data_dir.clone());
        profile.load()?;
        let transport = match options.transport {
            Some(transport) => transport,
            None => Arc::new(ReqwestTransport::new()?),
        };
        let keys = options
            .key_store
            .unwrap_or_else(|| Arc::new(FileKeyStore::new(data_dir.clone())));
        let runtime = match options.runtime {
            Some(runtime) => runtime,
            None => Runtime::current_or_new()?,
        };
        let core = Arc::new(Core {
            profile,
            data_dir,
            book,
            keys,
            transport,
            versions: options.versions,
            policy: options.retry,
            runtime,
            uploader: options
                .profile_uploader
                .unwrap_or_else(|| Arc::new(WaitsForServerOperation)),
            #[cfg(test)]
            before_write: Mutex::new(None),
            selected: Mutex::new(None),
            streams: Mutex::new(None),
            changes: watch::Sender::new(0),
        });
        let servers = entries
            .into_iter()
            .map(|entry| Server::new(core.clone(), entry))
            .collect();
        Ok(Self {
            inner: Arc::new(Inner {
                core,
                servers: Mutex::new(servers),
            }),
        })
    }

    pub fn data_dir(&self) -> &DataDir {
        &self.inner.core.data_dir
    }

    /// The local profile preset (ADR 0105).
    pub fn profile(&self) -> &ProfileStore {
        &self.inner.core.profile
    }

    /// The saved servers in the order of the server book.
    pub fn servers(&self) -> Vec<ServerHandle> {
        lock(&self.inner.servers)
            .iter()
            .map(|server| ServerHandle {
                server: server.clone(),
            })
            .collect()
    }

    pub fn server(&self, server_id: ServerId) -> Option<ServerHandle> {
        lock(&self.inner.servers)
            .iter()
            .find(|server| server.id == server_id)
            .map(|server| ServerHandle {
                server: server.clone(),
            })
    }

    /// Starts the flow that adds a server by its address.
    pub fn add_server(&self) -> AddServerFlow {
        AddServerFlow::new(self.inner.clone())
    }

    /// The servers on which this device has an account, for a new device that is to be
    /// linked on all of them (ADR 0104).
    pub fn link_offer(&self) -> LinkOffer {
        LinkOffer::of(self.servers().iter().map(ServerHandle::entry))
    }

    /// Starts the flow in which this new device asks for a link on every server of an
    /// offer (ADR 0104).
    pub fn link_this_device(&self) -> LinkDeviceFlow {
        LinkDeviceFlow::new(self.clone())
    }

    /// Starts the flow in which this device approves a new device on each of its servers
    /// (ADR 0104).
    pub fn approve_devices(&self) -> ApproveDevicesFlow {
        ApproveDevicesFlow::new(self.clone())
    }

    pub(crate) fn runtime(&self) -> &Runtime {
        &self.inner.core.runtime
    }

    pub(crate) fn notify(&self) {
        self.inner.core.notify();
    }

    /// Removes a server from the book and deletes the key of this device for it, in one
    /// step under the lock of the data directory. The account on the server is not
    /// changed; without the key this device cannot use it. A server whose flow runs an
    /// action is not removed. An entry that another client removed and added again is
    /// not removed, and its key stays.
    pub fn remove_server(&self, server_id: ServerId) -> Result<(), RemoveError> {
        let server = lock(&self.inner.servers)
            .iter()
            .find(|server| server.id == server_id)
            .cloned()
            .ok_or(BookError::NotSaved { server_id })?;
        let Ok(_gate) = server.gate.try_lock() else {
            return Err(RemoveError::Busy);
        };
        let keys = self.inner.core.keys.clone();
        let removed = self.inner.core.book.remove(server.key, || {
            keys.remove(server_id).map_err(RemoveError::from)
        });
        if matches!(&removed, Err(RemoveError::Book(BookError::Storage(_)))) {
            return removed.map(|_| ());
        }
        lock(&self.inner.servers).retain(|kept| !Arc::ptr_eq(kept, &server));
        server.retire();
        {
            let mut selected = lock(&self.inner.core.selected);
            if *selected == Some(server_id) {
                *selected = None;
            }
        }
        self.inner.core.notify();
        removed.map(|_| ())
    }

    /// The server that the interface shows. It gets every event; the others get the
    /// background events (ADR 0107).
    pub fn select(&self, server_id: Option<ServerId>) {
        *lock(&self.inner.core.selected) = server_id;
        for handle in self.servers() {
            handle.server.announce_level();
        }
        self.inner.core.notify();
    }

    pub fn selected(&self) -> Option<ServerId> {
        *lock(&self.inner.core.selected)
    }

    /// Attaches the event streams and tells them the present level of every server.
    pub fn attach_event_streams(&self, streams: Arc<dyn EventStreams>) {
        *lock(&self.inner.core.streams) = Some(streams);
        for handle in self.servers() {
            handle.server.forget_announcement();
            handle.server.announce_level();
        }
    }

    /// Contacts the saved servers side by side, a bounded number at a time, and brings
    /// each to its present state: logged in, waiting, banned, removed or unreachable. The
    /// result of one server does not depend on another.
    pub fn connect_all(
        &self,
    ) -> impl Future<Output = Vec<(ServerId, Result<(), FlowError>)>> + Send + 'static + use<> {
        let slots = Arc::new(tokio::sync::Semaphore::new(SERVERS_AT_ONCE));
        let pending: Vec<_> = self
            .servers()
            .into_iter()
            .map(|handle| {
                let (server_id, slots) = (handle.id(), slots.clone());
                let connecting = self.inner.core.runtime.run(async move {
                    let _slot = slots.acquire_owned().await;
                    handle.connect().await
                });
                (server_id, connecting)
            })
            .collect();
        async move {
            let mut results = Vec::with_capacity(pending.len());
            for (server_id, connecting) in pending {
                let result = match connecting.await {
                    Ok(result) => result,
                    Err(stopped) => Err(stopped.into()),
                };
                results.push((server_id, result));
            }
            results
        }
    }

    /// A value that reports each change of any server or of the list of servers.
    pub fn changes(&self) -> Changes {
        Changes {
            receiver: self.inner.core.changes.subscribe(),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum RemoveError {
    #[error("an action of the server is in progress")]
    Busy,
    #[error(transparent)]
    Book(#[from] BookError),
    #[error(transparent)]
    Key(#[from] KeyStoreError),
}

/// Wakes an interface when something that it shows has changed.
pub struct Changes {
    receiver: watch::Receiver<u64>,
}

impl Changes {
    /// Waits for the next change. False when the client does not exist any more.
    pub async fn changed(&mut self) -> bool {
        self.receiver.changed().await.is_ok()
    }
}

/// A session and the account that it belongs to.
#[derive(Clone)]
struct Held {
    token: SecretToken,
    account_id: AccountId,
}

struct ServerState {
    entry: ServerEntry,
    reach: Reach,
    access: AccessState,
    in_progress: Option<ActionKind>,
    error: Option<FlowError>,
    api: Option<ApiClient>,
    session: Option<Held>,
    /// The recoveries that the server told at a login and that nobody has taken yet.
    notices: Vec<RecoveryNotice>,
    announced: Option<(ConnectionLevel, Option<SecretToken>)>,
    /// The membership or the profile state in memory is not in the book yet: a write
    /// failed. It is cleared only by a write that succeeded.
    unsaved: bool,
    retired: bool,
}

pub(crate) struct Server {
    id: ServerId,
    /// The entry of the book that this object was made from.
    key: EntryKey,
    address: ServerAddress,
    core: Arc<Core>,
    state: Mutex<ServerState>,
    /// Held while an action of the access flow or a renewal of the session runs.
    gate: tokio::sync::Mutex<()>,
    /// Held from the snapshot of what a save writes to the clearing of the unsaved mark,
    /// so that two saves cannot write in another order than they read.
    saving: Mutex<()>,
}

/// One saved server.
#[derive(Clone)]
pub struct ServerHandle {
    server: Arc<Server>,
}

impl ServerHandle {
    pub fn id(&self) -> ServerId {
        self.server.id
    }

    /// The entry of the server book as the client holds it now.
    pub fn entry(&self) -> ServerEntry {
        lock(&self.server.state).entry.clone()
    }

    pub fn reach(&self) -> Reach {
        lock(&self.server.state).reach
    }

    /// The access flow: its state, its actions, the running action and the last error.
    pub fn access(&self) -> FlowView<AccessState, ActionKind> {
        let state = lock(&self.server.state);
        let actions = match (state.in_progress, state.retired) {
            (None, false) => access::actions(&state.access).to_vec(),
            _ => Vec::new(),
        };
        FlowView {
            state: state.access.clone(),
            in_progress: state.in_progress,
            actions,
            error: state.error.clone(),
        }
    }

    /// Runs one action of the access flow. The action runs to its end when the returned
    /// future is dropped. After an action that created an account, the profile preset is
    /// sent when the server offers that operation (ADR 0105).
    pub fn dispatch(
        &self,
        action: AccessAction,
    ) -> impl Future<Output = Result<(), FlowError>> + Send + 'static + use<> {
        let server = self.server.clone();
        let work = self.server.core.runtime.run(async move {
            let result = server.clone().dispatch(action).await;
            if result.is_ok() {
                let _ = server.apply_profile().await;
            }
            result
        });
        async move { work.await? }
    }

    /// Contacts the server and brings the flow to its present state. A device that
    /// belongs to an account logs in; no action of the user is needed (ADR 0109).
    pub fn connect(&self) -> impl Future<Output = Result<(), FlowError>> + Send + 'static + use<> {
        let kind = if matches!(self.access().state, AccessState::LoggedOut { .. }) {
            AccessAction::LogIn
        } else {
            AccessAction::Check
        };
        self.dispatch(kind)
    }

    /// Asks the server every `interval` until a join request or a device link is decided,
    /// and returns the state then. The first question is asked after one interval.
    /// Dropping the future stops the asking.
    pub fn wait_for_decision(
        &self,
        interval: Duration,
    ) -> impl Future<Output = Result<AccessState, FlowError>> + Send + 'static + use<> {
        let handle = self.clone();
        async move {
            let mut pause = interval;
            loop {
                let view = handle.access();
                let waiting = matches!(
                    view.state,
                    AccessState::WaitingForApproval { .. } | AccessState::WaitingForLink { .. }
                );
                if !waiting {
                    return Ok(view.state);
                }
                if handle.sleep(pause).await.is_err() {
                    return Err(FlowError::Stopped);
                }
                pause = interval;
                match handle.dispatch(AccessAction::Check).await {
                    Ok(())
                    | Err(FlowError::Unavailable | FlowError::Unreachable(_) | FlowError::Busy) => {
                    }
                    Err(FlowError::Limited { retry_after }) => {
                        pause = pause.max(retry_after.unwrap_or(interval));
                    }
                    Err(error) => return Err(error),
                }
            }
        }
    }

    fn sleep(
        &self,
        duration: Duration,
    ) -> impl Future<Output = Result<(), crate::runtime::Stopped>> + Send + 'static + use<> {
        self.server
            .core
            .runtime
            .run(async move { tokio::time::sleep(duration).await })
    }

    /// The recoveries of the account that the server told this device at its logins and
    /// that were not taken before. The server tells each device of a recovery once
    /// (ADR 0112), and a later login replaces the list in the state of the flow, so the
    /// client keeps every notice here until the interface takes it to show it.
    pub fn take_recoveries(&self) -> Vec<RecoveryNotice> {
        std::mem::take(&mut lock(&self.server.state).notices)
    }

    /// The account of the session that this device holds on the server now.
    pub fn account(&self) -> Option<AccountId> {
        lock(&self.server.state)
            .session
            .as_ref()
            .map(|held| held.account_id)
    }

    /// The operations of the logged-in account. Each request belongs to the account that
    /// the device has when the request starts. A session that ended is renewed with the
    /// device key once; a ban or a removal found then changes the state of the access
    /// flow and the call fails with [`ApiError::AccessLost`]. When the renewed session
    /// belongs to another account, the call fails with [`ApiError::AccountChanged`] and
    /// is not sent again.
    pub fn api(&self) -> ServerApi {
        ServerApi::new(Arc::new(Managed {
            server: self.server.clone(),
            account: None,
        }))
    }

    /// Like [`ServerHandle::api`], for one account only: a request fails with
    /// [`ApiError::AccountChanged`] when the device has another account. What a user
    /// confirmed for one account is sent through this.
    pub fn api_for(&self, account_id: AccountId) -> ServerApi {
        ServerApi::new(Arc::new(Managed {
            server: self.server.clone(),
            account: Some(account_id),
        }))
    }

    /// Starts the flow in which this logged-in device approves a new device (ADR 0112).
    pub fn approve_link(&self) -> ApproveLinkFlow {
        ApproveLinkFlow::new(self.clone())
    }

    pub(crate) fn core(&self) -> &Arc<Core> {
        &self.server.core
    }

    /// The public key of this device for the server, created when there is none. The
    /// server owner needs it for a recovery (ADR 0112).
    pub fn device_public_key(&self) -> Result<PublicKey, KeyStoreError> {
        match self.server.key_or_new() {
            Ok(key) => Ok(key.public_key()),
            Err(KeyFault::Key(error)) => Err(error),
            Err(KeyFault::Book(error)) => Err(KeyStoreError::Other(error.to_string())),
        }
    }

    pub fn set_connection_mode(&self, mode: ConnectionMode) -> Result<(), BookError> {
        let entry = self
            .server
            .core
            .book
            .update(self.server.key, |entry| entry.connection_mode = mode)?;
        lock(&self.server.state).entry.connection_mode = entry.connection_mode;
        self.server.announce_level();
        self.server.core.notify();
        Ok(())
    }

    /// Stores the position of this device in the event stream of the server.
    pub fn store_cursor(&self, cursor: EventCursor) -> Result<(), BookError> {
        let stored = cursor.clone();
        self.server
            .core
            .book
            .update(self.server.key, |entry| entry.last_cursor = Some(cursor))?;
        lock(&self.server.state).entry.last_cursor = Some(stored);
        Ok(())
    }

    /// The level that the event stream of this server should have now.
    pub fn connection_level(&self) -> ConnectionLevel {
        self.server.level().0
    }

    /// Whether the profile preset still waits to be sent to the account on this server.
    pub fn profile_sync(&self) -> ProfileSync {
        lock(&self.server.state).entry.profile_sync
    }

    /// Sends the profile preset to the account when it waits, and records the result: a
    /// success ends the waiting, a failure is counted. While the server does not offer
    /// the operation the call fails with [`ApiError::NotOffered`] and nothing changes.
    pub fn apply_profile(
        &self,
    ) -> impl Future<Output = Result<ProfileSync, ApiError>> + Send + 'static + use<> {
        let server = self.server.clone();
        let work = self
            .server
            .core
            .runtime
            .run(async move { server.apply_profile().await });
        async move { work.await.unwrap_or(Err(ApiError::Stopped)) }
    }
}

struct Managed {
    server: Arc<Server>,
    /// The only account that the requests may run for.
    account: Option<AccountId>,
}

impl Authority for Managed {
    fn send(&self, outline: Outline) -> AuthorityFuture {
        let (server, account) = (self.server.clone(), self.account);
        let work = self
            .server
            .core
            .runtime
            .run(async move { server.authorized(outline, account).await });
        Box::pin(async move { work.await.unwrap_or(Err(ApiError::Stopped)) })
    }
}

/// A running action of the access flow or a renewal of the session. Its end clears the
/// mark of the running action, also when the work stops before its end.
struct Running<'a>(&'a Server);

impl Running<'_> {
    fn finish(self, error: Option<FlowError>) {
        self.end(error);
        std::mem::forget(self);
    }

    fn end(&self, error: Option<FlowError>) {
        {
            let mut state = lock(&self.0.state);
            state.in_progress = None;
            state.error = error;
        }
        self.0.announce_level();
        self.0.core.notify();
    }
}

impl Drop for Running<'_> {
    fn drop(&mut self) {
        self.end(Some(FlowError::Stopped));
    }
}

fn held_of(probe: &Probe) -> Option<Held> {
    match probe {
        Probe::LoggedIn(session) => Some(Held {
            token: session.token.clone(),
            account_id: session.account_id,
        }),
        Probe::UnknownDevice | Probe::Banned => None,
    }
}

/// True when the server did not complete the operation for a reason that a login can
/// explain: the answer was lost, or the key already belongs to an account.
fn login_explains(error: &ApiError) -> bool {
    matches!(error, ApiError::OutcomeUnknown)
        || error.code() == Some(ErrorCode::KeyAlreadyRegistered)
}

enum KeyFault {
    Key(KeyStoreError),
    Book(BookError),
}

impl From<KeyFault> for FlowError {
    fn from(fault: KeyFault) -> Self {
        match fault {
            KeyFault::Key(error) => error.into(),
            KeyFault::Book(error) => error.into(),
        }
    }
}

impl Server {
    fn new(core: Arc<Core>, entry: ServerEntry) -> Arc<Self> {
        Arc::new(Self {
            id: entry.server_id,
            key: entry.key(),
            address: entry.address.clone(),
            core,
            state: Mutex::new(ServerState {
                access: access::initial(&entry.membership),
                entry,
                reach: Reach::Unknown,
                in_progress: None,
                error: None,
                api: None,
                session: None,
                notices: Vec::new(),
                announced: None,
                unsaved: false,
                retired: false,
            }),
            gate: tokio::sync::Mutex::new(()),
            saving: Mutex::new(()),
        })
    }

    fn retire(&self) {
        {
            let mut state = lock(&self.state);
            state.retired = true;
            state.session = None;
            state.api = None;
        }
        self.announce_level();
    }

    fn membership(&self) -> Membership {
        lock(&self.state).entry.membership.clone()
    }

    fn level(&self) -> (ConnectionLevel, Option<StreamTarget>) {
        let selected = *lock(&self.core.selected);
        let state = lock(&self.state);
        let (Some(api), Some(held), false) = (&state.api, &state.session, state.retired) else {
            return (ConnectionLevel::Off, None);
        };
        let level = if selected == Some(self.id) {
            ConnectionLevel::Selected
        } else if state.entry.connection_mode == ConnectionMode::Always {
            ConnectionLevel::Background
        } else {
            return (ConnectionLevel::Off, None);
        };
        let target = StreamTarget {
            address: state.entry.address.clone(),
            api_version: api.version(),
            token: held.token.clone(),
            cursor: state.entry.last_cursor.clone(),
        };
        (level, Some(target))
    }

    fn forget_announcement(&self) {
        lock(&self.state).announced = None;
    }

    /// Tells the event streams the level of this server when it or its session changed.
    fn announce_level(&self) {
        let Some(streams) = lock(&self.core.streams).clone() else {
            return;
        };
        let (level, target) = self.level();
        let now = (level, target.as_ref().map(|target| target.token.clone()));
        {
            let mut state = lock(&self.state);
            if state.announced.as_ref() == Some(&now) {
                return;
            }
            if state.announced.is_none() && level == ConnectionLevel::Off {
                state.announced = Some(now);
                return;
            }
            state.announced = Some(now);
        }
        streams.set_level(self.id, level, target);
    }

    async fn dispatch(self: Arc<Self>, action: AccessAction) -> Result<(), FlowError> {
        let Ok(_gate) = self.gate.try_lock() else {
            return Err(FlowError::Busy);
        };
        let kind = action.kind();
        let before = {
            let mut state = lock(&self.state);
            if state.retired || !access::actions(&state.access).contains(&kind) {
                return Err(FlowError::NotAvailable);
            }
            state.in_progress = Some(kind);
            state.error = None;
            state.access.clone()
        };
        self.core.notify();
        let running = Running(&self);
        let mut result = match action.checked() {
            Ok(action) => self.perform(&before, action).await,
            Err(error) => Err(error),
        };
        if result.is_ok() {
            self.answered();
            result = self.save();
        }
        running.finish(result.as_ref().err().cloned());
        result
    }

    async fn perform(&self, before: &AccessState, action: AccessAction) -> Result<(), FlowError> {
        match action {
            AccessAction::Check => {
                let api = self.contact(true).await?;
                self.reconcile(&api, before).await
            }
            AccessAction::LogIn => {
                let api = self.contact(false).await?;
                self.reconcile(&api, before).await
            }
            AccessAction::LogOut => self.log_out().await,
            AccessAction::ForgetAccount => self.forget_account(),
            AccessAction::Join {
                code,
                name,
                device_name,
            } => {
                let api = self.contact(false).await?;
                let key = self.key_or_new()?;
                match api.join(&key, code, name, device_name).await {
                    Ok(session) => self.settle_probe(before, Probe::LoggedIn(session), true),
                    Err(error) => self.explain_by_login(&api, &key, before, error).await,
                }
            }
            AccessAction::ClaimOwner {
                code,
                name,
                device_name,
            } => {
                let api = self.contact(false).await?;
                let key = self.key_or_new()?;
                match api.claim_owner(&key, code, name, device_name).await {
                    Ok(session) => {
                        // A claim that returns the owner to an existing account carries
                        // the notice of that recovery (ADR 0114).
                        let created = session.recoveries.is_empty();
                        self.settle_probe(before, Probe::LoggedIn(session), created)
                    }
                    Err(error) => self.explain_by_login(&api, &key, before, error).await,
                }
            }
            AccessAction::RequestAccess { name } => self.request_access(before, name).await,
            AccessAction::RequestLink { device_name } => {
                let api = self.contact(false).await?;
                let key = self.key_or_new()?;
                match api.request_device_link(&key, device_name).await {
                    Ok(link) if !link_code_is_plain(&link.code) => {
                        // A code that a person cannot type or that does not fit a link
                        // request is not shown and not passed on.
                        Err(self.failed(ApiError::InvalidResponse {
                            status: 201,
                            reason: "link code",
                        }))
                    }
                    Ok(link) => self.settle(
                        Settled {
                            state: AccessState::WaitingForLink {
                                code: link.code,
                                public_key: key.public_key(),
                                expires_at: link.expires_at,
                            },
                            membership: Membership::None,
                        },
                        None,
                    ),
                    Err(error) => self.explain_by_login(&api, &key, before, error).await,
                }
            }
        }
    }

    /// Asks for access without a code. The intent is stored before the request is sent,
    /// so a client that stops before it stores the answer asks the server at its next
    /// start what became of the request.
    async fn request_access(&self, before: &AccessState, name: String) -> Result<(), FlowError> {
        let api = self.contact(false).await?;
        let key = self.key_or_new()?;
        let previous = self.membership();
        self.remember(Membership::JoinRequested {
            join_request_id: None,
            name: name.clone(),
        })?;
        let waiting = |join_request_id| Settled {
            state: AccessState::WaitingForApproval {
                join_request_id,
                name: name.clone(),
            },
            membership: Membership::JoinRequested {
                join_request_id,
                name: name.clone(),
            },
        };
        match api.request_to_join(&key, name.clone()).await {
            Ok(request) => self.settle(waiting(Some(request.id)), None),
            Err(error)
                if matches!(error, ApiError::OutcomeUnknown)
                    || error.code() == Some(ErrorCode::JoinRequestPending) =>
            {
                match api.join_request_status(&key).await {
                    Ok(status) => match after_request_status(&status, &name) {
                        Some(settled) => self.settle(settled, None),
                        None => self.reconcile(&api, before).await,
                    },
                    Err(lookup) if lookup.code() == Some(ErrorCode::NotFound) => {
                        let _ = self.remember(previous);
                        Err(self.failed(error))
                    }
                    Err(_) => {
                        // Nobody knows whether the server holds the request. The flow
                        // waits without an identifier, and a check finds the answer.
                        let _ = self.settle(waiting(None), None);
                        Err(self.failed(error))
                    }
                }
            }
            Err(error) => {
                let _ = self.remember(previous);
                self.explain_by_login(&api, &key, before, error).await
            }
        }
    }

    /// Asks the server for its information, compares its identifier with the saved one
    /// and chooses the API version. Before this succeeds nothing else is sent.
    async fn contact(&self, refresh: bool) -> Result<ApiClient, FlowError> {
        let cached = lock(&self.state).api.clone();
        if let (false, Some(api)) = (refresh, cached) {
            return Ok(api);
        }
        let info = server_info(
            self.core.transport.as_ref(),
            &self.address,
            &self.core.policy,
        )
        .await
        .map_err(|error| self.failed(error))?;
        if info.server_id != self.id {
            self.cut_off(Reach::OtherServer {
                reported: info.server_id,
            });
            return Err(FlowError::OtherServer);
        }
        let version = match self.core.versions.negotiate(&info.api_versions) {
            Ok(version) => version,
            Err(incompatibility) => {
                self.cut_off(Reach::Incompatible(incompatibility));
                return Err(FlowError::Incompatible(incompatibility));
            }
        };
        let api = ApiClient::new(
            self.core.transport.clone(),
            self.address.clone(),
            self.id,
            version,
            self.core.policy,
        );
        let refreshed = {
            let mut state = lock(&self.state);
            let mut entry = state.entry.clone();
            entry.refresh(&info);
            let changed = entry != state.entry;
            state.entry = entry.clone();
            state.reach = Reach::Reachable { version };
            if state.api.as_ref().map(ApiClient::version) != Some(version) {
                state.api = Some(api.clone());
            }
            changed.then_some(entry)
        };
        if let Some(entry) = refreshed {
            let stored = self.core.book.update(self.key, |saved| {
                saved.name = entry.name;
                saved.logo_path = entry.logo_path;
                saved.api_versions = entry.api_versions;
            });
            if let Err(error) = stored {
                tracing::warn!(server = %self.id, %error, "the server book was not updated");
            }
        }
        Ok(api)
    }

    /// Stops every use of the server: no cached API, no session.
    fn cut_off(&self, reach: Reach) {
        let mut state = lock(&self.state);
        state.reach = reach;
        state.api = None;
        state.session = None;
        if let AccessState::LoggedIn { account_id, .. } = state.access {
            state.access = AccessState::LoggedOut { account_id };
        }
    }

    /// Records a failed request and turns it into the error of the flow.
    fn failed(&self, error: ApiError) -> FlowError {
        self.note(&error);
        FlowError::from(&error)
    }

    fn note(&self, error: &ApiError) {
        tracing::debug!(server = %self.id, %error, "a request failed");
        match error {
            ApiError::Unreachable(_) | ApiError::OutcomeUnknown => self.unanswered(),
            ApiError::InvalidResponse { status, .. } if *status >= 500 => self.unanswered(),
            ApiError::Rejected(_) | ApiError::InvalidResponse { .. } => self.answered(),
            _ => {}
        }
    }

    fn unanswered(&self) {
        lock(&self.state).reach = Reach::Unreachable;
    }

    /// The server answered, so it is reachable again after a failure.
    fn answered(&self) {
        let mut state = lock(&self.state);
        if let (Some(api), Reach::Unreachable | Reach::Unknown) = (&state.api, state.reach) {
            state.reach = Reach::Reachable {
                version: api.version(),
            };
        }
    }

    /// The key of this device for the server, when it was created for the saved address.
    fn stored_key(&self) -> Result<Option<DeviceKey>, KeyStoreError> {
        self.core.keys.load(self.id, &self.address)
    }

    /// The stored key, or a new one. A key is created only while the book still has the
    /// entry of this object, under the lock of the book.
    fn key_or_new(&self) -> Result<DeviceKey, KeyFault> {
        let book = &self.core.book;
        let _lock = book.lock().map_err(|error| KeyFault::Book(error.into()))?;
        let saved = book
            .has(self.key)
            .map_err(|error| KeyFault::Book(error.into()))?;
        if !saved {
            return Err(KeyFault::Book(BookError::NotSaved { server_id: self.id }));
        }
        keys::load_or_create(self.core.keys.as_ref(), self.id, &self.address).map_err(KeyFault::Key)
    }

    async fn probe(&self, api: &ApiClient, key: &DeviceKey) -> Result<Probe, ApiError> {
        let mut attempt = api.log_in(key, ExpectedAccount::Any).await;
        if matches!(attempt, Err(ApiError::OutcomeUnknown)) {
            attempt = api.log_in(key, ExpectedAccount::Any).await;
        }
        match attempt {
            Ok(session) => Ok(Probe::LoggedIn(session)),
            Err(error) if error.code() == Some(ErrorCode::UnknownDevice) => {
                Ok(Probe::UnknownDevice)
            }
            Err(error) if error.code() == Some(ErrorCode::AccountBanned) => Ok(Probe::Banned),
            Err(error) => Err(error),
        }
    }

    /// Finds out what this device is on the server and moves the flow there.
    async fn reconcile(&self, api: &ApiClient, before: &AccessState) -> Result<(), FlowError> {
        let membership = self.membership();
        let Some(key) = self.stored_key()? else {
            let settled = match membership {
                Membership::None => Settled {
                    state: AccessState::NoAccount { notice: None },
                    membership: Membership::None,
                },
                Membership::Removed => Settled {
                    state: AccessState::NoAccount {
                        notice: Some(Notice::Removed),
                    },
                    membership: Membership::Removed,
                },
                Membership::Member { .. }
                | Membership::Banned { .. }
                | Membership::JoinRequested { .. } => Settled {
                    state: AccessState::NoAccount {
                        notice: Some(Notice::KeyMissing),
                    },
                    membership: Membership::None,
                },
            };
            return self.settle(settled, None);
        };
        let mut approved = false;
        if let Membership::JoinRequested { name, .. } = &membership {
            match api.join_request_status(&key).await {
                Ok(status) => match after_request_status(&status, name) {
                    Some(settled) => return self.settle(settled, None),
                    None => approved = true,
                },
                Err(error) if error.code() == Some(ErrorCode::NotFound) => {}
                Err(error) => return Err(self.failed(error)),
            }
        }
        let probe = self
            .probe(api, &key)
            .await
            .map_err(|error| self.failed(error))?;
        let unknown_without_a_trace = probe == Probe::UnknownDevice
            && matches!(membership, Membership::None | Membership::Removed)
            && !matches!(before, AccessState::WaitingForLink { .. });
        if unknown_without_a_trace
            && let Ok(status) = api.join_request_status(&key).await
            && status.state == JoinRequestState::Pending
            && let Some(settled) = after_request_status(&status, "")
        {
            // The server holds a request of this key that the book does not know: the
            // client stopped before it stored the request, or the book was replaced.
            return self.settle(settled, None);
        }
        self.settle_probe(before, probe, approved)
    }

    /// After an operation failed in a way that a login can explain, logs in. A session
    /// means that the device has its account; anything else leaves the first error.
    async fn explain_by_login(
        &self,
        api: &ApiClient,
        key: &DeviceKey,
        before: &AccessState,
        error: ApiError,
    ) -> Result<(), FlowError> {
        if !login_explains(&error) {
            return Err(self.failed(error));
        }
        let created = matches!(error, ApiError::OutcomeUnknown);
        match self.probe(api, key).await {
            Ok(Probe::UnknownDevice) | Err(_) => Err(self.failed(error)),
            Ok(probe) => self.settle_probe(before, probe, created),
        }
    }

    /// Moves the flow to the state that a login attempt gives. `created` says that the
    /// account is one that this device has just created, so the profile preset is sent
    /// to it (ADR 0105).
    fn settle_probe(
        &self,
        before: &AccessState,
        probe: Probe,
        created: bool,
    ) -> Result<(), FlowError> {
        let held = held_of(&probe);
        if let Probe::LoggedIn(session) = &probe {
            let mut state = lock(&self.state);
            for notice in &session.recoveries {
                if !state.notices.iter().any(|kept| kept.id == notice.id) {
                    state.notices.push(notice.clone());
                }
            }
        }
        if created && held.is_some() {
            let has_avatar =
                matches!(self.core.profile.load(), Ok(preset) if preset.avatar.is_some());
            let mut state = lock(&self.state);
            if has_avatar && state.entry.profile_sync == ProfileSync::NotNeeded {
                state.entry.profile_sync = ProfileSync::Pending { attempts: 0 };
                state.unsaved = true;
            }
        }
        let settled = after_probe(before, &self.membership(), probe, OffsetDateTime::now_utc());
        self.settle(settled, held)
    }

    /// Moves the flow to a state and stores the membership that belongs to it. The state
    /// in memory is what the server said. When the book cannot be written, the action
    /// reports that and the entry stays marked as unsaved until a write succeeds.
    fn settle(&self, settled: Settled, session: Option<Held>) -> Result<(), FlowError> {
        {
            let mut state = lock(&self.state);
            state.access = settled.state;
            state.session = session;
            if state.entry.membership != settled.membership {
                state.entry.membership = settled.membership;
                state.unsaved = true;
            }
        }
        self.save()
    }

    /// Stores a membership before anything else happens. A failure leaves the membership
    /// as it was.
    fn remember(&self, membership: Membership) -> Result<(), FlowError> {
        let previous = {
            let mut state = lock(&self.state);
            let previous = state.entry.membership.clone();
            if previous == membership {
                return Ok(());
            }
            state.entry.membership = membership;
            (previous, std::mem::replace(&mut state.unsaved, true))
        };
        self.save().inspect_err(|_| {
            let mut state = lock(&self.state);
            state.entry.membership = previous.0.clone();
            state.unsaved = previous.1;
        })
    }

    /// Writes the membership and the profile state to the book when they wait to be
    /// written.
    fn save(&self) -> Result<(), FlowError> {
        let _saving = lock(&self.saving);
        let (membership, profile_sync) = {
            let state = lock(&self.state);
            if !state.unsaved || state.retired {
                return Ok(());
            }
            (state.entry.membership.clone(), state.entry.profile_sync)
        };
        let written = membership.clone();
        #[cfg(test)]
        {
            let pause = lock(&self.core.before_write).clone();
            if let Some(pause) = pause {
                pause();
            }
        }
        self.core.book.update(self.key, |entry| {
            entry.membership = membership;
            entry.profile_sync = profile_sync;
        })?;
        let mut state = lock(&self.state);
        if state.entry.membership == written && state.entry.profile_sync == profile_sync {
            state.unsaved = false;
        }
        Ok(())
    }

    async fn log_out(&self) -> Result<(), FlowError> {
        let (api, held, account_id) = {
            let mut state = lock(&self.state);
            let AccessState::LoggedIn { account_id, .. } = state.access else {
                return Err(FlowError::NotAvailable);
            };
            state.access = AccessState::LoggedOut { account_id };
            (state.api.clone(), state.session.take(), account_id)
        };
        tracing::debug!(server = %self.id, account = %account_id, "logging out");
        match (api, held) {
            (Some(api), Some(held)) => match api.log_out(&held.token).await {
                Ok(()) => Ok(()),
                Err(error) if error.category() == Some(ErrorCategory::SessionEnded) => Ok(()),
                Err(error) => Err(self.failed(error)),
            },
            _ => Ok(()),
        }
    }

    /// Deletes the key and the membership in one step under the lock of the book.
    fn forget_account(&self) -> Result<(), FlowError> {
        let _saving = lock(&self.saving);
        let keys = self.core.keys.clone();
        self.core.book.transact(self.key, |entry| {
            entry.membership = Membership::None;
            entry.profile_sync = ProfileSync::NotNeeded;
            keys.remove(entry.server_id).map_err(FlowError::from)
        })?;
        let mut state = lock(&self.state);
        state.access = AccessState::NoAccount { notice: None };
        state.session = None;
        state.entry.membership = Membership::None;
        state.entry.profile_sync = ProfileSync::NotNeeded;
        state.unsaved = false;
        Ok(())
    }

    fn session(&self) -> Result<(ApiClient, Held), ApiError> {
        let state = lock(&self.state);
        match (&state.api, &state.session, state.retired) {
            (Some(api), Some(held), false) => Ok((api.clone(), held.clone())),
            _ => Err(ApiError::NotLoggedIn),
        }
    }

    /// Sends one request of the account that the device has now. The request belongs to
    /// that account: after a renewal into another account it is not sent again.
    async fn authorized(
        self: Arc<Self>,
        outline: Outline,
        only_for: Option<AccountId>,
    ) -> Result<HttpResponse, ApiError> {
        let (api, held) = self.session()?;
        let started_for = only_for.unwrap_or(held.account_id);
        if held.account_id != started_for {
            return Err(ApiError::AccountChanged {
                was: started_for,
                now: held.account_id,
            });
        }
        let repeat = Outline {
            method: outline.method,
            path: outline.path.clone(),
            query: outline.query.clone(),
            body: outline.body.clone(),
        };
        let result = match send_with_token(&api, &held.token, outline).await {
            Err(error) if error.category() == Some(ErrorCategory::SessionEnded) => {
                let (api, renewed) = self.renew(&held.token).await?;
                if renewed.account_id != started_for {
                    return Err(ApiError::AccountChanged {
                        was: started_for,
                        now: renewed.account_id,
                    });
                }
                send_with_token(&api, &renewed.token, repeat).await
            }
            other => other,
        };
        match &result {
            Ok(_) => self.answered(),
            Err(error) => self.note(error),
        }
        result
    }

    /// The server did not accept the session any more: it expired, the device was
    /// removed, or the account was banned, removed or recovered. The device logs in again
    /// with its key, and the answer decides the state.
    async fn renew(&self, ended: &SecretToken) -> Result<(ApiClient, Held), ApiError> {
        let _gate = self.gate.lock().await;
        let (api, current) = self.session()?;
        if current.token != *ended {
            return Ok((api, current));
        }
        let before = {
            let mut state = lock(&self.state);
            state.session = None;
            state.in_progress = Some(ActionKind::LogIn);
            state.error = None;
            state.access.clone()
        };
        self.core.notify();
        let running = Running(self);
        let result = self.log_in_again(&before, api, current.account_id).await;
        running.finish(result.as_ref().err().map(FlowError::from));
        result
    }

    async fn log_in_again(
        &self,
        before: &AccessState,
        api: ApiClient,
        account_id: AccountId,
    ) -> Result<(ApiClient, Held), ApiError> {
        let logged_out = || {
            lock(&self.state).access = AccessState::LoggedOut { account_id };
        };
        let key = match self.stored_key() {
            Ok(Some(key)) => key,
            Ok(None) => {
                let _ = self.settle(
                    Settled {
                        state: AccessState::NoAccount {
                            notice: Some(Notice::KeyMissing),
                        },
                        membership: Membership::None,
                    },
                    None,
                );
                return Err(ApiError::AccessLost(AccessLoss::Removed));
            }
            Err(error) => {
                logged_out();
                return Err(error.into());
            }
        };
        let probe = match self.probe(&api, &key).await {
            Ok(probe) => probe,
            Err(error) => {
                logged_out();
                self.note(&error);
                return Err(error);
            }
        };
        let outcome = match held_of(&probe) {
            Some(held) => Ok((api, held)),
            None if probe == Probe::Banned => Err(ApiError::AccessLost(AccessLoss::Banned)),
            None => Err(ApiError::AccessLost(AccessLoss::Removed)),
        };
        if let Err(error) = self.settle_probe(before, probe, false) {
            tracing::warn!(server = %self.id, %error, "the server book was not updated");
        }
        outcome
    }

    /// Sends the profile preset to the account when it waits (ADR 0105).
    async fn apply_profile(self: &Arc<Self>) -> Result<ProfileSync, ApiError> {
        let (sync, account) = {
            let state = lock(&self.state);
            let account = state.session.as_ref().map(|held| held.account_id);
            (state.entry.profile_sync, account)
        };
        let ProfileSync::Pending { attempts } = sync else {
            return Ok(sync);
        };
        let account = account.ok_or(ApiError::NotLoggedIn)?;
        let preset = self.core.profile.load()?;
        let Some(avatar) = self.core.profile.avatar()? else {
            return Ok(self.record_sync(ProfileSync::NotNeeded));
        };
        let api = ServerApi::new(Arc::new(Managed {
            server: self.clone(),
            account: Some(account),
        }));
        let update = ProfileUpdate {
            name: preset.name,
            avatar: Some(avatar),
        };
        match self.core.uploader.upload(api, update).await {
            Ok(()) => Ok(self.record_sync(ProfileSync::Applied)),
            Err(error @ ApiError::NotOffered { .. }) => Err(error),
            Err(error) => {
                self.record_sync(ProfileSync::Pending {
                    attempts: attempts.saturating_add(1),
                });
                Err(error)
            }
        }
    }

    fn record_sync(&self, sync: ProfileSync) -> ProfileSync {
        {
            let mut state = lock(&self.state);
            state.entry.profile_sync = sync;
            state.unsaved = true;
        }
        if let Err(error) = self.save() {
            tracing::warn!(server = %self.id, %error, "the server book was not updated");
        }
        self.core.notify();
        sync
    }
}

#[cfg(test)]
mod tests;
