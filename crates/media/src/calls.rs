//! The one call of a client over all its servers (ADR 0107, ADR 0734).
//!
//! A server decides who is in a call and gives the token for the media server (ADR 0510);
//! the engine carries the media. This module joins the two: it asks a server for a place,
//! starts the engine with the answer, tells the server the person's own state, applies the
//! blocks that the server reports, connects again after a loss, and ends the engine when
//! the server ends the place.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use junto_client_core::api::{ApiError, ErrorCategory};
use junto_client_core::flow::FlowError;
use junto_client_core::live::{Missed, Notice, Notices, StreamStatus};
use junto_client_core::model::ServerModel;
use junto_client_core::protocol::call::{
    CallJoin, CallMove, CallPlace, LeaveReason, MicrophoneBlock, MoveOutcome, OwnCall, ShareAudio,
    SoundBlock,
};
use junto_client_core::protocol::error::ErrorCode;
use junto_client_core::protocol::events::{Cursor, Event};
use junto_client_core::protocol::ids::{
    AccountId, CallInvitationId, CallMoveId, ServerId, StreamEpoch,
};
use junto_client_core::{Client, ServerHandle};
use livekit::prelude::{Room, RoomOptions};
use time::OffsetDateTime;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::time::Instant;

use crate::audio::{AudioError, AudioHost, DeviceInfo, Devices};
use crate::engine::{
    Connection, Controls, Engine, EngineOptions, EngineState, Levels, MainPicture, MediaJoin,
    MicrophoneState, OutputState, SeenShare, ShareId, ShareState, VoiceConfig, Watching,
};
use crate::permission::{MicrophoneAccess, MicrophonePermission};
use crate::share::picture::PictureFrame;
use crate::share::{
    CaptureError, CaptureRequest, Pending, ScreenAccess, ScreenCapture, ShareSources,
};

/// The times of the work that the calls do by themselves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CallTiming {
    /// The wait before the first request for a new token after the media connection was
    /// lost. It doubles with each further request, up to `reconnect_longest`, until a media
    /// connection is made.
    pub reconnect_first: Duration,
    pub reconnect_longest: Duration,
    /// The shortest time between two reports of the own state. The server takes 30 in
    /// 10 seconds (ADR 0510).
    pub state_spacing: Duration,
    /// The shortest time between two reports of the share of this device. They count
    /// against the limit of the reports of the own state.
    pub share_spacing: Duration,
    /// The shortest time between two reports of speaking. The server takes 20 in
    /// 10 seconds (ADR 0513).
    pub speaking_spacing: Duration,
    /// A report that the person speaks lasts 5 seconds on the server and is renewed after
    /// this time.
    pub speaking_renewal: Duration,
    /// After this time, a state of the server that has never shown the place of this
    /// device means that the place has ended. Before, the events of the join may not have
    /// arrived.
    pub settle: Duration,
}

impl Default for CallTiming {
    fn default() -> Self {
        Self {
            reconnect_first: Duration::from_secs(1),
            reconnect_longest: Duration::from_secs(5),
            state_spacing: Duration::from_millis(500),
            share_spacing: Duration::from_secs(1),
            speaking_spacing: Duration::from_millis(700),
            speaking_renewal: Duration::from_secs(3),
            settle: Duration::from_secs(10),
        }
    }
}

/// The sound devices, the permission, the screen capture and the times that the calls of a
/// client use.
#[derive(Clone)]
pub struct MediaOptions {
    pub host: Arc<dyn AudioHost>,
    pub permission: Arc<dyn MicrophonePermission>,
    pub capture: Arc<dyn ScreenCapture>,
    pub timing: CallTiming,
}

impl MediaOptions {
    /// The devices, the permission and the screen capture of the machine.
    pub fn system() -> Self {
        Self {
            host: crate::audio::system(),
            permission: Arc::new(crate::permission::SystemPermission),
            capture: crate::share::system(),
            timing: CallTiming::default(),
        }
    }
}

/// A call on one server.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CallTarget {
    pub server_id: ServerId,
    pub place: CallPlace,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// The first media connection of the call is made.
    Connecting,
    Connected,
    /// The media connection is interrupted or lost, and the client restores it.
    Reconnecting,
}

/// The call that this device is in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveCall {
    pub target: CallTarget,
    /// The identity of this device in the room (ADR 0508).
    pub identity: String,
    pub phase: Phase,
    /// When the media connection was interrupted. Present while the phase is
    /// `Reconnecting`.
    pub interrupted_at: Option<OffsetDateTime>,
    pub microphone: MicrophoneState,
    pub access: MicrophoneAccess,
    pub input_device: Option<DeviceInfo>,
    pub output: OutputState,
    pub output_device: Option<DeviceInfo>,
    /// The server closed the microphone of the person (ADR 0503, ADR 0512).
    pub microphone_block: Option<MicrophoneBlock>,
    pub sound_block: Option<SoundBlock>,
    /// The share of this device.
    pub share: ShareState,
    /// The shares of the other people, in the order in which they were seen.
    pub shares: Vec<SeenShare>,
    /// The share whose picture and sound the person gets.
    pub watched: Option<ShareId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EndReason {
    /// The server ended the place. The reason is absent when the client did not receive
    /// the event that states it.
    Server(Option<LeaveReason>),
    /// The account is in a call with another device or in another call.
    Elsewhere,
    /// The media connection was lost, and the server refused a new token.
    Refused(FlowError),
}

/// How the last join or the last call ended, when the person did not end it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    JoinFailed {
        target: CallTarget,
        error: FlowError,
    },
    Ended {
        target: CallTarget,
        reason: EndReason,
    },
    /// A person with "move members" moved the person of this device into this call
    /// (ADR 0511).
    Moved { target: CallTarget },
}

/// How a device asks for its place in a call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Entry {
    /// The request of a person who may enter, or of the device that holds the place.
    Open,
    /// The acceptance of an invitation, which passes the lock and the user limit
    /// (ADR 0514).
    Invitation(CallInvitationId),
    /// The place that a move gave (ADR 0511).
    Moved,
}

/// What an interface shows of the calls of this client.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CallsView {
    /// The person closed the microphone. The setting holds in a call and outside one.
    pub muted: bool,
    /// The person turned the sound off.
    pub deafened: bool,
    /// The place that a join asks for now.
    pub joining: Option<CallTarget>,
    pub call: Option<ActiveCall>,
    pub outcome: Option<Outcome>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum JoinError {
    #[error("another join is in progress")]
    Busy,
    #[error("the server is not saved")]
    NoServer,
    /// The person left while the server was asked. A place that the server gave was given
    /// up, and no media connection was started.
    #[error("the person left before the call began")]
    Cancelled,
    #[error(transparent)]
    Refused(FlowError),
}

enum SessionCommand {
    Controls,
    Talking(bool),
    Volume(AccountId, f32),
    Config(VoiceConfig),
    RetryMicrophone,
    StartShare(CaptureRequest),
    StopShare,
    Watch(Option<ShareId>),
    ShareVolume(f32),
    ViewSize(u32, u32),
    #[cfg(feature = "test-support")]
    SimulateFullReconnect,
    #[cfg(feature = "test-support")]
    Statistics(crate::engine::statistics::Reply),
    #[cfg(feature = "test-support")]
    SoundCheck(crate::engine::statistics::SoundReply),
    /// Ends the call on this device. Who ends it gets the identity of the place and tells
    /// the server.
    End {
        done: oneshot::Sender<String>,
    },
}

struct ActiveHandle {
    key: u64,
    target: CallTarget,
    commands: mpsc::UnboundedSender<SessionCommand>,
}

/// The join that the person asked for last and did not end.
#[derive(Clone, Copy)]
struct Joining {
    key: u64,
    target: CallTarget,
}

#[derive(Default)]
struct Shared {
    muted: bool,
    deafened: bool,
    talking: bool,
    voice: VoiceConfig,
    volumes: HashMap<(ServerId, AccountId), f32>,
    /// The volume of the sound of the share that the person watches: 1.0 is 100 %.
    share_volume: Option<f32>,
    /// The size in pixels inside which the interface draws the watched share.
    view_size: Option<(u32, u32)>,
    joining: Option<Joining>,
    next_key: u64,
    active: Option<ActiveHandle>,
}

/// One piece of work that asks a server for a place, or gives one up.
type Turn = Pin<Box<dyn Future<Output = ()> + Send + 'static>>;

struct Inner {
    client: Client,
    runtime: tokio::runtime::Handle,
    options: MediaOptions,
    shared: Mutex<Shared>,
    view: watch::Sender<CallsView>,
    levels: watch::Sender<Levels>,
    main_picture: watch::Sender<Option<MainPicture>>,
    small_pictures: watch::Sender<HashMap<ShareId, Arc<PictureFrame>>>,
    turns: mpsc::UnboundedSender<Turn>,
}

impl Inner {
    fn shared(&self) -> std::sync::MutexGuard<'_, Shared> {
        self.shared.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn tell(&self, command: SessionCommand) {
        if let Some(active) = &self.shared().active {
            let _ = active.commands.send(command);
        }
    }

