//! The client: the saved servers, and for each of them its own state, session and access
//! flow (ADR 0101, ADR 0107). A server that fails or cannot be reached changes its own
//! state only.

use std::future::Future;
use std::io;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use junto_protocol::account::ModerationNoticePage;
use junto_protocol::error::ErrorCode;
use junto_protocol::events::EndReason;
use junto_protocol::ids::{AccountId, DeviceId, RecoveryId, ServerId};
use junto_protocol::invite::JoinRequestState;
use junto_protocol::keys::PublicKey;
use time::OffsetDateTime;
use tokio::sync::watch;

use crate::address::ServerAddress;
use crate::api::session::{
    Authority, AuthorityFuture, OpenAuthorityFuture, Outline, open_with_token, send_with_token,
};
use crate::api::{
    AccessLoss, ApiClient, ApiError, AvatarImage, ErrorCategory, ExpectedAccount, ReqwestTransport,
    RetryPolicy, ServerApi, Session, Transport, TransportError, TransportErrorKind, logo_at,
    server_info,
};
use crate::book::{
    BookError, ConnectionMode, EntryKey, Membership, Owner, ProfileSync, ServerBook, ServerEntry,
};
use crate::events::{ConnectionLevel, EventStreams, StreamTarget};
use crate::flow::access::{
    self, AccessAction, AccessState, ActionKind, Notice, Probe, Settled, after_probe,
    after_request_status, request_membership,
};
use crate::flow::add_server::AddServerFlow;
use crate::flow::approve_link::ApproveLinkFlow;
use crate::flow::link_all::{
    ApproveDevicesFlow, LinkDeviceFlow, LinkOffer, SERVERS_AT_ONCE, link_code_is_plain,
};
use crate::flow::machine::lock;
use crate::flow::{FlowError, FlowView};
use crate::keys::{self, DeviceKey, FileKeyStore, KeyStore, KeyStoreError};
use crate::live::{LiveServer, Shared};
use crate::paths::DataDir;
use crate::profile::ProfileStore;
use crate::runtime::Runtime;
use crate::secret::SecretToken;
use crate::settings::SettingsStore;
use crate::storage::StorageError;
use crate::stream::{
    Desired, Environment, Host, ReqwestStreamTransport, StreamPolicy, StreamTransport, Streams,
};
use crate::updates::{UpdateCheck, UpdateSource, Version};
use crate::version::{ApiVersion, Incompatibility, SupportedVersions};
use crate::voice::VoiceStore;