    /// Runs `work` after every piece of work that was given before it. A caller that also
    /// changes what the calls hold gives the work while it holds the lock of that state, so
    /// the order of the work is the order of the changes. A join, a leave and
    /// a request for a new token each ask a server about the place of this client, and the
    /// answer of one decides what the next one finds. They run one at a time, in the order
    /// in which they were started, and each uses its own answer before the next one starts:
    /// no answer arrives after the person has done something else.
    fn in_turn<T: Send + 'static>(
        &self,
        work: impl Future<Output = T> + Send + 'static,
    ) -> oneshot::Receiver<T> {
        let (done, result) = oneshot::channel();
        let _ = self.turns.send(Box::pin(async move {
            let _ = done.send(work.await);
        }));
        result
    }

    fn joins(&self, key: u64) -> bool {
        self.shared()
            .joining
            .is_some_and(|joining| joining.key == key)
    }

    /// The join has ended. It is shown as asked for until now, unless the person ended it.
    fn joined(&self, key: u64) {
        let mut shared = self.shared();
        if shared.joining.is_some_and(|joining| joining.key == key) {
            shared.joining = None;
            self.view.send_modify(|view| view.joining = None);
        }
    }
}

async fn turns(runtime: tokio::runtime::Handle, mut turns: mpsc::UnboundedReceiver<Turn>) {
    while let Some(turn) = turns.recv().await {
        // A task of its own: a failure of one piece of work does not end the others.
        let _ = runtime.spawn(turn).await;
    }
}

/// The calls of one client. Every clone is the same object.
#[derive(Clone)]
pub struct Calls {
    inner: Arc<Inner>,
}

impl Calls {
    pub fn new(client: Client, options: MediaOptions) -> Self {
        let runtime = client.runtime().handle().clone();
        let (turns_given, turns_taken) = mpsc::unbounded_channel();
        runtime.spawn(turns(runtime.clone(), turns_taken));
        Self {
            inner: Arc::new(Inner {
                client,
                runtime,
                options,
                shared: Mutex::new(Shared::default()),
                view: watch::Sender::new(CallsView::default()),
                levels: watch::Sender::new(Levels::default()),
                main_picture: watch::Sender::new(None),
                small_pictures: watch::Sender::new(HashMap::new()),
                turns: turns_given,
            }),
        }
    }

    pub fn view(&self) -> CallsView {
        self.inner.view.borrow().clone()
    }

    /// Wakes after each change of the view.
    pub fn watch(&self) -> watch::Receiver<CallsView> {
        self.inner.view.subscribe()
    }

    /// The levels of the call that this device is in. Empty without a call.
    pub fn levels(&self) -> watch::Receiver<Levels> {
        self.inner.levels.subscribe()
    }

    pub fn devices(&self) -> Result<Devices, AudioError> {
        self.inner.options.host.devices()
    }

    /// The microphone permission of the system now. The call shows no question.
    pub fn microphone_access(&self) -> MicrophoneAccess {
        self.inner.options.permission.state()
    }

    pub fn set_muted(&self, muted: bool) {
        self.inner.shared().muted = muted;
        self.inner.view.send_modify(|view| view.muted = muted);
        self.inner.tell(SessionCommand::Controls);
    }

    pub fn set_deafened(&self, deafened: bool) {
        self.inner.shared().deafened = deafened;
        self.inner.view.send_modify(|view| view.deafened = deafened);
        self.inner.tell(SessionCommand::Controls);
    }

    /// The push-to-talk key is held or released (ADR 0507).
    pub fn set_talking(&self, talking: bool) {
        let mut shared = self.inner.shared();
        if shared.talking != talking {
            shared.talking = talking;
            drop(shared);
            self.inner.tell(SessionCommand::Talking(talking));
        }
    }

    /// The sound settings of the person. They hold for the call of now and for later ones.
    pub fn set_voice(&self, voice: VoiceConfig) {
        self.inner.shared().voice = voice.clone();
        self.inner.tell(SessionCommand::Config(voice));
    }

    /// The volume of one person for this listener: 1.0 is 100 % (ADR 0507).
    pub fn set_volume(&self, server_id: ServerId, account_id: AccountId, gain: f32) {
        let mut shared = self.inner.shared();
        shared.volumes.insert((server_id, account_id), gain);
        if let Some(active) = &shared.active
            && active.target.server_id == server_id
        {
            let _ = active
                .commands
                .send(SessionCommand::Volume(account_id, gain));
        }
    }

    /// Opens the microphone again after it failed or was refused.
    pub fn retry_microphone(&self) {
        self.inner.tell(SessionCommand::RetryMicrophone);
    }

    /// Whether this build can share a screen on the system that it runs on.
    pub fn can_share(&self) -> bool {
        self.inner.options.capture.supported()
    }

    /// Whether the system lets this application record the screen. The call shows no
    /// question.
    pub fn screen_access(&self) -> ScreenAccess {
        self.inner.options.capture.access()
    }

    /// Makes the system list this application for the permission and show its question,
    /// and opens the settings of the system where the person gives it (ADR 0502).
    pub fn ask_for_screen_access(&self) {
        self.inner.options.capture.request_access();
        self.inner.options.capture.open_settings();
    }

    /// The screens, windows and applications that can be shared now.
    pub fn share_sources(&self) -> Pending<Result<ShareSources, CaptureError>> {
        // The Linux capture reads its sources with work that needs the runtime of the
        // client, and the window polls this future on a thread without one.
        #[cfg(target_os = "linux")]
        {
            let reading = self
                .inner
                .runtime
                .spawn(self.inner.options.capture.sources());
            Box::pin(async move {
                reading
                    .await
                    .map_err(|error| CaptureError::Failed(error.to_string()))?
            })
        }
        #[cfg(not(target_os = "linux"))]
        {
            self.inner.options.capture.sources()
        }
    }

    /// Starts the share of this device in its call. The result is in the view of the
    /// call. Without a call nothing starts.
    pub fn start_share(&self, request: CaptureRequest) {
        self.inner.tell(SessionCommand::StartShare(request));
    }

    /// Ends the share of this device, and puts away the statement that a share ended by
    /// itself.
    pub fn stop_share(&self) {
        self.inner.tell(SessionCommand::StopShare);
    }

    /// The share that the person chose to watch in the call of this device (ADR 0502).
    /// Its sound is played, and the sound of every other share is not.
    pub fn watch_share(&self, share: ShareId) {
        self.inner.tell(SessionCommand::Watch(Some(share)));
    }

    /// The volume of the sound of the watched share: 1.0 is 100 %, and 0.0 keeps the
    /// picture without its sound.
    pub fn set_share_volume(&self, gain: f32) {
        self.inner.shared().share_volume = Some(gain);
        self.inner.tell(SessionCommand::ShareVolume(gain));
    }

    /// The size in pixels inside which the interface draws the watched share. The media
    /// server is asked for the layer of that size.
    pub fn set_view_size(&self, width: u32, height: u32) {
        let mut shared = self.inner.shared();
        if shared.view_size != Some((width, height)) {
            shared.view_size = Some((width, height));
            drop(shared);
            self.inner.tell(SessionCommand::ViewSize(width, height));
        }
    }

    /// Makes the call library of the call of this device connect anew, for a test.
    #[cfg(feature = "test-support")]
    pub fn simulate_full_reconnect(&self) {
        self.inner.tell(SessionCommand::SimulateFullReconnect);
    }

    /// Queries the connection statistics for a failed live test.
    #[cfg(feature = "test-support")]
    pub fn statistics(
        &self,
    ) -> impl Future<Output = Result<livekit::rtc_engine::SessionStats, String>> + Send + 'static
    {
        let (reply, stats) = oneshot::channel();
        self.inner.tell(SessionCommand::Statistics(reply));
        async move {
            stats
                .await
                .unwrap_or_else(|_| Err("the call ended before its statistics answered".to_owned()))
        }
    }