/// How a client is opened. Every part except the data directory has a default.
pub struct ClientOptions {
    pub data_dir: PathBuf,
    pub transport: Option<Arc<dyn Transport>>,
    pub key_store: Option<Arc<dyn KeyStore>>,
    pub versions: SupportedVersions,
    pub retry: RetryPolicy,
    pub runtime: Option<Runtime>,
    /// What carries the connections of the event stream. The default is made when the
    /// streams start.
    pub stream_transport: Option<Arc<dyn StreamTransport>>,
    pub stream: StreamPolicy,
    /// Where the client learns of a newer build (ADR 0704). Without a source the client
    /// never asks.
    pub update_source: Option<Arc<dyn UpdateSource>>,
    /// The version of the build that runs, which a release is compared with.
    pub build: Version,
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
            stream_transport: None,
            stream: StreamPolicy::default(),
            update_source: None,
            build: Version::THIS_BUILD,
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
    updates: UpdateCheck,
    stream_transport: Option<Arc<dyn StreamTransport>>,
    stream_policy: StreamPolicy,
    /// Runs between the snapshot and the write of a save. Only tests set it, to fix the
    /// order of two saves.
    #[cfg(test)]
    before_write: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    #[cfg(test)]
    after_remove: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    selected: Mutex<Option<EntryKey>>,
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
    /// In the order of the server book: the order of the file at the start, a new server
    /// at the end as in the book, and the order that the book returns after a move.
    servers: Mutex<Vec<Arc<Server>>>,
    /// Held from the move of an entry in the book to the new order of `servers`, so that
    /// two moves give `servers` the order of the later one.
    moving: Mutex<()>,
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
        let changes = watch::Sender::new(0);
        let told = changes.clone();
        let updates = UpdateCheck::with_notice(
            options.build,
            options.update_source,
            runtime.clone(),
            move || told.send_modify(|generation| *generation += 1),
        );
        let core = Arc::new(Core {
            profile,
            data_dir,
            book,
            keys,
            transport,
            versions: options.versions,
            policy: options.retry,
            runtime,
            updates,
            stream_transport: options.stream_transport,
            stream_policy: options.stream,
            #[cfg(test)]
            before_write: Mutex::new(None),
            #[cfg(test)]
            after_remove: Mutex::new(None),
            selected: Mutex::new(None),
            streams: Mutex::new(None),
            changes,
        });
        let servers = entries
            .into_iter()
            .map(|entry| Server::new(core.clone(), entry))
            .collect();
        Ok(Self {
            inner: Arc::new(Inner {
                core,
                servers: Mutex::new(servers),
                moving: Mutex::new(()),
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

    /// The settings of the interface of this installation (ADR 0710).
    pub fn settings(&self) -> SettingsStore {
        SettingsStore::new(self.inner.core.data_dir.clone())
    }

    /// The sound settings of this installation (ADR 0710).
    pub fn voice(&self) -> VoiceStore {
        VoiceStore::new(self.inner.core.data_dir.clone())
    }

    /// Whether a newer build of the client exists (ADR 0704). Its periodic check starts
    /// with the event streams. A change of its state is a change of the client.
    pub fn updates(&self) -> &UpdateCheck {
        &self.inner.core.updates
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

    /// The runtime that carries the network work of this client.
    pub fn runtime(&self) -> &Runtime {
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
        let key = if may_wait(&lock(&server.state).access) {
            KeyAfterRemoval::Kept
        } else {
            KeyAfterRemoval::Removed
        };
        self.remove_locked(&server, key)
    }

    /// Removes this saved entry and its key after any running access action or session
    /// renewal. The entry is selected when this method is called, so a later replacement
    /// is never removed. Poll this future on a background executor: removal writes files.
    ///
    /// A join request of this device that waits is withdrawn on the server first. When the
    /// server does not confirm the withdrawal, the entry is removed and the key stays, so
    /// that an approval cannot make an account for a key that this device no longer holds;
    /// the server added again finds its request with that key (ADR 0111).
    pub fn remove_server_when_free(
        &self,
        server_id: ServerId,
    ) -> impl Future<Output = Result<(), RemoveError>> + Send + 'static + use<> {
        let client = self.clone();
        let server = lock(&self.inner.servers)
            .iter()
            .find(|server| server.id == server_id)
            .cloned()
            .ok_or(BookError::NotSaved { server_id });
        async move {
            let server = server?;
            let runtime = client.inner.core.runtime.clone();
            let work = runtime.run(async move {
                let _gate = server.gate.lock().await;
                let key = server.withdraw_before_removal().await?;
                client.remove_locked(&server, key)
            });
            work.await.map_err(|_| RemoveError::Stopped)?
        }
    }

    fn remove_locked(&self, server: &Arc<Server>, key: KeyAfterRemoval) -> Result<(), RemoveError> {
        let server_id = server.id;
        if lock(&server.state).retired {
            return Err(BookError::NotSaved { server_id }.into());
        }
        let keys = self.inner.core.keys.clone();
        let removed = match key {
            KeyAfterRemoval::Removed => self.inner.core.book.remove(server.key, || {
                keys.remove(server_id).map_err(RemoveError::from)
            }),
            // The book records the kept key; it stays until the server confirms that no
            // request and no account has it (ADR 0712).
            KeyAfterRemoval::Kept => self
                .inner
                .core
                .book
                .remove_keeping_key(server.key, OffsetDateTime::now_utc().unix_timestamp())
                .map_err(RemoveError::from),
        };
        if matches!(&removed, Err(RemoveError::Book(BookError::Storage(_)))) {
            return removed.map(|_| ());
        }
        lock(&self.inner.servers).retain(|kept| !Arc::ptr_eq(kept, server));
        server.retire();
        #[cfg(test)]
        if let Some(after_remove) = lock(&self.inner.core.after_remove).clone() {
            after_remove();
        }
        {
            let mut selected = lock(&self.inner.core.selected);
            if *selected == Some(server.key) {
                *selected = None;
            }
        }
        self.inner.core.notify();
        removed.map(|_| ())
    }

    /// Moves a server to `position` of the list that [`Client::servers`] gives, in the
    /// server book and in that list. A position past the end is the end. The book can hold
    /// servers that another program added and that this client does not show: the server
    /// is put directly before the server that follows it in the list of this client, and
    /// at the end of the book when none follows. A server that the book does not hold any
    /// more is "not saved" and nothing moves.
    pub fn move_server(&self, server_id: ServerId, position: usize) -> Result<(), BookError> {
        let _moving = lock(&self.inner.moving);
        let (key, following) = {
            let servers = lock(&self.inner.servers);
            let moved = servers
                .iter()
                .find(|server| server.id == server_id)
                .ok_or(BookError::NotSaved { server_id })?;
            let following: Vec<EntryKey> = servers
                .iter()
                .filter(|server| server.id != server_id)
                .skip(position)
                .map(|server| server.key)
                .collect();
            (moved.key, following)
        };
        let order = self.inner.core.book.reorder(key, |others| {
            following
                .iter()
                .find_map(|next| others.iter().position(|entry| entry.key() == *next))
                .unwrap_or(others.len())
        })?;
        // A server that the book does not hold any more keeps its place after the others.
        let place = |server: &Arc<Server>| {
            order
                .iter()
                .position(|key| *key == server.key)
                .unwrap_or(usize::MAX)
        };
        lock(&self.inner.servers).sort_by_key(place);
        self.inner.core.notify();
        Ok(())
    }

    /// The server that the interface shows. It gets every event; the others get the
    /// background events (ADR 0107).
    pub fn select(&self, server_id: Option<ServerId>) {
        {
            let servers = lock(&self.inner.servers);
            let entry = server_id.and_then(|id| {
                servers
                    .iter()
                    .find(|server| server.id == id)
                    .map(|server| server.key)
            });
            *lock(&self.inner.core.selected) = entry;
        }
        for handle in self.servers() {
            handle.server.announce_level();
        }
        self.inner.core.notify();
    }

    pub fn selected(&self) -> Option<ServerId> {
        lock(&self.inner.core.selected).map(|entry| entry.server_id)
    }

    /// Starts the event streams: from now on every saved server has the connection that
    /// its level asks for (ADR 0107), and [`ServerHandle::live`] gives its state. The
    /// periodic update check starts too (ADR 0704). A second call changes nothing.
    pub fn start_event_streams(&self) -> Result<(), TransportError> {
        let core = &self.inner.core;
        core.updates.start();
        {
            let mut streams = lock(&core.streams);
            if streams.is_some() {
                return Ok(());
            }
            let sockets = match &core.stream_transport {
                Some(sockets) => sockets.clone(),
                None => Arc::new(ReqwestStreamTransport::new()?),
            };
            let environment = Environment {
                transport: core.transport.clone(),
                sockets,
                retry: core.policy,
                policy: core.stream_policy,
            };
            *streams = Some(Arc::new(Streams::new(environment, core.runtime.clone())));
        }
        self.announce_levels();
        Ok(())
    }

    /// Attaches the event streams and tells them the present level of every server.
    #[cfg(test)]
    pub(crate) fn attach_event_streams(&self, streams: Arc<dyn EventStreams>) {
        *lock(&self.inner.core.streams) = Some(streams);
        self.announce_levels();
    }

    fn announce_levels(&self) {
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
    #[error("the client has stopped")]
    Stopped,
    /// The server answered that the key of this device has an account: the join request
    /// was approved while the server was removed, or the account of a stale inactive state
    /// is active or banned now. The device checked its state, and the server and its key
    /// stay.
    #[error("the server answered that the key has an account; the server stays")]
    HasAccount,
}

/// True when a join request of the key of this device can wait on the server: the state
/// waits for an approval, or the account is inactive and can have asked to return; a
/// client that restarts without a connection shows an inactive account until a check
/// finds its request (ADR 0712).
fn may_wait(state: &AccessState) -> bool {
    matches!(
        state,
        AccessState::WaitingForApproval { .. } | AccessState::Inactive { .. }
    )
}

/// Whether the removal of a server from the book also removes the key of this device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KeyAfterRemoval {
    Removed,
    /// A join request of the key can still wait on the server.
    Kept,
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
    device_id: DeviceId,
}

struct ServerState {
    entry: ServerEntry,
    reach: Reach,
    access: AccessState,
    in_progress: Option<ActionKind>,
    error: Option<FlowError>,
    api: Option<ApiClient>,
    session: Option<Held>,
    /// Counts the changes of the account that the state of the profile preset belongs
    /// to. A try to send the avatar records its result only for the count that it read.
    preset_owner: u64,
    /// The recoveries that this device acknowledged since the client started, each with
    /// the account and the device that acknowledged it. The server never tells them
    /// again to that device, except in the answer of a login that it computed before the
    /// acknowledgement. A recovery that the server tells another account or another
    /// device of the entry is that one's to acknowledge.
    acknowledged: Vec<(AccountId, DeviceId, RecoveryId)>,
    announced: Option<(ConnectionLevel, Option<SecretToken>)>,
    /// The account is in a call with this device, as the event stream tells it.
    in_call: bool,
    /// After a login that got no answer (ADR 0787): no login after an ended session is
    /// sent before this time, and how many such logins followed each other.
    login_wait: Option<(tokio::time::Instant, u32)>,
    /// The membership or the profile state in memory is not in the book yet: a write
    /// failed. It is cleared only by a write that succeeded.
    unsaved: bool,
    retired: bool,
}

pub(crate) struct Server {
    /// This object, for the tasks that must not keep it alive.
    this: Weak<Server>,
    /// What the event stream client of the server and its readers share.
    live: Arc<Shared>,
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
    /// Held while the avatar of the preset is sent, so that one try ends before the next
    /// reads the state.
    applying: tokio::sync::Mutex<()>,
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
    /// future is dropped. After an action that succeeded, the avatar of the profile preset
    /// is sent to an account that waits for it (ADR 0717).
    pub fn dispatch(
        &self,
        action: AccessAction,
    ) -> impl Future<Output = Result<(), FlowError>> + Send + 'static + use<> {
        self.run_wanted(Wanted::Action(action))
    }

    fn run_wanted(
        &self,
        wanted: Wanted,
    ) -> impl Future<Output = Result<(), FlowError>> + Send + 'static + use<> {
        self.run_in_turn(wanted, Turn::Now)
    }

    fn run_in_turn(
        &self,
        wanted: Wanted,
        turn: Turn,
    ) -> impl Future<Output = Result<(), FlowError>> + Send + 'static + use<> {
        let server = self.server.clone();
        let work = self.server.core.runtime.run(async move {
            let gate = match turn {
                Turn::Now => server.gate.try_lock().map_err(|_| FlowError::Busy),
                Turn::WhenFree => Ok(server.gate.lock().await),
            };
            let result = match gate {
                Ok(gate) => server.clone().dispatch(gate, wanted).await,
                Err(busy) => Err(busy),
            };
            if result.is_ok()
                && let Ok(applying) = server.applying.try_lock()
            {
                // A try that runs already is not waited for.
                let _ = server.apply_profile(applying).await;
            }
            result
        });
        async move { work.await? }
    }

    /// Runs one action of the access flow after an action or a renewal of the session that
    /// runs now has ended. `dispatch` answers `busy` in that case (ADR 0712); an interface
    /// uses this for what a person asked for, so that the request is not lost because the
    /// client asked the server something by itself at the same moment. Actions that wait
    /// run in the order in which they were asked for, and an action that waits runs before
    /// one that `dispatch` asks for after the running one has ended.
    pub fn dispatch_when_free(
        &self,
        action: AccessAction,
    ) -> impl Future<Output = Result<(), FlowError>> + Send + 'static + use<> {
        self.run_wanted_when_free(Wanted::Action(action))
    }