    /// What the engine of the call of this device knows of its sound now, for a live test
    /// that failed. Nothing when this device has no call.
    #[cfg(feature = "test-support")]
    pub fn sound_check(
        &self,
    ) -> impl Future<Output = Option<crate::engine::statistics::SoundCheck>> + Send + 'static {
        let (reply, check) = oneshot::channel();
        self.inner.tell(SessionCommand::SoundCheck(reply));
        async move { check.await.ok() }
    }

    /// The picture of the watched share of the call of this device.
    pub fn main_picture(&self) -> watch::Receiver<Option<MainPicture>> {
        self.inner.main_picture.subscribe()
    }

    /// A small picture of each share of the call of this device.
    pub fn small_pictures(&self) -> watch::Receiver<HashMap<ShareId, Arc<PictureFrame>>> {
        self.inner.small_pictures.subscribe()
    }

    /// Forgets how the last join or call ended.
    pub fn dismiss_outcome(&self) {
        self.inner
            .view
            .send_if_modified(|view| view.outcome.take().is_some());
    }

    /// Enters the call at a place. The server is asked first: a refusal leaves a call that
    /// this device is in as it is. When the place is given, that call ends, and the media
    /// connection of the new one starts (ADR 0734). The join runs to its end when the
    /// returned future is dropped. A [`Calls::leave`] before the answer of the server ends
    /// the join: a place that the server gives then is given up.
    pub fn join(
        &self,
        target: CallTarget,
    ) -> impl Future<Output = Result<(), JoinError>> + Send + 'static + use<> {
        self.enter(target, Entry::Open)
    }

    /// Accepts an invitation into a call and enters it, as [`Calls::join`] enters a call.
    pub fn join_invited(
        &self,
        target: CallTarget,
        invitation_id: CallInvitationId,
    ) -> impl Future<Output = Result<(), JoinError>> + Send + 'static + use<> {
        self.enter(target, Entry::Invitation(invitation_id))
    }

    fn enter(
        &self,
        target: CallTarget,
        entry: Entry,
    ) -> impl Future<Output = Result<(), JoinError>> + Send + 'static + use<> {
        let asked = ask(&self.inner, target, entry);
        async move {
            asked?
                .await
                .unwrap_or(Err(JoinError::Refused(FlowError::Stopped)))
        }
    }

    /// Leaves the call that this device is in, and ends a join that waits for the answer
    /// of its server. Without either, only the work below is waited for. The call ends on
    /// this device at once. The future ends when the server was told, which is after every
    /// request for a place that was started before has its answer and has given up a
    /// place that nobody uses.
    pub fn leave(&self) -> impl Future<Output = ()> + Send + 'static + use<> {
        let inner = self.inner.clone();
        let told = {
            let mut shared = self.inner.shared();
            if shared.joining.take().is_some() {
                self.inner.view.send_modify(|view| view.joining = None);
            }
            let ending = shared.active.as_ref().and_then(|active| {
                let (done, ended) = oneshot::channel();
                active
                    .commands
                    .send(SessionCommand::End { done })
                    .ok()
                    .map(|()| (ended, active.target.server_id))
            });
            self.inner.in_turn(async move {
                // A session that ended by itself had no place any more.
                if let Some((ended, server_id)) = ending
                    && let Ok(identity) = ended.await
                {
                    give_up(&inner, server_id, &identity).await;
                }
            })
        };
        async move {
            let _ = told.await;
        }
    }
}

/// Notes the join as the one that the person asks for, and gives its work. One join at a
/// time: a second one while the first waits for its server is refused.
fn ask(
    inner: &Arc<Inner>,
    target: CallTarget,
    entry: Entry,
) -> Result<oneshot::Receiver<Result<(), JoinError>>, JoinError> {
    let server = inner
        .client
        .server(target.server_id)
        .ok_or(JoinError::NoServer)?;
    let mut shared = inner.shared();
    if shared.joining.is_some() {
        return Err(JoinError::Busy);
    }
    shared.next_key += 1;
    let key = shared.next_key;
    shared.joining = Some(Joining { key, target });
    inner.view.send_modify(|view| {
        view.joining = Some(target);
        view.outcome = None;
    });
    Ok(inner.in_turn(join(inner.clone(), key, server, target, entry)))
}

/// Ends the active call on this device and waits until its session has ended. Returns the
/// identity of its place, or nothing when no call was active or the call ended by itself.
async fn end_active(inner: &Arc<Inner>) -> Option<String> {
    let (done, ended) = oneshot::channel();
    let sent = match &inner.shared().active {
        Some(active) => active.commands.send(SessionCommand::End { done }).is_ok(),
        None => false,
    };
    if sent { ended.await.ok() } else { None }
}

/// Ends a place of this device on a server that no media connection of this client uses.
/// The request names the identity that this device was given, so the server ends only that
/// place: a place that another device or session of the account took meanwhile stays
/// (ADR 0510).
async fn give_up(inner: &Arc<Inner>, server_id: ServerId, identity: &str) {
    let Some(server) = inner.client.server(server_id) else {
        return;
    };
    // Without an answer the server ends the place by itself, because no media connection
    // uses it (ADR 0509).
    if let Err(error) = server.api().leave_call_as(identity).await {
        tracing::debug!(%error, "the server was not told that the call was left");
    }
}

async fn request(
    server: &ServerHandle,
    place: &CallPlace,
    entry: Entry,
) -> Result<CallJoin, ApiError> {
    let api = server.api();
    if let Entry::Invitation(invitation_id) = entry {
        return api.accept_call_invitation(invitation_id).await;
    }
    match place {
        CallPlace::Channel { channel_id, .. } => api.join_channel_call(*channel_id).await,
        CallPlace::Direct {
            low_account_id,
            high_account_id,
        } => {
            let own = server.account();
            let other = if own == Some(*low_account_id) {
                *high_account_id
            } else {
                *low_account_id
            };
            api.join_direct_call(other).await
        }
    }
}

async fn join(
    inner: Arc<Inner>,
    key: u64,
    server: ServerHandle,
    target: CallTarget,
    entry: Entry,
) -> Result<(), JoinError> {
    if !inner.joins(key) {
        // The person left while the join waited for its turn. The server was not asked.
        return Err(JoinError::Cancelled);
    }
    let answer = request(&server, &target.place, entry).await;
    let result = match answer {
        Ok(answer) => {
            let identity = answer.identity.clone();
            if started(&inner, key, server, target, answer).await {
                Ok(())
            } else {
                // The person left while the server was asked, and the server gave a
                // place. No media connection uses it.
                give_up(&inner, target.server_id, &identity).await;
                Err(JoinError::Cancelled)
            }
        }
        Err(_) if !inner.joins(key) => Err(JoinError::Cancelled),
        Err(error) => {
            let error = refusal(&error);
            inner.view.send_modify(|view| {
                view.outcome = Some(Outcome::JoinFailed {
                    target,
                    error: error.clone(),
                });
            });
            Err(JoinError::Refused(error))
        }
    };
    inner.joined(key);
    if entry == Entry::Moved && result.is_ok() {
        inner
            .view
            .send_modify(|view| view.outcome = Some(Outcome::Moved { target }));
    }
    result
}

/// Ends the earlier call and starts the new one, unless the person has ended the join.
async fn started(
    inner: &Arc<Inner>,
    key: u64,
    server: ServerHandle,
    target: CallTarget,
    answer: CallJoin,
) -> bool {
    if !inner.joins(key) {
        return false;
    }
    // The earlier call ends only now: a refusal has left it as it was. A call on the same
    // server was ended by that server when it gave the new place (ADR 0510), so only a
    // call on another server is left there.
    let earlier = inner.shared().active.as_ref().map(|active| active.target);
    if let Some(earlier) = earlier
        && let Some(identity) = end_active(inner).await
        && earlier.server_id != target.server_id
    {
        give_up(inner, earlier.server_id, &identity).await;
    }
    start(inner, key, server, target, answer)
}

/// Why a server gave no place in a call. The general form of a failure names a server that
/// failed; a server without a reachable media server is named as that.
fn refusal(error: &ApiError) -> FlowError {
    match error.code() {
        Some(code @ ErrorCode::MediaUnavailable) => FlowError::Refused {
            code,
            category: ErrorCategory::ServerFault,
        },
        _ => FlowError::from(error),
    }
}

fn media_join(answer: &CallJoin) -> MediaJoin {
    MediaJoin {
        url: answer.media_url.clone(),
        token: answer.token.clone(),
    }
}

/// Starts the media connection of a call. Returns false, and starts nothing, when the
/// person has ended the join.
fn start(
    inner: &Arc<Inner>,
    join: u64,
    server: ServerHandle,
    target: CallTarget,
    answer: CallJoin,
) -> bool {
    let (commands, taken) = mpsc::unbounded_channel();
    let mut shared = inner.shared();
    if shared.joining.is_none_or(|joining| joining.key != join) {
        return false;
    }
    shared.next_key += 1;
    let key = shared.next_key;
    // The state of the server lists the person only after the media connection is made.
    // The blocks of the answer hold from the first moment (ADR 0503).
    let controls = Controls {
        muted: shared.muted,
        deafened: shared.deafened,
        microphone_blocked: answer.microphone_block.is_some(),
        sound_blocked: answer.sound_block.is_some(),
    };
    let volumes = shared
        .volumes
        .iter()
        .filter(|((server_id, _), _)| *server_id == target.server_id)
        .map(|((_, account_id), gain)| (*account_id, *gain))
        .collect();
    let engine = Engine::start(
        &inner.runtime,
        EngineOptions {
            host: inner.options.host.clone(),
            permission: inner.options.permission.clone(),
            capture: inner.options.capture.clone(),
        },
        media_join(&answer),
        shared.voice.clone(),
        controls,
        volumes,
        Watching {
            choice: None,
            volume: shared.share_volume.unwrap_or(1.0),
        },
    );
    if shared.talking {
        engine.set_talking(true);
    }
    if let Some((width, height)) = shared.view_size {
        engine.set_view_size(width, height);
    }
    shared.active = Some(ActiveHandle {
        key,
        target,
        commands,
    });
    drop(shared);
    let timing = inner.options.timing;
    let session = Session {
        inner: inner.clone(),
        key,
        server,
        target,
        place: Place::new(answer.identity, Instant::now(), timing.settle),
        fate: Fate::Undecided,
        review: true,
        engine,
        controls,
        microphone_block: answer.microphone_block,
        sound_block: answer.sound_block,
        connected_once: false,
        interrupted_at: None,
        retry: None,
        backoff: Backoff::new(timing.reconnect_first, timing.reconnect_longest),
        rejoining: false,
        state_report: Reporter::new(timing.state_spacing, None),
        speaking_report: Reporter::new(timing.speaking_spacing, Some(timing.speaking_renewal)),
        share_report: Reporter::holding(timing.share_spacing, None),
        moving: None,
        speaking: false,
        timing,
    };
    session.show();
    inner.runtime.spawn(session.run(taken));
    true
}

/// Decides when a report is sent: a changed value at once, at most one request at a time,
/// and never two requests closer than the spacing. A value that the server forgets is sent
/// again before it does.
#[derive(Debug)]
pub(crate) struct Reporter<T> {
    spacing: Duration,
    renewal: Option<Duration>,
    sent: Option<T>,
    last: Option<Instant>,
    in_flight: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Step {
    Send,
    Wait(Instant),
    Idle,
}

impl<T: PartialEq + Clone> Reporter<T> {
    pub(crate) fn new(spacing: Duration, renewal: Option<Duration>) -> Self {
        Self {
            spacing,
            renewal,
            sent: None,
            last: None,
            in_flight: false,
        }
    }

    /// A reporter for a value that the server holds already: `value` is sent only after
    /// another value was.
    pub(crate) fn holding(spacing: Duration, value: T) -> Self {
        Self {
            sent: Some(value),
            ..Self::new(spacing, None)
        }
    }

    /// What to do for the value that holds now. `renews`: the server forgets this value
    /// unless it is sent again.
    pub(crate) fn step(&self, wanted: &T, renews: bool, now: Instant) -> Step {
        if self.in_flight {
            return Step::Idle;
        }
        let earliest = self.last.map(|last| last + self.spacing);
        let renew_at = match (self.renewal, self.last) {
            (Some(renewal), Some(last)) if renews => Some(last + renewal),
            _ => None,
        };
        let changed = self.sent.as_ref() != Some(wanted);
        let due = changed || renew_at.is_some_and(|at| now >= at);
        match (due, earliest) {
            (true, Some(earliest)) if now < earliest => Step::Wait(earliest),
            (true, _) => Step::Send,
            (false, _) => renew_at.map_or(Step::Idle, Step::Wait),
        }
    }

    pub(crate) fn sending(&mut self, value: T, now: Instant) {
        self.sent = Some(value);
        self.last = Some(now);
        self.in_flight = true;
    }

    /// The request has ended. A value that the server did not take is sent again, not
    /// before `retry` has passed.
    pub(crate) fn done(&mut self, taken: bool, retry: Duration, now: Instant) {
        self.in_flight = false;
        if !taken {
            self.sent = None;
            self.last = Some(now + retry.saturating_sub(self.spacing));
        }
    }
}

/// What is known of the place of this device in a call: its identity, and what the state
/// and the events of the server have told about it. A state of the server names the event
/// up to which it holds, and an event has a number, so the two are put in one order: the
/// session reads them from two sources, and the state can be ahead of the events.
///
/// The numbers count within one history of the server. A server whose data was restored
/// starts another history, and the client loads its state again. Numbers of two histories
/// are not compared.
#[derive(Debug)]
pub(crate) struct Place {
    identity: String,
    /// When the server gave the place.
    since: Instant,
    settle: Duration,
    /// The history of the last state that was read.
    history: Option<StreamEpoch>,
    /// The history of the events that are read now.
    events: Option<StreamEpoch>,
    /// The number of the last event, or of the last state, that showed the place.
    shown: Option<u64>,
    /// The last event that told that the account is in no call: its number and its reason.
    left: Option<(u64, Option<LeaveReason>)>,
}

/// What a client does with its place now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Fate {
    /// A current state of the server shows the place.
    Held,
    /// Nothing tells whether the place exists: there is no current state, the server has
    /// not shown the place yet, or this client asks the server for a place now.
    Undecided,
    End(EndReason),
}

impl Place {
    pub(crate) fn new(identity: String, now: Instant, settle: Duration) -> Self {
        Self {
            identity,
            since: now,
            settle,
            history: None,
            events: None,
            shown: None,
            left: None,
        }
    }

    pub(crate) fn identity(&self) -> &str {
        &self.identity
    }

    /// The state was loaded again. The events that follow belong to its history.
    pub(crate) fn reloaded(&mut self, history: StreamEpoch) {
        self.events = Some(history);
    }

    /// Takes an event of the server with its number.
    pub(crate) fn note(&mut self, id: u64, event: &Event) {
        if self.events.is_some() && self.history.is_some() && self.events != self.history {
            // An event of the history before the one of the state.
            return;
        }
        match event {
            Event::OwnCallChanged {
                call: Some(own), ..
            } if own.identity == self.identity => {
                self.shown = self.shown.max(Some(id));
            }
            Event::OwnCallChanged { call: None, left }
                if self.left.is_none_or(|(before, _)| id > before) =>
            {
                self.left = Some((id, *left));
            }
            _ => {}
        }
    }

    /// The reason of the event that ended the place: an event that tells that the account
    /// is in no call, and that follows the last sign of the place. An event from before
    /// that sign tells the end of an earlier call.
    fn ended(&self) -> Option<Option<LeaveReason>> {
        self.left
            .filter(|(id, _)| self.shown.is_none_or(|shown| *id > shown))
            .map(|(_, reason)| reason)
    }

    /// Why the server ended the place, when an event has told it.
    pub(crate) fn cause(&self) -> Option<LeaveReason> {
        self.ended().flatten()
    }

    /// When the server must have shown the place. Absent when it has.
    pub(crate) fn settles_at(&self) -> Option<Instant> {
        self.shown.is_none().then_some(self.since + self.settle)
    }

    /// Decides from what is known now. `state`: the cursor of a current state of the
    /// server, which names its last event, and the own call that this state shows. `asking`: this client
    /// asks that server for a place now, so another identity in the state is its own.
    pub(crate) fn fate(
        &mut self,
        state: Option<(Cursor, Option<&OwnCall>)>,
        asking: bool,
        now: Instant,
    ) -> Fate {
        if let Some((cursor, _)) = state
            && self.history != Some(cursor.epoch)
        {
            if self.history.is_some() {
                // Another history: what the earlier one told is void.
                self.shown = None;
                self.left = None;
            }
            self.history = Some(cursor.epoch);
            self.events.get_or_insert(cursor.epoch);
        }
        // The events told the place and then its end. This holds whatever a state shows,
        // also in the first seconds of a place.
        if self.shown.is_some()
            && let Some(reason) = self.ended()
        {
            return Fate::End(EndReason::Server(reason));
        }
        let Some((cursor, own)) = state else {
            return Fate::Undecided;
        };
        match own {
            Some(own) if own.identity == self.identity => {
                self.shown = self.shown.max(Some(cursor.position));
                Fate::Held
            }
            // The events of a join arrive after its answer. Until the server has shown the
            // place once, or the time for that has passed, a state without it decides
            // nothing.
            _ if self.shown.is_none() && now < self.since + self.settle => Fate::Undecided,
            Some(_) if asking => Fate::Undecided,
            Some(_) => Fate::End(EndReason::Elsewhere),
            None => Fate::End(EndReason::Server(self.cause())),
        }
    }

    /// The server gave a new token. With another identity the earlier place had ended, and
    /// the call continues at the new one. Returns whether the identity changed.
    pub(crate) fn renewed(&mut self, identity: &str, now: Instant) -> bool {
        if identity == self.identity {
            return false;
        }
        identity.clone_into(&mut self.identity);
        self.since = now;
        self.shown = None;
        self.left = None;
        true
    }
}

/// The waits between the requests for a new token.
#[derive(Debug)]
pub(crate) struct Backoff {
    first: Duration,
    longest: Duration,
    next: Duration,
}

impl Backoff {
    pub(crate) fn new(first: Duration, longest: Duration) -> Self {
        Self {
            first,
            longest,
            next: first,
        }
    }

    /// The wait before the next request. The one after it is twice as long, up to the
    /// longest.
    pub(crate) fn take(&mut self) -> Duration {
        let wait = self.next;
        self.next = (wait * 2).min(self.longest);
        wait
    }

    /// The media connection is made.
    pub(crate) fn reset(&mut self) {
        self.next = self.first;
    }
}

/// Items that arrive in an order, one at a time.
pub(crate) trait Source {
    type Item;

    /// The next item, or nothing when no further one comes.
    fn next(&mut self) -> impl Future<Output = Option<Self::Item>> + Send;
}

impl Source for Notices {
    type Item = Result<Notice, Missed>;

    fn next(&mut self) -> impl Future<Output = Option<Self::Item>> + Send {
        Notices::next(self)
    }
}

/// Reads items until `pick` finds a value in one, for at most `limit` in all, however
/// many items arrive.
pub(crate) async fn first_within<S: Source, T>(
    limit: Duration,
    items: &mut S,
    mut pick: impl FnMut(S::Item) -> Option<T>,
) -> Option<T> {
    let deadline = Instant::now() + limit;
    loop {
        match tokio::time::timeout_at(deadline, items.next()).await {
            Ok(Some(item)) => {
                if let Some(value) = pick(item) {
                    return Some(value);
                }
            }
            Ok(None) | Err(_) => return None,
        }
    }
}