    fn run_wanted_when_free(
        &self,
        wanted: Wanted,
    ) -> impl Future<Output = Result<(), FlowError>> + Send + 'static + use<> {
        self.run_in_turn(wanted, Turn::WhenFree)
    }

    /// Contacts the server as [`ServerHandle::connect`] does, after an action or a renewal
    /// of the session that runs now has ended.
    pub fn connect_when_free(
        &self,
    ) -> impl Future<Output = Result<(), FlowError>> + Send + 'static + use<> {
        self.run_wanted_when_free(Wanted::Contact)
    }

    /// Logs out, after an action or a renewal of the session that runs now has ended.
    /// `dispatch` refuses a second action while one runs (ADR 0712); a program that ends
    /// uses this, so that a session that the running action creates is ended too. Without
    /// a session nothing is sent.
    pub fn end_session(
        &self,
    ) -> impl Future<Output = Result<(), FlowError>> + Send + 'static + use<> {
        let ending = self.dispatch_when_free(AccessAction::LogOut);
        async move {
            match ending.await {
                Err(FlowError::NotAvailable) => Ok(()),
                result => result,
            }
        }
    }

    /// Contacts the server and brings the flow to its present state. A device that
    /// belongs to an account logs in; no action of the user is needed (ADR 0109). After a
    /// logout of the person nothing is sent: only the action `log in` ends that logout.
    pub fn connect(&self) -> impl Future<Output = Result<(), FlowError>> + Send + 'static + use<> {
        self.run_wanted(Wanted::Contact)
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
                    | Err(
                        FlowError::Unavailable
                        | FlowError::Unreachable(_)
                        | FlowError::Busy
                        | FlowError::UnknownCode {
                            category: ErrorCategory::ServerFault,
                            ..
                        },
                    ) => {}
                    Err(
                        FlowError::Limited { retry_after, .. }
                        | FlowError::UnknownCode {
                            category: ErrorCategory::Limited,
                            retry_after,
                            ..
                        },
                    ) => {
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

    /// States that the interface has shown a recovery to the person (ADR 0118). The server
    /// removes the notice of this device, and the recovery leaves the state `logged in`.
    /// Until then every login tells the recovery again. The acknowledgement is sent for the
    /// account that the flow shows. When the server holds no such notice of this device,
    /// the recovery leaves the state too and the call fails with `not_found`.
    pub fn acknowledge_recovery(
        &self,
        recovery_id: RecoveryId,
    ) -> impl Future<Output = Result<(), ApiError>> + Send + 'static + use<> {
        let server = self.server.clone();
        let work = self
            .server
            .core
            .runtime
            .run(async move { server.acknowledge_recovery(recovery_id).await });
        async move { work.await.unwrap_or(Err(ApiError::Stopped)) }
    }

    /// Reads the logo of the server from its public route, without the session
    /// (ADR 0101). Absent: the server has no logo, or the client knows no address of one.
    pub fn logo(
        &self,
    ) -> impl Future<Output = Result<Option<AvatarImage>, ApiError>> + Send + 'static + use<> {
        let server = self.server.clone();
        let work = self
            .server
            .core
            .runtime
            .run(async move { server.logo().await });
        async move { work.await.unwrap_or(Err(ApiError::Stopped)) }
    }

    /// The private moderation notices that the server addresses to the key of this device
    /// (ADR 0120): the device proves its key without a session, so the notices of a removed
    /// or banned account are read too. A device without a stored key gets none.
    pub fn moderation_notices(
        &self,
    ) -> impl Future<Output = Result<ModerationNoticePage, FlowError>> + Send + 'static + use<>
    {
        let server = self.server.clone();
        let work = self
            .server
            .core
            .runtime
            .run(async move { server.moderation_notices().await });
        async move {
            work.await
                .unwrap_or_else(|_| Err(FlowError::from(ApiError::Stopped)))
        }
    }

    /// Sets the state of the access flow, without a server.
    #[cfg(feature = "test-support")]
    pub fn set_access_for_test(&self, access: AccessState) {
        lock(&self.server.state).access = access;
    }

    /// The account of the session that this device holds on the server now.
    pub fn account(&self) -> Option<AccountId> {
        lock(&self.server.state)
            .session
            .as_ref()
            .map(|held| held.account_id)
    }

    /// The entry of the server book and the account that the access flow shows as logged
    /// in. Absent in every other state of the flow, and for an entry that was removed from
    /// the book. The owner stays while an ended session of the same account is renewed.
    pub fn owner(&self) -> Option<Owner> {
        let state = lock(&self.server.state);
        match state.access {
            AccessState::LoggedIn { account_id, .. } if !state.retired => Some(Owner {
                entry: self.server.key,
                account_id,
            }),
            _ => None,
        }
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
        self.server.api_for(account_id)
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

    /// The state of the server as the event stream keeps it, and its changes
    /// (ADR 0719). Without a call of [`Client::start_event_streams`] the state stays
    /// absent.
    pub fn live(&self) -> LiveServer {
        LiveServer {
            shared: self.server.live.clone(),
        }
    }

    /// The files of the messages of this server: uploads, downloads, and the sending of
    /// a message with files (ADR 0721).
    pub fn transfers(&self) -> crate::transfers::Transfers {
        let handle = self.clone();
        crate::transfers::Transfers {
            live: self.live(),
            runtime: self.server.core.runtime.clone(),
            owned: Arc::new(move || {
                let owner = handle.owner()?;
                Some((owner, handle.api_for(owner.account_id)))
            }),
        }
    }

    /// The level that the event stream of this server should have now.
    pub fn connection_level(&self) -> ConnectionLevel {
        self.server.level().0
    }

    /// The private moderation notices that the person put away on this device.
    pub fn moderation_notices_shown(&self) -> Vec<uuid::Uuid> {
        lock(&self.server.state)
            .entry
            .moderation_notices_shown
            .clone()
    }

    /// Records in the server book that the person put a moderation notice away. The book
    /// keeps the newest [`crate::book::MOST_SHOWN_NOTICES`].
    pub fn put_moderation_notice_away(&self, notice_id: uuid::Uuid) -> Result<(), BookError> {
        let entry = self.server.core.book.update(self.server.key, |entry| {
            let shown = &mut entry.moderation_notices_shown;
            if !shown.contains(&notice_id) {
                shown.push(notice_id);
            }
            let over = shown.len().saturating_sub(crate::book::MOST_SHOWN_NOTICES);
            shown.drain(..over);
        })?;
        lock(&self.server.state).entry.moderation_notices_shown = entry.moderation_notices_shown;
        self.server.core.notify();
        Ok(())
    }

    /// Whether the avatar of the profile preset still waits to be sent to the account on
    /// this server.
    pub fn profile_sync(&self) -> ProfileSync {
        lock(&self.server.state).entry.profile_sync
    }

    /// Sends the avatar of the profile preset to the account when it waits, and records the
    /// result: a success ends the waiting, a refusal of the image ends it too, and another
    /// failure is counted (ADR 0717).
    pub fn apply_profile(
        &self,
    ) -> impl Future<Output = Result<ProfileSync, ApiError>> + Send + 'static + use<> {
        let server = self.server.clone();
        let work = self.server.core.runtime.run(async move {
            let applying = server.applying.lock().await;
            server.apply_profile(applying).await
        });
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
        let work = self.server.core.runtime.run(async move {
            let exchange = |api: ApiClient, token: SecretToken, outline: Outline| async move {
                send_with_token(&api, &token, outline).await
            };
            server.authorized(outline, account, exchange).await
        });
        Box::pin(async move { work.await.unwrap_or(Err(ApiError::Stopped)) })
    }

    fn open(&self, outline: Outline, from: u64) -> OpenAuthorityFuture {
        let (server, account) = (self.server.clone(), self.account);
        let work = self.server.core.runtime.run(async move {
            let exchange = move |api: ApiClient, token: SecretToken, outline: Outline| async move {
                open_with_token(&api, &token, outline, from).await
            };
            server.authorized(outline, account, exchange).await
        });
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
            device_id: session.device_id,
        }),
        Probe::UnknownDevice | Probe::Banned | Probe::Inactive | Probe::Deleted => None,
    }
}

/// The wait after the first login without an answer. It doubles with each further one.
const LOGIN_WAIT_FIRST: Duration = Duration::from_secs(1);
const LOGIN_WAIT_LONGEST: Duration = Duration::from_secs(60);

/// True when a login failed without an answer of the server about the account: the server
/// was not reached, it failed, or it named a time to wait.
fn unanswered(error: &ApiError) -> bool {
    error.is_unavailable() || error.category() == Some(ErrorCategory::Limited)
}

/// True when the server did not complete the operation for a reason that a login can
/// explain: the answer was lost, or the key already belongs to an account.
fn login_explains(error: &ApiError) -> bool {
    matches!(error, ApiError::OutcomeUnknown)
        || error.code() == Some(ErrorCode::KeyAlreadyRegistered)
}

/// What an operation of the access flow creates when it succeeds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Creates {
    /// No account: a login, a check, a request for a link.
    Nothing,
    /// An account for this device: a join with a code, an approved join request.
    Account,
    /// A claim: the owner account, or a new device of the owner account that exists. The
    /// second is a recovery and leaves a notice for the new device (ADR 0114).
    Owner,
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

/// What a caller of the flow asks for: one action, or the contact that the client makes by
/// itself. The action of a contact is chosen from the state after the action lock is taken,
/// so a logout of the person that ends while the contact waits is not undone by a login
/// chosen before it.
#[derive(Debug, Clone)]
enum Wanted {
    Action(AccessAction),
    Contact,
}

/// When an action takes the lock of the actions of its server.
#[derive(Debug, Clone, Copy)]
enum Turn {
    /// At once, or not at all: a running action makes it `busy`.
    Now,
    /// In the order of the waiting actions, after the running one. The lock is held from
    /// its turn to the end of the action, so no action asked for later runs first.
    WhenFree,
}

/// The action that contacts the server in a state: a login for a device that is logged out,
/// none after a logout of the person, and a check otherwise.
fn contact_action(state: &AccessState) -> Option<AccessAction> {
    match state {
        AccessState::LoggedOut { .. } => Some(AccessAction::LogIn),
        AccessState::LoggedOutByPerson { .. } => None,
        _ => Some(AccessAction::Check),
    }
}