/// How long the connection to the room of the destination of a move may take. A move
/// waits 30 seconds on the server (ADR 0511).
const MOVE_CONNECT_LIMIT: Duration = Duration::from_secs(15);
/// A report that failed is sent again after this time.
const REPORT_RETRY: Duration = Duration::from_secs(3);
/// How long the end of a call waits for the event that states its cause.
const CAUSE_WAIT: Duration = Duration::from_millis(500);

enum Done {
    State(bool),
    Speaking(bool),
    Share(bool),
    /// The answer to a request for a new token. The session confirms that it took the
    /// answer; an answer that no session takes gives a place that nobody uses.
    Rejoined(Box<Result<CallJoin, ApiError>>, oneshot::Sender<()>),
}

/// Asks the server for a new token for the place of a session, in its turn.
async fn renew(
    inner: Arc<Inner>,
    server: ServerHandle,
    target: CallTarget,
    done: mpsc::UnboundedSender<Done>,
) {
    if done.is_closed() {
        // The call ended while the request waited for its turn.
        return;
    }
    let answer = request(&server, &target.place, Entry::Open).await;
    let given = answer.as_ref().ok().map(|answer| answer.identity.clone());
    let (taken, confirmed) = oneshot::channel();
    let used =
        done.send(Done::Rejoined(Box::new(answer), taken)).is_ok() && confirmed.await.is_ok();
    if let Some(identity) = given.filter(|_| !used) {
        // The call ended while the server was asked. The place that it gave is given up
        // before the next piece of work asks a server.
        give_up(&inner, target.server_id, &identity).await;
    }
}

/// The work of one call from the answer of the server to its end.
struct Session {
    inner: Arc<Inner>,
    key: u64,
    server: ServerHandle,
    target: CallTarget,
    place: Place,
    /// What the place is by the last reading of the server.
    fate: Fate,
    /// The server is read again before the next step.
    review: bool,
    engine: Engine,
    controls: Controls,
    microphone_block: Option<MicrophoneBlock>,
    sound_block: Option<SoundBlock>,
    connected_once: bool,
    interrupted_at: Option<OffsetDateTime>,
    /// When the next request for a new token is due.
    retry: Option<Instant>,
    backoff: Backoff,
    rejoining: bool,
    state_report: Reporter<(bool, bool)>,
    speaking_report: Reporter<bool>,
    /// The sound of the share that the server was told. Absent: no share.
    share_report: Reporter<Option<ShareAudio>>,
    /// The move of the person of this device that waits (ADR 0511).
    moving: Option<Moving>,
    speaking: bool,
    timing: CallTiming,
}

/// A move of the person of this device that has no outcome yet.
struct Moving {
    call_move: CallMove,
    /// Ends the connection of this device to the room of the destination: the work that
    /// holds the connection closes it when this is sent or dropped.
    _stop: oneshot::Sender<()>,
    /// When the move is given up. The server ends a move after 30 seconds (ADR 0511); an
    /// outcome that has not arrived by this time was missed.
    deadline: Instant,
    /// The state of the server was replaced by a snapshot since the move began: the event
    /// with the outcome can be among the changes that were not told.
    reloaded: bool,
}

/// How long the outcome of a move is waited for.
const MOVE_OUTCOME_LIMIT: Duration = Duration::from_secs(45);

fn far() -> Instant {
    Instant::now() + Duration::from_secs(86_400)
}

impl Session {
    async fn run(mut self, mut commands: mpsc::UnboundedReceiver<SessionCommand>) {
        let mut engine_state = self.engine.state();
        let mut levels = self.engine.levels();
        let mut main_picture = self.engine.main_picture();
        let mut small_pictures = self.engine.small_pictures();
        let mut live = self.server.live().watch();
        let mut notices = self.server.live().notices();
        let (done, mut finished) = mpsc::unbounded_channel::<Done>();
        let (reason, ended) = loop {
            if std::mem::take(&mut self.review) {
                self.fate = self.read_server();
            }
            if let Fate::End(reason) = &self.fate {
                break (Some(reason.clone()), None);
            }
            let wake = self.work(Instant::now(), &done);
            tokio::select! {
                command = commands.recv() => match command {
                    Some(SessionCommand::End { done }) => break (None, Some(done)),
                    Some(command) => self.command(command),
                    None => break (None, None),
                },
                changed = engine_state.changed() => {
                    if changed.is_err() {
                        // The runtime ends.
                        break (None, None);
                    }
                    let state = engine_state.borrow_and_update().clone();
                    self.engine_changed(&state);
                }
                changed = levels.changed() => {
                    if changed.is_ok() {
                        let now = levels.borrow_and_update().clone();
                        self.speaking = now.own.speaking;
                        self.inner.levels.send_replace(now);
                    }
                }
                changed = main_picture.changed() => {
                    if changed.is_ok() {
                        let now = main_picture.borrow_and_update().clone();
                        self.inner.main_picture.send_replace(now);
                    }
                }
                changed = small_pictures.changed() => {
                    if changed.is_ok() {
                        let now = small_pictures.borrow_and_update().clone();
                        self.inner.small_pictures.send_replace(now);
                    }
                }
                view = live.changed() => {
                    if view.is_some() {
                        self.review = true;
                    }
                }
                notice = notices.next() => {
                    if let Some(Ok(notice)) = notice {
                        self.note(&notice);
                    }
                }
                Some(result) = finished.recv() => self.finished(result),
                () = tokio::time::sleep_until(wake) => self.review = true,
            }
        };
        // An answer to a request for a new token that arrives from now on is taken by
        // nobody, and who asked gives the place up.
        drop(finished);
        self.end_move();
        let Self {
            inner,
            key,
            target,
            engine,
            mut place,
            ..
        } = self;
        // The media connection and the devices close at once. The cause of the end is
        // told a moment later, when the event that states it has arrived.
        engine.close().await;
        inner.levels.send_replace(Levels::default());
        inner.main_picture.send_replace(None);
        inner.small_pictures.send_replace(HashMap::new());
        let reason = match reason {
            Some(EndReason::Server(None)) => {
                let cause = first_within(CAUSE_WAIT, &mut notices, |notice| {
                    if let Ok(Notice::Event { id, event, .. }) = notice {
                        place.note(id, &event);
                    }
                    place.cause()
                })
                .await;
                Some(EndReason::Server(cause))
            }
            other => other,
        };
        {
            let mut shared = inner.shared();
            if shared
                .active
                .as_ref()
                .is_some_and(|active| active.key == key)
            {
                shared.active = None;
            }
        }
        inner.view.send_modify(|view| {
            if view.call.as_ref().is_some_and(|call| call.target == target) {
                view.call = None;
            }
            if let Some(reason) = reason {
                view.outcome = Some(Outcome::Ended { target, reason });
            }
        });
        if let Some(ended) = ended {
            let _ = ended.send(place.identity);
        }
    }

    fn note(&mut self, notice: &Notice) {
        match notice {
            Notice::Event { id, event, .. } => {
                self.place.note(*id, event);
                self.note_move(event);
            }
            Notice::Reloaded { cursor } => {
                self.place.reloaded(cursor.epoch);
                if let Some(moving) = &mut self.moving {
                    moving.reloaded = true;
                }
            }
            Notice::Ended { .. } => {}
        }
        self.review = true;
    }

    /// A move of the person of this device (ADR 0511). The device joins the room of the
    /// destination with the token of the move and stays in the call of the source: the
    /// server completes the move when the media server lists the device there. After the
    /// completion the device enters the call of the destination as it enters any call,
    /// which ends the call of the source on this device.
    fn note_move(&mut self, event: &Event) {
        match event {
            Event::CallMoveRequested { call_move }
                if Some(call_move.account_id) == self.server.account()
                    && call_move.from == self.target.place =>
            {
                self.end_move();
                let (api, move_id) = (self.server.api(), call_move.id);
                let (stop, stopped) = oneshot::channel::<()>();
                self.inner.runtime.spawn(async move {
                    let connected = async {
                        let answer = api.join_move(move_id).await.ok()?;
                        let connecting = Room::connect(
                            &answer.media_url,
                            answer.token.expose(),
                            RoomOptions::default(),
                        );
                        tokio::time::timeout(MOVE_CONNECT_LIMIT, connecting)
                            .await
                            .ok()?
                            .ok()
                    };
                    match connected.await {
                        // The connection is held until the move has its outcome or is
                        // given up, and is closed then. A connection that is only
                        // dropped stays in the room of the destination.
                        Some((room, _events)) => {
                            let _ = stopped.await;
                            if let Err(error) = room.close().await {
                                tracing::debug!(%error, "the connection of a move was not closed");
                            }
                        }
                        None => {
                            if let Err(error) = api.fail_move(move_id).await {
                                tracing::debug!(%error, "the failure of a move was not told");
                            }
                        }
                    }
                });
                self.moving = Some(Moving {
                    call_move: call_move.clone(),
                    _stop: stop,
                    deadline: Instant::now() + MOVE_OUTCOME_LIMIT,
                    reloaded: false,
                });
            }
            Event::CallMoveResolved { move_id, outcome } => self.move_resolved(*move_id, *outcome),
            _ => {}
        }
    }