impl Server {
    fn new(core: Arc<Core>, entry: ServerEntry) -> Arc<Self> {
        Arc::new_cyclic(|this| Self {
            this: this.clone(),
            live: Shared::new(),
            id: entry.server_id,
            key: entry.key(),
            address: entry.address.clone(),
            core,
            state: Mutex::new(ServerState {
                access: access::initial(&entry.membership, entry.logged_out),
                entry,
                reach: Reach::Unknown,
                in_progress: None,
                error: None,
                api: None,
                session: None,
                preset_owner: 0,
                acknowledged: Vec::new(),
                announced: None,
                in_call: false,
                login_wait: None,
                unsaved: false,
                retired: false,
            }),
            gate: tokio::sync::Mutex::new(()),
            saving: Mutex::new(()),
            applying: tokio::sync::Mutex::new(()),
        })
    }

    fn retire(&self) {
        {
            let mut state = lock(&self.state);
            state.retired = true;
            state.session = None;
            state.api = None;
            state.announced = Some((ConnectionLevel::Off, None));
        }
        // The streams are told in every case, so that the task of this server ends.
        let streams = lock(&self.core.streams).clone();
        if let Some(streams) = streams {
            streams.set_level(self, ConnectionLevel::Off, None);
        }
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
        // A call goes on while another server is shown (ADR 0107), so the server of a
        // call keeps a connection in every connection mode.
        let level = if selected == Some(self.key) {
            ConnectionLevel::Selected
        } else if state.entry.connection_mode == ConnectionMode::Always || state.in_call {
            ConnectionLevel::Background
        } else {
            return (ConnectionLevel::Off, None);
        };
        let target = StreamTarget {
            server_id: self.id,
            address: state.entry.address.clone(),
            api_version: api.version(),
            token: held.token.clone(),
            account_id: held.account_id,
            device_id: held.device_id,
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
            // A login after an ended session runs, and the device has no session until it
            // ends. The streams keep what they have: the end of the login announces the
            // level (ADR 0787).
            if state.session.is_none()
                && state.in_progress == Some(ActionKind::LogIn)
                && matches!(state.access, AccessState::LoggedIn { .. })
            {
                return;
            }
            if state.announced.as_ref() == Some(&now) {
                return;
            }
            if state.announced.is_none() && level == ConnectionLevel::Off {
                state.announced = Some(now);
                return;
            }
            state.announced = Some(now);
        }
        streams.set_level(self, level, target);
    }

    #[cfg(test)]
    pub(crate) fn id(&self) -> ServerId {
        self.id
    }