    fn move_resolved(&mut self, move_id: CallMoveId, outcome: MoveOutcome) {
        let Some(moving) = self
            .moving
            .as_ref()
            .filter(|moving| moving.call_move.id == move_id)
        else {
            return;
        };
        let to = moving.call_move.to;
        self.end_move();
        if outcome == MoveOutcome::Completed {
            let target = CallTarget {
                server_id: self.target.server_id,
                place: to,
            };
            // The join runs to its end by itself, and ends this call when it has its place.
            if let Err(error) = ask(&self.inner, target, Entry::Moved) {
                tracing::warn!(%error, "the call of the destination of a move was not entered");
            }
        }
    }

    /// Ends the move that waits: the work that holds the connection to the room of its
    /// destination closes that connection.
    fn end_move(&mut self) {
        self.moving = None;
    }

    /// Decides a move whose outcome was not told. After a snapshot replaced the state, a
    /// move that the state no longer holds has ended: the person is in the call of the
    /// destination, or the move had no effect. A move without an outcome at its time
    /// limit is given up.
    fn review_move(&mut self, model: Option<&ServerModel>) {
        let Some(moving) = &self.moving else {
            return;
        };
        let (move_id, to) = (moving.call_move.id, moving.call_move.to);
        if let Some(model) = model.filter(|_| moving.reloaded)
            && !model.moves.iter().any(|held| held.id == move_id)
        {
            let completed = model.own_call.as_ref().is_some_and(|own| own.place == to);
            self.move_resolved(
                move_id,
                if completed {
                    MoveOutcome::Completed
                } else {
                    MoveOutcome::Abandoned
                },
            );
        } else if Instant::now() >= moving.deadline {
            self.end_move();
        }
    }

    fn command(&mut self, command: SessionCommand) {
        match command {
            SessionCommand::Controls => self.apply_controls(),
            SessionCommand::Talking(talking) => self.engine.set_talking(talking),
            SessionCommand::Volume(account_id, gain) => self.engine.set_volume(account_id, gain),
            SessionCommand::Config(config) => self.engine.set_config(config),
            SessionCommand::RetryMicrophone => self.engine.retry_microphone(),
            SessionCommand::StartShare(request) => self.engine.start_share(request),
            SessionCommand::StopShare => self.engine.stop_share(),
            SessionCommand::Watch(choice) => self.engine.watch(choice),
            SessionCommand::ShareVolume(gain) => self.engine.set_share_volume(gain),
            SessionCommand::ViewSize(width, height) => self.engine.set_view_size(width, height),
            #[cfg(feature = "test-support")]
            SessionCommand::Statistics(reply) => self.engine.statistics(reply),
            #[cfg(feature = "test-support")]
            SessionCommand::SoundCheck(reply) => self.engine.sound_check(reply),
            #[cfg(feature = "test-support")]
            SessionCommand::SimulateFullReconnect => self.engine.simulate_full_reconnect(),
            // The loop takes this command itself.
            SessionCommand::End { .. } => {}
        }
    }

    fn apply_controls(&mut self) {
        let (muted, deafened) = {
            let shared = self.inner.shared();
            (shared.muted, shared.deafened)
        };
        let controls = Controls {
            muted,
            deafened,
            microphone_blocked: self.microphone_block.is_some(),
            sound_blocked: self.sound_block.is_some(),
        };
        if controls != self.controls {
            self.controls = controls;
            self.engine.set_controls(controls);
        }
    }

    /// Reads from the state of the server what concerns this call: whether the place of
    /// this device still exists, and the blocks of the person.
    fn read_server(&mut self) -> Fate {
        let view = self.server.live().view();
        // Without a current state the place can still exist.
        let model = view
            .model
            .as_deref()
            .filter(|_| view.status == StreamStatus::Live);
        self.review_move(model);
        // This client asks the server for a place now: a join of the person, which ends
        // this call when it has its answer, or a new token for this call.
        let asking = self.rejoining
            || self.moving.is_some()
            || self
                .inner
                .shared()
                .joining
                .is_some_and(|joining| joining.target.server_id == self.target.server_id);
        let state = model.map(|model| (model.cursor, model.own_call.as_ref()));
        let fate = self.place.fate(state, asking, Instant::now());
        if let (Fate::Held, Some(model)) = (&fate, model) {
            self.read_blocks(model);
        }
        fate
    }

    fn read_blocks(&mut self, model: &ServerModel) {
        // The state lists the person when the media connection is made. Until then the
        // blocks are the ones that the server gave with the place.
        let Some(own) = model
            .call_at(&self.target.place)
            .and_then(|call| call.participant(model.account.id))
        else {
            return;
        };
        let blocks = (own.microphone_block.clone(), own.sound_block.clone());
        let hand_raised = own.hand_raised;
        self.take_blocks(blocks, Some(model), hand_raised);
    }

    /// Uses the blocks that the server tells, from its state or with a new token.
    ///
    /// While another person runs meeting mode, the end of the microphone block of the
    /// person does not open the microphone: the person's own setting becomes "closed" in
    /// the same step, and the person opens it (ADR 0505, ADR 0739). The hand of the
    /// person is lowered. This holds for every block and whoever removed it, because the
    /// state tells no one who removed a block.
    fn take_blocks(
        &mut self,
        blocks: (Option<MicrophoneBlock>, Option<SoundBlock>),
        model: Option<&ServerModel>,
        hand_raised: bool,
    ) {
        let run_by_another = model.is_some_and(|model| {
            model
                .call_at(&self.target.place)
                .and_then(|call| call.meeting.as_ref())
                .is_some_and(|meeting| meeting.presenter != model.account.id)
        });
        if self.microphone_block.is_some() && blocks.0.is_none() && run_by_another {
            self.inner.shared().muted = true;
            self.inner.view.send_modify(|view| view.muted = true);
            if hand_raised {
                let api = self.server.api();
                self.inner.runtime.spawn(async move {
                    if let Err(error) = api.lower_hand().await {
                        tracing::debug!(%error, "the hand was not lowered");
                    }
                });
            }
        }
        self.set_blocks(blocks);
    }

    fn set_blocks(&mut self, blocks: (Option<MicrophoneBlock>, Option<SoundBlock>)) {
        if blocks != (self.microphone_block.clone(), self.sound_block.clone()) {
            (self.microphone_block, self.sound_block) = blocks;
            self.apply_controls();
            self.show();
        }
    }

    fn engine_changed(&mut self, state: &EngineState) {
        match state.connection {
            Connection::Connected => {
                self.connected_once = true;
                self.interrupted_at = None;
                self.retry = None;
                self.backoff.reset();
            }
            Connection::Connecting => {}
            Connection::Reconnecting => {
                self.interrupted_at
                    .get_or_insert_with(OffsetDateTime::now_utc);
            }
            Connection::Lost(_) => {
                self.interrupted_at
                    .get_or_insert_with(OffsetDateTime::now_utc);
                if self.retry.is_none() && !self.rejoining {
                    self.retry = Some(Instant::now() + self.backoff.take());
                }
            }
        }
        self.show_with(state);
    }

    fn finished(&mut self, result: Done) {
        let now = Instant::now();
        match result {
            Done::State(taken) => self.state_report.done(taken, REPORT_RETRY, now),
            Done::Speaking(taken) => self.speaking_report.done(taken, REPORT_RETRY, now),
            Done::Share(taken) => self.share_report.done(taken, REPORT_RETRY, now),
            Done::Rejoined(answer, taken) => {
                let _ = taken.send(());
                self.rejoining = false;
                self.review = true;
                match *answer {
                    Ok(answer) => {
                        if self.place.renewed(&answer.identity, now) {
                            // The server gave a new place: the earlier one had ended.
                            self.state_report = Reporter::new(self.timing.state_spacing, None);
                            self.share_report = Reporter::holding(self.timing.share_spacing, None);
                            self.show();
                        }
                        self.engine.reconnect(media_join(&answer));
                        // The blocks of the answer are newer than the state when events
                        // were missed: the same rule applies to them.
                        let view = self.server.live().view();
                        let hand_raised = view.model.as_deref().is_some_and(|model| {
                            model
                                .call_at(&self.target.place)
                                .and_then(|call| call.participant(model.account.id))
                                .is_some_and(|own| own.hand_raised)
                        });
                        self.take_blocks(
                            (answer.microphone_block, answer.sound_block),
                            view.model.as_deref(),
                            hand_raised,
                        );
                    }
                    Err(error) => {
                        let passes = error.is_unavailable()
                            || error.category() == Some(ErrorCategory::Limited)
                            || matches!(error, ApiError::NotLoggedIn);
                        if passes {
                            let wait = error.retry_after().unwrap_or_else(|| self.backoff.take());
                            self.retry = Some(now + wait);
                        } else {
                            self.fate = Fate::End(EndReason::Refused(refusal(&error)));
                            self.review = false;
                        }
                    }
                }
            }
        }
    }

    /// Starts what is due now and returns when the next thing is due.
    fn work(&mut self, now: Instant, done: &mpsc::UnboundedSender<Done>) -> Instant {
        let mut wake = far();
        if let Some(moving) = &self.moving {
            wake = wake.min(moving.deadline);
        }
        if let Some(at) = self.place.settles_at()
            && at > now
        {
            wake = wake.min(at);
        }
        if let Some(at) = self.retry
            && !self.rejoining
        {
            if now < at {
                wake = wake.min(at);
            } else if self.fate == Fate::Held {
                // A new token is asked for only while the server shows the place of this
                // device. A place that the server has ended is not asked for again: a
                // person whom a moderator removed stays out.
                self.retry = None;
                self.rejoining = true;
                drop(self.inner.in_turn(renew(
                    self.inner.clone(),
                    self.server.clone(),
                    self.target,
                    done.clone(),
                )));
            }
        }
        let state = self.engine.state().borrow().clone();
        if state.connection != Connection::Connected {
            return wake;
        }
        // A person whose microphone cannot be used is told to the others as muted.
        let silent = self.controls.muted
            || !matches!(
                state.microphone,
                MicrophoneState::Running | MicrophoneState::Closed
            );
        let wanted = (silent, self.controls.deafened);
        match self.state_report.step(&wanted, false, now) {
            Step::Send => {
                self.state_report.sending(wanted, now);
                let (api, done) = (self.server.api(), done.clone());
                self.inner.runtime.spawn(async move {
                    let taken = api.report_call_state(wanted.0, wanted.1).await.is_ok();
                    let _ = done.send(Done::State(taken));
                });
            }
            Step::Wait(at) => wake = wake.min(at),
            Step::Idle => {}
        }
        // The server shows a share from this report, and only while the media server lists
        // its picture track (ADR 0510). The share is reported when its track is published.
        let shared = match &state.share {
            ShareState::Running(info) => Some(info.sound),
            ShareState::Off | ShareState::Starting | ShareState::Ended(_) => None,
        };
        match self.share_report.step(&shared, false, now) {
            Step::Send => {
                self.share_report.sending(shared, now);
                let (api, done) = (self.server.api(), done.clone());
                self.inner.runtime.spawn(async move {
                    let told = match shared {
                        Some(audio) => api.report_screen_share(audio).await,
                        None => api.end_screen_share().await,
                    };
                    let _ = done.send(Done::Share(told.is_ok()));
                });
            }
            Step::Wait(at) => wake = wake.min(at),
            Step::Idle => {}
        }
        if matches!(self.target.place, CallPlace::Channel { .. }) {
            let speaking = self.speaking;
            match self.speaking_report.step(&speaking, speaking, now) {
                Step::Send => {
                    self.speaking_report.sending(speaking, now);
                    let (api, done) = (self.server.api(), done.clone());
                    self.inner.runtime.spawn(async move {
                        let taken = api.report_speaking(speaking).await.is_ok();
                        let _ = done.send(Done::Speaking(taken));
                    });
                }
                Step::Wait(at) => wake = wake.min(at),
                Step::Idle => {}
            }
        }
        wake
    }

    fn show(&self) {
        let state = self.engine.state().borrow().clone();
        self.show_with(&state);
    }