    /// Runs one action while `_gate`, the lock of the actions of this server, is held.
    async fn dispatch(
        self: Arc<Self>,
        _gate: tokio::sync::MutexGuard<'_, ()>,
        wanted: Wanted,
    ) -> Result<(), FlowError> {
        let action = match wanted {
            Wanted::Action(action) => action,
            Wanted::Contact => match contact_action(&lock(&self.state).access) {
                Some(action) => action,
                None => return Ok(()),
            },
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
                if let AccessState::LoggedOutByPerson { account_id } = *before {
                    self.end_logout_of_person(account_id)?;
                }
                let api = self.contact(false).await?;
                self.reconcile(&api, before).await
            }
            AccessAction::LogOut => self.log_out().await,
            AccessAction::LogOutByPerson => self.log_out_by_person().await,
            AccessAction::ForgetAccount => {
                if may_wait(before) {
                    self.withdraw_request(before).await?;
                }
                self.forget_account()
            }
            AccessAction::Join {
                code,
                name,
                device_name,
            } => {
                let api = self.contact(false).await?;
                let key = self.key_or_new()?;
                match api.join(&key, code, name, device_name).await {
                    Ok(session) => {
                        self.settle_probe(before, Probe::LoggedIn(session), Creates::Account)
                    }
                    Err(error) => {
                        self.explain_by_login(&api, &key, before, error, Creates::Account)
                            .await
                    }
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
                        self.settle_probe(before, Probe::LoggedIn(session), Creates::Owner)
                    }
                    Err(error) => {
                        self.explain_by_login(&api, &key, before, error, Creates::Owner)
                            .await
                    }
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
                    Err(error) => {
                        self.explain_by_login(&api, &key, before, error, Creates::Nothing)
                            .await
                    }
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
        self.remember(request_membership(&previous, None, &name))?;
        let waiting = |join_request_id| Settled {
            state: AccessState::WaitingForApproval {
                join_request_id,
                name: name.clone(),
            },
            membership: request_membership(&previous, join_request_id, &name),
        };
        match api.request_to_join(&key, name.clone()).await {
            // A request that already waited keeps the name that the server holds.
            Ok(request) => self.settle(
                Settled {
                    state: AccessState::WaitingForApproval {
                        join_request_id: Some(request.id),
                        name: request.name.clone(),
                    },
                    membership: request_membership(&previous, Some(request.id), &request.name),
                },
                None,
            ),
            Err(error)
                if matches!(error, ApiError::OutcomeUnknown)
                    || error.code() == Some(ErrorCode::JoinRequestPending) =>
            {
                match api.join_request_status(&key).await {
                    Ok(status) => match after_request_status(&status, &name, &previous) {
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
                self.explain_by_login(&api, &key, before, error, Creates::Nothing)
                    .await
            }
        }
    }

    /// Withdraws a join request of this device that can wait, before the server is removed
    /// from the book. The key stays unless the server confirmed that no request of it waits.
    /// When the request was approved meanwhile, the device logs in to the new account and
    /// the removal stops.
    async fn withdraw_before_removal(&self) -> Result<KeyAfterRemoval, RemoveError> {
        let before = lock(&self.state).access.clone();
        if !may_wait(&before) {
            return Ok(KeyAfterRemoval::Removed);
        }
        let key = match self.stored_key() {
            Ok(Some(key)) => key,
            Ok(None) => return Ok(KeyAfterRemoval::Removed),
            Err(_) => return Ok(KeyAfterRemoval::Kept),
        };
        let Ok(api) = self.contact(false).await else {
            return Ok(KeyAfterRemoval::Kept);
        };
        match api.withdraw_join_request(&key).await {
            Ok(()) => Ok(KeyAfterRemoval::Removed),
            Err(error) if error.code() == Some(ErrorCode::KeyAlreadyRegistered) => {
                let _ = self.reconcile(&api, &before).await;
                let _ = self.save();
                // The device is logged in to the new account: its event stream starts, as
                // after a login of an action.
                self.announce_level();
                self.core.notify();
                Err(RemoveError::HasAccount)
            }
            Err(_) => Ok(KeyAfterRemoval::Kept),
        }
    }

    /// Withdraws the waiting join request of this device on the server, so that no
    /// approval makes an account for a key that this device then discards (ADR 0111). When
    /// the request was approved meanwhile, the device logs in and keeps its key.
    async fn withdraw_request(&self, before: &AccessState) -> Result<(), FlowError> {
        let Some(key) = self.stored_key()? else {
            return Ok(());
        };
        let api = self.contact(false).await?;
        match api.withdraw_join_request(&key).await {
            Ok(()) => Ok(()),
            Err(error) if error.code() == Some(ErrorCode::KeyAlreadyRegistered) => {
                self.reconcile(&api, before).await?;
                Err(FlowError::NotAvailable)
            }
            Err(error) => Err(self.failed(error)),
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
            Err(error) if error.code() == Some(ErrorCode::AccountInactive) => Ok(Probe::Inactive),
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
                Membership::Deleted { account_id } => Settled {
                    state: AccessState::Deleted { account_id },
                    membership: Membership::Deleted { account_id },
                },
                Membership::Member { .. }
                | Membership::Banned { .. }
                | Membership::Inactive { .. }
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
                Ok(status) => match after_request_status(&status, name, &membership) {
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
        if probe == Probe::Inactive && matches!(membership, Membership::Inactive { .. }) {
            let name = match before {
                AccessState::WaitingForApproval { name, .. } => name.as_str(),
                _ => "",
            };
            match api.join_request_status(&key).await {
                Ok(status) => {
                    if let Some(settled) = after_request_status(&status, name, &membership) {
                        return self.settle(settled, None);
                    }
                }
                Err(error) if error.code() == Some(ErrorCode::NotFound) => {}
                Err(error) => return Err(self.failed(error)),
            }
        }
        let unknown_without_a_trace = probe == Probe::UnknownDevice
            && matches!(membership, Membership::None | Membership::Removed)
            && !matches!(before, AccessState::WaitingForLink { .. });
        if unknown_without_a_trace
            && let Ok(status) = api.join_request_status(&key).await
            && status.state == JoinRequestState::Pending
            && let Some(settled) = after_request_status(&status, "", &membership)
        {
            // The server holds a request of this key that the book does not know: the
            // client stopped before it stored the request, or the book was replaced.
            return self.settle(settled, None);
        }
        let creates = if approved {
            Creates::Account
        } else {
            Creates::Nothing
        };
        self.settle_probe(before, probe, creates)
    }

    /// After an operation failed in a way that a login can explain, logs in. A session
    /// means that the device has its account; anything else leaves the first error.
    /// `creates` says what the operation creates when it succeeds.
    async fn explain_by_login(
        &self,
        api: &ApiClient,
        key: &DeviceKey,
        before: &AccessState,
        error: ApiError,
        creates: Creates,
    ) -> Result<(), FlowError> {
        if !login_explains(&error) {
            return Err(self.failed(error));
        }
        match self.probe(api, key).await {
            Ok(Probe::UnknownDevice) | Err(_) => Err(self.failed(error)),
            Ok(probe) => {
                // A lost answer: the operation was done. Otherwise the key had its account
                // before the operation, and only an approved join request of this device
                // can have created that account for it.
                let creates = if matches!(error, ApiError::OutcomeUnknown) {
                    creates
                } else if self.approved_request_created(api, key, &probe).await {
                    Creates::Account
                } else {
                    Creates::Nothing
                };
                self.settle_probe(before, probe, creates)
            }
        }
    }

    /// True when the book holds a join request of this device and the server states that
    /// the request was approved into the account of the login.
    async fn approved_request_created(
        &self,
        api: &ApiClient,
        key: &DeviceKey,
        probe: &Probe,
    ) -> bool {
        let Probe::LoggedIn(session) = probe else {
            return false;
        };
        if !matches!(self.membership(), Membership::JoinRequested { .. }) {
            return false;
        }
        matches!(
            api.join_request_status(key).await,
            Ok(status) if status.state == JoinRequestState::Approved
                && status.account_id == Some(session.account_id)
        )
    }

    /// Moves the flow to the state that a login attempt gives. When the operation created
    /// the account of the login, the avatar of the profile preset is sent to it (ADR 0717).
    fn settle_probe(
        &self,
        before: &AccessState,
        probe: Probe,
        creates: Creates,
    ) -> Result<(), FlowError> {
        let held = held_of(&probe);
        // A join of an inactive account returns it with its own name and avatar
        // (ADR 0121): the profile preset is not sent to it.
        let returning = |session: &Session| matches!(self.membership(), Membership::Inactive { account_id } if account_id == session.account_id);
        let new_account = match (&probe, creates) {
            (Probe::LoggedIn(session), Creates::Account) => !returning(session),
            (Probe::LoggedIn(session), Creates::Owner) => session.recoveries.is_empty(),
            _ => false,
        };
        let preset_waits = new_account
            && matches!(self.core.profile.load(), Ok(preset) if preset.avatar.is_some());
        let settled = after_probe(before, &self.membership(), probe, OffsetDateTime::now_utc());
        self.settle_account(settled, held, preset_waits)
    }

    /// Moves the flow to a state and stores the membership that belongs to it. The state
    /// in memory is what the server said. When the book cannot be written, the action
    /// reports that and the entry stays marked as unsaved until a write succeeds.
    fn settle(&self, settled: Settled, session: Option<Held>) -> Result<(), FlowError> {
        self.settle_account(settled, session, false)
    }

    /// The state of the profile preset belongs to one account (ADR 0717): it starts again
    /// when the device has another account or none, and it waits when `preset_waits`.
    fn settle_account(
        &self,
        mut settled: Settled,
        session: Option<Held>,
        preset_waits: bool,
    ) -> Result<(), FlowError> {
        {
            let mut state = lock(&self.state);
            if let AccessState::LoggedIn {
                account_id,
                device_id,
                recoveries,
                ..
            } = &mut settled.state
            {
                // The answer of a login can be older than an acknowledgement.
                let (account_id, device_id) = (*account_id, *device_id);
                recoveries.retain(|notice| {
                    !state
                        .acknowledged
                        .contains(&(account_id, device_id, notice.id))
                });
            }
            state.access = settled.state;
            if session.is_some() {
                state.login_wait = None;
            }
            state.session = session;
            let other_account = access::account_of(&state.entry.membership)
                != access::account_of(&settled.membership);
            let profile_sync = if preset_waits {
                ProfileSync::Pending { attempts: 0 }
            } else if other_account {
                ProfileSync::NotNeeded
            } else {
                state.entry.profile_sync
            };
            if preset_waits || other_account {
                state.preset_owner += 1;
            }
            // An answer of the server comes only after the logout of the person has ended.
            if state.entry.membership != settled.membership
                || state.entry.profile_sync != profile_sync
                || state.entry.logged_out
            {
                state.entry.membership = settled.membership;
                state.entry.profile_sync = profile_sync;
                state.entry.logged_out = false;
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
        let (membership, profile_sync, logged_out) = {
            let state = lock(&self.state);
            if !state.unsaved || state.retired {
                return Ok(());
            }
            (
                state.entry.membership.clone(),
                state.entry.profile_sync,
                state.entry.logged_out,
            )
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
            entry.logged_out = logged_out;
        })?;
        let mut state = lock(&self.state);
        if state.entry.membership == written
            && state.entry.profile_sync == profile_sync
            && state.entry.logged_out == logged_out
        {
            state.unsaved = false;
        }
        Ok(())
    }

    /// The logout that the person asked for. The state in memory changes first, then the
    /// book, then the server is told. The session ends on this device also when the book
    /// cannot be written or the server does not answer; the first failure is reported.
    async fn log_out_by_person(&self) -> Result<(), FlowError> {
        let (api, held, account_id) = {
            let mut state = lock(&self.state);
            let account_id = match state.access {
                AccessState::LoggedIn { account_id, .. }
                | AccessState::LoggedOut { account_id } => account_id,
                _ => return Err(FlowError::NotAvailable),
            };
            state.access = AccessState::LoggedOutByPerson { account_id };
            if !state.entry.logged_out {
                state.entry.logged_out = true;
                state.unsaved = true;
            }
            (state.api.clone(), state.session.take(), account_id)
        };
        tracing::debug!(server = %self.id, account = %account_id, "the person logs out");
        self.core.notify();
        let saved = self.save();
        let told = match (api, held) {
            (Some(api), Some(held)) => match api.log_out(&held.token).await {
                Ok(()) => Ok(()),
                Err(error) if error.category() == Some(ErrorCategory::SessionEnded) => Ok(()),
                Err(error) => Err(self.failed(error)),
            },
            _ => Ok(()),
        };
        saved.and(told)
    }

    /// Ends the logout of the person before a login that the person asked for: from then
    /// on the client logs in by itself again. A failed write leaves the logout as it was.
    fn end_logout_of_person(&self, account_id: AccountId) -> Result<(), FlowError> {
        {
            let mut state = lock(&self.state);
            state.access = AccessState::LoggedOut { account_id };
            state.entry.logged_out = false;
            state.unsaved = true;
        }
        self.save().inspect_err(|_| {
            let mut state = lock(&self.state);
            state.access = AccessState::LoggedOutByPerson { account_id };
            state.entry.logged_out = true;
        })
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
            entry.logged_out = false;
            keys.remove(entry.server_id).map_err(FlowError::from)
        })?;
        let mut state = lock(&self.state);
        state.access = AccessState::NoAccount { notice: None };
        state.session = None;
        state.entry.membership = Membership::None;
        state.entry.profile_sync = ProfileSync::NotNeeded;
        state.entry.logged_out = false;
        state.preset_owner += 1;
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
    async fn authorized<T, F, Sent>(
        self: Arc<Self>,
        outline: Outline,
        only_for: Option<AccountId>,
        exchange: F,
    ) -> Result<T, ApiError>
    where
        F: Fn(ApiClient, SecretToken, Outline) -> Sent,
        Sent: Future<Output = Result<T, ApiError>>,
    {
        let (api, held) = self.session()?;
        let started_for = only_for.unwrap_or(held.account_id);
        if held.account_id != started_for {
            return Err(ApiError::AccountChanged {
                was: started_for,
                now: held.account_id,
            });
        }
        let repeat = outline.clone();
        let result = match exchange(api, held.token.clone(), outline).await {
            Err(error) if error.category() == Some(ErrorCategory::SessionEnded) => {
                let (api, renewed) = self.renew(&held.token).await?;
                if renewed.account_id != started_for {
                    return Err(ApiError::AccountChanged {
                        was: started_for,
                        now: renewed.account_id,
                    });
                }
                exchange(api, renewed.token, repeat).await
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
        self.renew_after(ended, None).await
    }

    /// [`Server::renew`] after the event stream told why the session ended. A deletion is
    /// stated only there: the key is unknown afterwards, as after a removal (ADR 0121).
    async fn renew_after(
        &self,
        ended: &SecretToken,
        reason: Option<EndReason>,
    ) -> Result<(ApiClient, Held), ApiError> {
        let _gate = self.gate.lock().await;
        let (api, current) = self.session()?;
        if current.token != *ended {
            return Ok((api, current));
        }
        let waits = lock(&self.state)
            .login_wait
            .is_some_and(|(until, _)| tokio::time::Instant::now() < until);
        if waits {
            // The stream client tries the session again after its own wait.
            self.live.send(crate::live::Command::LoginUnanswered);
            return Err(ApiError::Unreachable(TransportError::new(
                TransportErrorKind::Timeout,
                "the last login got no answer, and the next one waits",
            )));
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
        let result = self.log_in_again(&before, api, current, reason).await;
        running.finish(result.as_ref().err().map(FlowError::from));
        result
    }

    /// `ended`: the session that the server no longer accepts.
    ///
    /// A login that the server refuses leaves the device logged out. A login without an
    /// answer of the server about the account tells nothing: the device keeps its state
    /// and the session, and the next request or the event stream finds the end of the
    /// session again and logs in again (ADR 0787).
    async fn log_in_again(
        &self,
        before: &AccessState,
        api: ApiClient,
        ended: Held,
        reason: Option<EndReason>,
    ) -> Result<(ApiClient, Held), ApiError> {
        let account_id = ended.account_id;
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
                if unanswered(&error) {
                    {
                        let mut state = lock(&self.state);
                        let failures = state.login_wait.map_or(0, |(_, failures)| failures);
                        let wait = error
                            .retry_after()
                            .unwrap_or(LOGIN_WAIT_FIRST * 2u32.pow(failures.min(6)))
                            .min(LOGIN_WAIT_LONGEST);
                        state.login_wait = Some((tokio::time::Instant::now() + wait, failures + 1));
                        state.session = Some(ended);
                    }
                    self.live.send(crate::live::Command::LoginUnanswered);
                } else {
                    logged_out();
                }
                self.note(&error);
                return Err(error);
            }
        };
        lock(&self.state).login_wait = None;
        let probe = match (probe, reason) {
            (Probe::UnknownDevice, Some(EndReason::Deleted)) => Probe::Deleted,
            (probe, _) => probe,
        };
        let outcome = match held_of(&probe) {
            Some(held) => Ok((api, held)),
            None => Err(ApiError::AccessLost(match probe {
                Probe::Banned => AccessLoss::Banned,
                Probe::Inactive => AccessLoss::Inactive,
                Probe::Deleted => AccessLoss::Deleted,
                Probe::LoggedIn(_) | Probe::UnknownDevice => AccessLoss::Removed,
            })),
        };
        if let Err(error) = self.settle_probe(before, probe, Creates::Nothing) {
            tracing::warn!(server = %self.id, %error, "the server book was not updated");
        }
        outcome
    }

    /// Sends the avatar of the profile preset to the account when it waits (ADR 0717).
    /// The name is not sent: it travelled with the join, the claim or the join request.
    /// The caller holds the lock of the tries.
    ///
    /// The request uses the session that the device has when the try starts and is not
    /// sent again with a renewed session: after a removal and a return, the account can be
    /// the same one and the avatar is no longer wanted.
    async fn apply_profile(
        &self,
        _applying: tokio::sync::MutexGuard<'_, ()>,
    ) -> Result<ProfileSync, ApiError> {
        let (sync, session, owner) = {
            let state = lock(&self.state);
            let session = match (&state.api, &state.session, state.retired) {
                (Some(api), Some(held), false) => Some((api.clone(), held.token.clone())),
                _ => None,
            };
            (state.entry.profile_sync, session, state.preset_owner)
        };
        let ProfileSync::Pending { attempts } = sync else {
            return Ok(sync);
        };
        let (api, token) = session.ok_or(ApiError::NotLoggedIn)?;
        let failed_try = ProfileSync::Pending {
            attempts: attempts.saturating_add(1),
        };
        let image = match self.core.profile.avatar() {
            Ok(Some((_, image))) => image,
            Ok(None) => return Ok(self.record_sync(owner, ProfileSync::NotNeeded)),
            Err(error) => {
                // A stored image that is larger than an avatar can be is never sent.
                let refused = matches!(error, StorageError::TooLarge { .. });
                self.record_sync(
                    owner,
                    if refused {
                        ProfileSync::Refused
                    } else {
                        failed_try
                    },
                );
                return Err(error.into());
            }
        };
        let stored = api.with_token(token).set_avatar(image).await;
        match &stored {
            Ok(_) => self.answered(),
            Err(error) => self.note(error),
        }
        match stored {
            Ok(_) => Ok(self.record_sync(owner, ProfileSync::Applied)),
            Err(error) => {
                // A server that refuses the image itself refuses it at every try.
                let refused = matches!(error, ApiError::Avatar(_))
                    || matches!(
                        error.code(),
                        Some(ErrorCode::ImageTooLarge | ErrorCode::ImageUnsupported)
                    );
                self.record_sync(
                    owner,
                    if refused {
                        ProfileSync::Refused
                    } else {
                        failed_try
                    },
                );
                Err(error)
            }
        }
    }

    fn api_for(self: &Arc<Self>, account_id: AccountId) -> ServerApi {
        ServerApi::new(Arc::new(Managed {
            server: self.clone(),
            account: Some(account_id),
        }))
    }

    async fn moderation_notices(&self) -> Result<ModerationNoticePage, FlowError> {
        let api = self.contact(false).await?;
        let key = self.stored_key().map_err(FlowError::from)?;
        let Some(key) = key else {
            return Ok(ModerationNoticePage::default());
        };
        api.moderation_notices(&key, None)
            .await
            .map_err(FlowError::from)
    }

    /// The logo from the route of the API version that was chosen for the server. Before
    /// the server was contacted in this run, the path of the server book is used.
    async fn logo(&self) -> Result<Option<AvatarImage>, ApiError> {
        let (api, path) = {
            let state = lock(&self.state);
            // An address that answered as another server gets nothing but the request
            // for the server information (ADR 0714).
            if matches!(state.reach, Reach::OtherServer { .. }) {
                return Ok(None);
            }
            (state.api.clone(), state.entry.logo_path.clone())
        };
        if let Some(api) = api {
            return api.server_logo().await;
        }
        match path.and_then(|path| self.address.logo_url(&path)) {
            Some(url) => logo_at(self.core.transport.as_ref(), url, &self.core.policy).await,
            None => Ok(None),
        }
    }

    /// Sends the acknowledgement of a recovery for the account that the flow shows, and
    /// takes the recovery out of the state when the server holds its notice no longer.
    async fn acknowledge_recovery(
        self: &Arc<Self>,
        recovery_id: RecoveryId,
    ) -> Result<(), ApiError> {
        let (account, device) = match &lock(&self.state).access {
            AccessState::LoggedIn {
                account_id,
                device_id,
                ..
            } => (*account_id, *device_id),
            _ => return Err(ApiError::NotLoggedIn),
        };
        let result = self
            .api_for(account)
            .acknowledge_recovery(recovery_id)
            .await;
        let gone = match &result {
            Ok(()) => true,
            Err(error) => error.code() == Some(ErrorCode::NotFound),
        };
        if gone {
            {
                let mut state = lock(&self.state);
                let key = (account, device, recovery_id);
                if !state.acknowledged.contains(&key) {
                    state.acknowledged.push(key);
                }
                // The answer belongs to the account and the device that asked: a notice
                // that the entry shows for another one by now stays.
                if let AccessState::LoggedIn {
                    account_id,
                    device_id,
                    recoveries,
                    ..
                } = &mut state.access
                    && (*account_id, *device_id) == (account, device)
                {
                    recoveries.retain(|notice| notice.id != recovery_id);
                }
            }
            self.core.notify();
        }
        result
    }

    /// Records the result of a try to send the avatar. `owner` is the count that the try
    /// read when it started: a result for an account that the device left meanwhile, also
    /// when it has returned to it, is dropped.
    fn record_sync(&self, owner: u64, sync: ProfileSync) -> ProfileSync {
        {
            let mut state = lock(&self.state);
            if state.preset_owner != owner {
                return state.entry.profile_sync;
            }
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

/// The server object as its event stream client reaches it.
struct StreamHost(Weak<Server>);

impl Host for StreamHost {
    /// Logs in again with the device key, once (ADR 0712). The result reaches the stream
    /// client as another session, or as the level `off`.
    fn session_ended(&self, token: &SecretToken, reason: Option<EndReason>) {
        let Some(server) = self.0.upgrade() else {
            return;
        };
        let token = token.clone();
        let runtime = server.core.runtime.clone();
        drop(runtime.run(async move {
            let _ = server.renew_after(&token, reason).await;
        }));
    }

    fn in_call(&self, in_call: bool) {
        let Some(server) = self.0.upgrade() else {
            return;
        };
        if std::mem::replace(&mut lock(&server.state).in_call, in_call) == in_call {
            return;
        }
        server.announce_level();
        server.core.notify();
    }
}

impl EventStreams for Streams {
    fn set_level(&self, server: &Server, level: ConnectionLevel, target: Option<StreamTarget>) {
        let host = || Arc::new(StreamHost(server.this.clone())) as Arc<dyn Host>;
        let gone = lock(&server.state).retired;
        self.set(&server.live, host, Desired { level, target }, gone);
    }
}

#[cfg(test)]
mod tests;