    fn show_with(&self, state: &EngineState) {
        let phase = match state.connection {
            Connection::Connected => Phase::Connected,
            Connection::Connecting if !self.connected_once && self.interrupted_at.is_none() => {
                Phase::Connecting
            }
            Connection::Connecting | Connection::Reconnecting | Connection::Lost(_) => {
                Phase::Reconnecting
            }
        };
        let call = ActiveCall {
            target: self.target,
            identity: self.place.identity().to_owned(),
            phase,
            interrupted_at: self.interrupted_at.filter(|_| phase == Phase::Reconnecting),
            microphone: state.microphone.clone(),
            access: state.access,
            input_device: state.input_device.clone(),
            output: state.output.clone(),
            output_device: state.output_device.clone(),
            microphone_block: self.microphone_block.clone(),
            sound_block: self.sound_block.clone(),
            share: state.share.clone(),
            shares: state.shares.clone(),
            watched: state.watched,
        };
        let key = self.key;
        let current = self
            .inner
            .shared()
            .active
            .as_ref()
            .is_some_and(|active| active.key == key);
        if !current {
            return;
        }
        self.inner.view.send_if_modified(|view| {
            if view.call.as_ref() == Some(&call) {
                false
            } else {
                view.call = Some(call);
                true
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(target_os = "linux")]
    #[allow(clippy::unwrap_used)]
    fn source_discovery_polled_by_a_window_uses_the_client_runtime() {
        struct RuntimeCapture;
        impl ScreenCapture for RuntimeCapture {
            fn access(&self) -> ScreenAccess {
                ScreenAccess::AskOnStart
            }
            fn request_access(&self) {}
            fn open_settings(&self) {}
            fn sources(&self) -> Pending<Result<ShareSources, CaptureError>> {
                Box::pin(async {
                    if tokio::runtime::Handle::try_current().is_err() {
                        return Err(CaptureError::Failed(
                            "source discovery has no client runtime".into(),
                        ));
                    }
                    tokio::time::sleep(Duration::from_millis(1)).await;
                    Ok(ShareSources::default())
                })
            }
            fn start(
                &self,
                _: CaptureRequest,
                _: Arc<dyn crate::share::CaptureSink>,
            ) -> Pending<Result<crate::share::CaptureStream, CaptureError>> {
                Box::pin(async { Err(CaptureError::Unsupported) })
            }
        }
        assert!(tokio::runtime::Handle::try_current().is_err());
        let directory = tempfile::Builder::new()
            .prefix("junto-linux-source-runtime-")
            .tempdir_in("/tmp")
            .unwrap();
        let client = Client::open(junto_client_core::ClientOptions::new(
            directory.path().join("client"),
        ))
        .unwrap();
        let calls = Calls::new(
            client,
            MediaOptions {
                host: Arc::new(crate::audio::synthetic::SyntheticHost::with_tone(440.0)),
                permission: Arc::new(crate::permission::SystemPermission),
                capture: Arc::new(RuntimeCapture),
                timing: CallTiming::default(),
            },
        );
        let mut reading = calls.share_sources();
        let mut context = std::task::Context::from_waker(std::task::Waker::noop());
        let started = std::time::Instant::now();
        loop {
            if let std::task::Poll::Ready(result) = reading.as_mut().poll(&mut context) {
                assert_eq!(result, Ok(ShareSources::default()));
                break;
            }
            assert!(
                started.elapsed() < Duration::from_secs(10),
                "source discovery did not finish"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_changed_value_is_sent_at_once_and_never_closer_than_the_spacing() {
        let spacing = Duration::from_millis(500);
        let mut reporter: Reporter<bool> = Reporter::new(spacing, None);
        let start = Instant::now();
        assert_eq!(reporter.step(&false, false, start), Step::Send);
        reporter.sending(false, start);
        // One request at a time.
        assert_eq!(reporter.step(&true, false, start), Step::Idle);
        reporter.done(true, REPORT_RETRY, start);
        assert_eq!(reporter.step(&false, false, start), Step::Idle);
        assert_eq!(
            reporter.step(&true, false, start),
            Step::Wait(start + spacing)
        );
        let later = start + spacing;
        assert_eq!(reporter.step(&true, false, later), Step::Send);
        reporter.sending(true, later);
        reporter.done(true, REPORT_RETRY, later);
        assert_eq!(
            reporter.step(&true, false, later + spacing * 10),
            Step::Idle
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_value_that_the_server_forgets_is_renewed_and_a_failed_report_is_sent_again() {
        let (spacing, renewal) = (Duration::from_millis(700), Duration::from_secs(3));
        let mut reporter: Reporter<bool> = Reporter::new(spacing, Some(renewal));
        let start = Instant::now();
        reporter.sending(true, start);
        reporter.done(true, REPORT_RETRY, start);
        // While the person speaks, the report is renewed before the server drops it.
        assert_eq!(
            reporter.step(&true, true, start),
            Step::Wait(start + renewal)
        );
        assert_eq!(reporter.step(&true, true, start + renewal), Step::Send);
        reporter.sending(true, start + renewal);
        reporter.done(true, REPORT_RETRY, start + renewal);
        // The end of speaking is a change and is not renewed.
        let end = start + renewal + spacing;
        assert_eq!(reporter.step(&false, false, end), Step::Send);
        reporter.sending(false, end);
        reporter.done(true, REPORT_RETRY, end);
        assert_eq!(reporter.step(&false, false, end + renewal * 2), Step::Idle);

        // A report that the server did not take is sent again after the retry time.
        let mut reporter: Reporter<bool> = Reporter::new(spacing, None);
        reporter.sending(true, start);
        reporter.done(false, REPORT_RETRY, start);
        assert_eq!(
            reporter.step(&true, false, start),
            Step::Wait(start + REPORT_RETRY)
        );
        assert_eq!(
            reporter.step(&true, false, start + REPORT_RETRY),
            Step::Send
        );
    }

    #[test]
    fn at_most_the_reports_that_the_server_takes_are_sent() {
        // 30 reports of the state in 10 seconds (ADR 0510), 20 of speaking (ADR 0513). The
        // reports of the own state and of the share count against one limit.
        let timing = CallTiming::default();
        let in_10_seconds =
            |spacing: Duration| Duration::from_secs(10).as_millis() / spacing.as_millis();
        assert!(in_10_seconds(timing.state_spacing) + in_10_seconds(timing.share_spacing) <= 30);
        assert!(Duration::from_secs(10).as_millis() / timing.speaking_spacing.as_millis() <= 20);
        assert!(timing.speaking_renewal < Duration::from_secs(5));
    }

    fn own(identity: &str) -> OwnCall {
        OwnCall {
            call_id: junto_client_core::protocol::ids::CallId::generate(),
            place: CallPlace::direct(AccountId::generate(), AccountId::generate()),
            device_id: junto_client_core::protocol::ids::DeviceId::generate(),
            identity: identity.to_owned(),
        }
    }

    fn changed(call: Option<OwnCall>, left: Option<LeaveReason>) -> Event {
        Event::OwnCallChanged { call, left }
    }

    const SETTLE: Duration = Duration::from_secs(10);

    thread_local! {
        /// The history of a test.
        static HISTORY: StreamEpoch = StreamEpoch::generate();
    }

    /// A current state of the server at an event of the history of the test.
    fn at(position: u64, own: Option<&OwnCall>) -> Option<(Cursor, Option<&OwnCall>)> {
        let epoch = HISTORY.with(|history| *history);
        Some((Cursor { epoch, position }, own))
    }

    fn ended(reason: Option<LeaveReason>) -> Fate {
        Fate::End(EndReason::Server(reason))
    }

    #[tokio::test(start_paused = true)]
    async fn a_place_that_the_state_does_not_show_yet_is_kept_until_the_settle_time() {
        let start = Instant::now();
        let mut place = Place::new("ours".to_owned(), start, SETTLE);
        assert_eq!(place.settles_at(), Some(start + SETTLE));
        // The state is older than the join: no call, or the call that the join replaced.
        assert_eq!(place.fate(at(4, None), false, start), Fate::Undecided);
        let before = own("before");
        assert_eq!(
            place.fate(at(4, Some(&before)), false, start),
            Fate::Undecided
        );
        // Without a current state nothing is decided at any time.
        assert_eq!(place.fate(None, false, start + SETTLE), Fate::Undecided);
        // After the settle time a state without the place is the end of the place.
        let late = start + SETTLE;
        assert_eq!(place.fate(at(4, None), false, late), ended(None));
        assert_eq!(
            place.fate(at(4, Some(&before)), false, late),
            Fate::End(EndReason::Elsewhere)
        );
        // A state that shows the place ends the wait.
        let mut place = Place::new("ours".to_owned(), start, SETTLE);
        let ours = own("ours");
        assert_eq!(place.fate(at(5, Some(&ours)), false, start), Fate::Held);
        assert_eq!(place.settles_at(), None);
        assert_eq!(place.fate(at(6, None), false, start), ended(None));
    }

    #[tokio::test(start_paused = true)]
    async fn a_removal_that_follows_the_join_at_once_ends_the_place_inside_the_settle_time() {
        // The state can pass from "no call" to "no call" without ever showing the place:
        // the join and the removal by a moderator arrive as one change. The events still
        // tell both.
        let start = Instant::now();
        let mut place = Place::new("ours".to_owned(), start, SETTLE);
        assert_eq!(place.fate(at(7, None), false, start), Fate::Undecided);
        place.note(6, &changed(Some(own("ours")), None));
        place.note(7, &changed(None, Some(LeaveReason::Disconnected)));
        let removed = ended(Some(LeaveReason::Disconnected));
        assert_eq!(place.fate(at(7, None), false, start), removed);
        // The events decide without a current state too.
        assert_eq!(place.fate(None, false, start), removed);
        assert_eq!(place.cause(), Some(LeaveReason::Disconnected));

        // The leave of the call before this one is no end of this one: its event comes
        // before the event of the join.
        let mut place = Place::new("ours".to_owned(), start, SETTLE);
        place.note(4, &changed(None, Some(LeaveReason::Switched)));
        assert_eq!(place.fate(at(4, None), false, start), Fate::Undecided);
        place.note(5, &changed(Some(own("ours")), None));
        let ours = own("ours");
        assert_eq!(place.fate(at(5, Some(&ours)), false, start), Fate::Held);
        // The end of this place, before its event is read: the cause is not known yet.
        assert_eq!(place.fate(at(6, None), false, start), ended(None));
        assert_eq!(place.cause(), None);
        place.note(6, &changed(None, Some(LeaveReason::ConnectionLost)));
        assert_eq!(place.cause(), Some(LeaveReason::ConnectionLost));
    }

    #[tokio::test(start_paused = true)]
    async fn an_event_from_before_the_state_that_shows_the_place_does_not_end_it() {
        // The session reads the state and the events from two sources. The state is ahead:
        // it shows the place at event 9, and the events up to 9 are read after that.
        let start = Instant::now();
        let mut place = Place::new("ours".to_owned(), start, SETTLE);
        let ours = own("ours");
        assert_eq!(place.fate(at(9, Some(&ours)), false, start), Fate::Held);
        place.note(4, &changed(None, Some(LeaveReason::Switched)));
        place.note(5, &changed(Some(own("ours")), None));
        assert_eq!(place.fate(at(9, Some(&ours)), false, start), Fate::Held);
        assert_eq!(place.fate(None, false, start), Fate::Undecided);
        // An event after that state ends the place.
        place.note(10, &changed(None, Some(LeaveReason::Disconnected)));
        assert_eq!(
            place.fate(at(9, Some(&ours)), false, start),
            ended(Some(LeaveReason::Disconnected))
        );
    }

    #[tokio::test(start_paused = true)]
    async fn the_events_of_a_history_before_a_restore_do_not_end_the_place() {
        // The place is shown at event 100. The server is restored to an earlier time: its
        // state has another history, is at event 50, and shows the place.
        let start = Instant::now();
        let mut place = Place::new("ours".to_owned(), start, SETTLE);
        let ours = own("ours");
        assert_eq!(place.fate(at(100, Some(&ours)), false, start), Fate::Held);
        let restored = Cursor {
            epoch: StreamEpoch::generate(),
            position: 50,
        };
        let state = Some((restored, Some(&ours)));
        assert_eq!(place.fate(state, false, start), Fate::Held);
        // An event of the earlier history is read after that: a removal that the
        // restored server does not hold.
        place.note(101, &changed(None, Some(LeaveReason::Disconnected)));
        assert_eq!(place.fate(state, false, start), Fate::Held);
        assert_eq!(place.fate(None, false, start), Fate::Undecided);
        // The events of the new history follow the notice of the reload, and count.
        place.reloaded(restored.epoch);
        place.note(51, &changed(None, Some(LeaveReason::Disconnected)));
        assert_eq!(
            place.fate(state, false, start),
            ended(Some(LeaveReason::Disconnected))
        );
    }

    #[tokio::test(start_paused = true)]
    async fn another_identity_is_no_other_device_while_this_client_asks_for_a_place() {
        let start = Instant::now();
        let mut place = Place::new("ours".to_owned(), start, SETTLE);
        let (ours, renewed, other) = (own("ours"), own("renewed"), own("other"));
        assert_eq!(place.fate(at(5, Some(&ours)), false, start), Fate::Held);
        // A new token is asked for, and the state shows its identity before the answer of
        // the request is read.
        assert_eq!(
            place.fate(at(6, Some(&renewed)), true, start),
            Fate::Undecided
        );
        assert!(place.renewed("renewed", start));
        assert_eq!(place.identity(), "renewed");
        assert_eq!(place.fate(at(6, Some(&renewed)), false, start), Fate::Held);
        assert!(!place.renewed("renewed", start));
        // Without a request of this client, another identity is another device.
        assert_eq!(
            place.fate(at(7, Some(&other)), false, start),
            Fate::End(EndReason::Elsewhere)
        );
    }

    #[test]
    fn the_wait_for_a_new_token_doubles_up_to_its_longest_and_starts_again_after_a_connection() {
        let mut backoff = Backoff::new(Duration::from_secs(1), Duration::from_secs(5));
        let waits: Vec<u64> = (0..5).map(|_| backoff.take().as_secs()).collect();
        assert_eq!(waits, [1, 2, 4, 5, 5]);
        backoff.reset();
        assert_eq!(backoff.take(), Duration::from_secs(1));
    }

    /// An item every 100 ms, without an end: 1, 2, 3 and so on.
    struct Steady(u32);

    impl Source for Steady {
        type Item = u32;

        async fn next(&mut self) -> Option<u32> {
            tokio::time::sleep(Duration::from_millis(100)).await;
            self.0 += 1;
            Some(self.0)
        }
    }

    struct Ended;

    impl Source for Ended {
        type Item = u32;

        async fn next(&mut self) -> Option<u32> {
            None
        }
    }

    #[tokio::test(start_paused = true)]
    async fn the_wait_for_a_value_has_one_bound_however_many_other_items_arrive() {
        let limit = Duration::from_millis(500);
        // No item holds the value, and the items never stop.
        let start = Instant::now();
        let mut items = Steady(0);
        let waiting = first_within(limit, &mut items, |_| None::<u8>);
        let found = tokio::time::timeout(Duration::from_secs(60), waiting).await;
        assert_eq!(found, Ok(None));
        assert_eq!(start.elapsed(), limit);

        let mut items = Steady(0);
        let found = first_within(limit, &mut items, |item| (item == 3).then_some(7u8));
        assert_eq!(found.await, Some(7));
        assert_eq!(first_within(limit, &mut Ended, Some).await, None);
    }
}
