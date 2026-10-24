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
use junto_client_core::book::Membership;
use junto_client_core::flow::FlowError;
use junto_client_core::live::{Missed, Notice, Notices, StreamStatus};
use junto_client_core::model::ServerModel;
use junto_client_core::protocol::call::{
    Call, CallJoin, CallMove, CallPlace, LeaveReason, MicrophoneBlock, MoveOutcome, OwnCall,
    ShareAudio, SoundBlock,
};
use junto_client_core::protocol::error::ErrorCode;
use junto_client_core::protocol::events::{Cursor, Event};
use junto_client_core::protocol::ids::{
    AccountId, CallId, CallInvitationId, CallMoveId, DeviceId, ServerId, StreamEpoch,
};
use junto_client_core::{Client, ServerHandle};
use livekit::prelude::{Room, RoomOptions};
use time::OffsetDateTime;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::time::Instant;

use crate::audio::capture::CaptureLink;
use crate::audio::{AudioError, AudioHost, DeviceInfo, Devices};
use crate::engine::{
    Connection, Controls, Engine, EngineOptions, EngineState, Levels, Lost, MainPicture, MediaJoin,
    MicrophoneState, OutputState, SeenShare, ShareId, ShareState, VoiceConfig, Watching,
};
use crate::permission::{MicrophoneAccess, MicrophonePermission};
use crate::share::picture::VideoPicture;
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
    /// How long the end of a call waits for the work of a move to close its connection to
    /// the room of the destination.
    pub move_end: Duration,
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
            move_end: Duration::from_secs(20),
        }
    }
}

/// The sound devices, the permission, the screen capture and the times that the calls of a
/// client use.
#[derive(Clone)]
pub struct MediaOptions {
    pub camera: Arc<dyn crate::camera::CameraCapture>,
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
            camera: crate::camera::system(),
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
    /// The media is connected; the server has not listed this place as inside yet.
    Entering,
    Connected,
    /// The media connection is interrupted or lost, and the client restores it.
    Reconnecting,
}

fn connection_phase(
    connection: Connection,
    connected_once: bool,
    interrupted: bool,
    admitted: bool,
) -> Phase {
    match connection {
        Connection::Connected if !admitted => Phase::Entering,
        Connection::Connected => Phase::Connected,
        Connection::Connecting if !connected_once && !interrupted => Phase::Connecting,
        Connection::Connecting | Connection::Reconnecting | Connection::Lost(_) => {
            Phase::Reconnecting
        }
    }
}

/// A call and the seat of this device in it. An action that opens a microphone, a camera
/// or a share names the seat that the person saw when the action was made (ADR 0788).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Seat {
    pub target: CallTarget,
    /// The identity of the device in the room (ADR 0508).
    pub identity: String,
}

/// An action that had no effect, because the device was in another call, or at another
/// seat, than the one that the action named.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unused {
    Microphone,
    Camera,
    Share,
}

/// The call that this device is in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveCall {
    pub camera: crate::camera::CameraState,
    pub cameras: Vec<AccountId>,
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

impl ActiveCall {
    pub fn seat(&self) -> Seat {
        Seat {
            target: self.target,
            identity: self.identity.clone(),
        }
    }
}

/// Why the calls closed the microphone setting of the person without an action of the
/// person.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MicrophoneClosed {
    /// A microphone block ended, and the device could not read how: the events of that
    /// time were not delivered, or the block had no position among them. A presenter of
    /// meeting mode can have let the person speak, and then the person opens the
    /// microphone (ADR 0785).
    EndOfBlockNotRead,
    /// The microphone runs on another device than the person used: the device of the
    /// call failed or was removed (ADR 0794).
    DeviceChanged,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EndReason {
    /// The server ended the place. The reason is absent when the client did not receive
    /// the event that states it.
    Server(Option<LeaveReason>),
    /// A request for a new token gave another place, and the device could not read why
    /// the earlier one had ended: the events of that time were not delivered. The device
    /// gave the new place up (ADR 0785).
    NotRead,
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
    /// The last action that had no effect because it named another call or seat. It stays
    /// until [`Calls::dismiss_unused`].
    pub unused: Option<Unused>,
    /// Why the calls closed the microphone setting last. It stays until
    /// [`Calls::dismiss_closed`].
    pub closed: Option<MicrophoneClosed>,
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
    StartCamera(Seat, Option<String>),
    StopCamera,
    Controls,
    Muted(bool),
    /// The person opens the microphone at this seat. The number is the count of closes by
    /// the person at the time of the action.
    Unmute(Seat, u64),
    /// The talk key, with the count of closes of the microphone at the press or the
    /// release.
    Talking(bool, u64),
    Volume(AccountId, f32),
    Config(VoiceConfig),
    RetryMicrophone(Seat),
    StartShare(Seat, CaptureRequest),
    StopShare,
    Watch(Option<ShareId>),
    ShareVolume(f32),
    ViewSize(u32, u32),
    CameraViews(HashMap<ShareId, (u32, u32)>),
    #[cfg(feature = "test-support")]
    SimulateFullReconnect,
    #[cfg(feature = "test-support")]
    SimulateConnectionLoss,
    #[cfg(feature = "test-support")]
    Statistics(crate::engine::statistics::Reply),
    #[cfg(feature = "test-support")]
    SoundCheck(crate::engine::statistics::SoundReply),
    #[cfg(feature = "test-support")]
    HoldEngine(oneshot::Sender<(oneshot::Receiver<()>, crate::engine::HeldEngine)>),
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
    /// The gate of the microphone of the engine of the call.
    gate: Arc<CaptureLink>,
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
    camera_views: Option<HashMap<ShareId, (u32, u32)>>,
    joining: Option<Joining>,
    next_key: u64,
    active: Option<ActiveHandle>,
    /// How often the person closed the microphone. An open that was asked for before the
    /// last close has no effect: a close does not wait for an open that is on its way.
    closes: u64,
}

/// One piece of work that asks a server for a place, or gives one up.
type Turn = Pin<Box<dyn Future<Output = ()> + Send + 'static>>;

struct Inner {
    camera_pictures: watch::Sender<HashMap<ShareId, Arc<VideoPicture>>>,
    client: Client,
    runtime: tokio::runtime::Handle,
    options: MediaOptions,
    shared: Mutex<Shared>,
    view: watch::Sender<CallsView>,
    levels: watch::Sender<Levels>,
    main_picture: watch::Sender<Option<MainPicture>>,
    small_pictures: watch::Sender<HashMap<ShareId, Arc<VideoPicture>>>,
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

    fn set_muted(&self, muted: bool) {
        {
            let mut shared = self.shared();
            shared.muted = muted;
            if muted {
                shared.closes += 1;
                // The frames stop here, before the session and the engine take the close.
                if let Some(active) = &shared.active {
                    active.gate.stop_now();
                }
            }
        }
        self.view.send_modify(|view| view.muted = muted);
        self.tell(SessionCommand::Muted(muted));
    }

    /// An action named another call or seat than the one of this device, and did nothing.
    fn unused(&self, action: Unused) {
        self.view.send_modify(|view| view.unused = Some(action));
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

/// The calls of one client. Every clone is the same object. Any executor can poll the
/// futures that it returns (ADR 0733).
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
                camera_pictures: watch::Sender::new(HashMap::new()),
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

    /// The seat of this device in its call, as the view shows it now.
    pub fn seat(&self) -> Option<Seat> {
        self.inner.view.borrow().call.as_ref().map(ActiveCall::seat)
    }

    /// Closes the microphone of the person. The setting holds in a call and outside one.
    pub fn mute(&self) {
        self.inner.set_muted(true);
    }

    /// Opens the microphone of the person for the seat that the person saw when the action
    /// was made: `None` when the device was in no call, and the setting is then the one of
    /// the next call. When the device is in another call or at another seat by now, the
    /// microphone stays closed, and the view tells it (ADR 0788). Without a call nothing is
    /// told: the end of the call was.
    pub fn unmute_in(&self, seat: Option<&Seat>) {
        let mut shared = self.inner.shared();
        let session = shared.active.as_ref().map(|active| active.commands.clone());
        match (session, seat) {
            (Some(session), Some(seat)) => {
                let _ = session.send(SessionCommand::Unmute(seat.clone(), shared.closes));
            }
            (Some(_), None) => {
                drop(shared);
                self.inner.unused(Unused::Microphone);
            }
            // The lock is held: a call that starts now reads the setting after this.
            (None, None) => {
                shared.muted = false;
                self.inner.view.send_modify(|view| view.muted = false);
            }
            (None, Some(_)) => {}
        }
    }

    /// Sets the microphone setting in the call of the moment, for a test.
    #[cfg(feature = "test-support")]
    pub fn set_muted(&self, muted: bool) {
        self.inner.set_muted(muted);
    }

    /// Puts away the statement that an action had no effect.
    pub fn dismiss_unused(&self) {
        self.inner
            .view
            .send_if_modified(|view| view.unused.take().is_some());
    }

    /// Puts away the statement that the calls closed the microphone setting.
    pub fn dismiss_closed(&self) {
        self.inner
            .view
            .send_if_modified(|view| view.closed.take().is_some());
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
            if let Some(active) = &shared.active {
                // The release is a close: the frames stop here. The press carries the
                // count of closes of this moment, so a release that follows it is later
                // than the press when the engine takes the press.
                let stops = if talking {
                    active.gate.stops()
                } else {
                    active.gate.stop_now()
                };
                let _ = active
                    .commands
                    .send(SessionCommand::Talking(talking, stops));
            }
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

    /// Opens the microphone again after it failed or was refused, at the seat that the
    /// person saw. At another seat nothing opens (ADR 0788).
    pub fn retry_microphone_in(&self, seat: &Seat) {
        self.inner
            .tell(SessionCommand::RetryMicrophone(seat.clone()));
    }

    /// Opens the microphone again in the call of the moment, for a test.
    #[cfg(feature = "test-support")]
    pub fn retry_microphone(&self) {
        if let Some(seat) = self.seat() {
            self.retry_microphone_in(&seat);
        }
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

    /// The screens, windows and applications that can be shared now. The capture reads
    /// them in a task of the runtime of the client: the capture of each system waits with
    /// Tokio, and the window polls the returned future on an executor without a runtime
    /// (ADR 0733).
    pub fn share_sources(&self) -> Pending<Result<ShareSources, CaptureError>> {
        let capture = self.inner.options.capture.clone();
        let reading =
            crate::executor::on_runtime(
                &self.inner.runtime,
                async move { capture.sources().await },
            );
        Box::pin(async move {
            reading
                .await
                .map_err(|error| CaptureError::Failed(error.to_string()))?
        })
    }

    /// Starts the share of this device at the seat for which the person chose it. The
    /// result is in the view of the call. Without a call nothing starts, and at another
    /// seat nothing starts and the view tells it (ADR 0788).
    pub fn start_share_in(&self, seat: &Seat, request: CaptureRequest) {
        self.inner
            .tell(SessionCommand::StartShare(seat.clone(), request));
    }

    /// Starts the share in the call of the moment, for a test.
    #[cfg(feature = "test-support")]
    pub fn start_share(&self, request: CaptureRequest) {
        if let Some(seat) = self.seat() {
            self.start_share_in(&seat, request);
        }
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

    /// The camera pictures currently drawn by the interface and their pixel bounds.
    /// An empty map suspends remote video until a drawing window asks for it again.
    pub fn set_camera_views(&self, views: HashMap<ShareId, (u32, u32)>) {
        let mut shared = self.inner.shared();
        if shared.camera_views.as_ref() != Some(&views) {
            shared.camera_views = Some(views.clone());
            drop(shared);
            self.inner.tell(SessionCommand::CameraViews(views));
        }
    }

    /// Reports the actual pixel size of one camera tile in the drawing window.
    pub fn set_camera_view(&self, id: ShareId, size: (u32, u32)) {
        let mut views = self.inner.shared().camera_views.clone().unwrap_or_default();
        if views.insert(id, size) != Some(size) {
            self.set_camera_views(views);
        }
    }

    /// Removes requests for cameras which the drawing window no longer shows.
    pub fn retain_camera_views(&self, visible: &[ShareId]) {
        let mut views = self.inner.shared().camera_views.clone().unwrap_or_default();
        views.retain(|id, _| visible.contains(id));
        self.set_camera_views(views);
    }

    /// Gives a view of the calls to the readers, as the calls do, for a test of a reader
    /// that needs a connected call without a media server, such as the requests of a
    /// recording against a server whose media server is in memory. No session runs: an
    /// action on the media of the call does nothing.
    #[cfg(feature = "test-support")]
    pub fn show_view_for_test(&self, view: CallsView) {
        self.inner.view.send_replace(view);
    }

    /// Gives a picture of the watched share to the readers, as the session of a call
    /// does, for a test of a reader.
    #[cfg(feature = "test-support")]
    pub fn show_main_picture_for_test(&self, picture: Option<MainPicture>) {
        self.inner.main_picture.send_replace(picture);
    }

    /// Gives small pictures of the shares to the readers, as the session of a call does,
    /// for a test of a reader.
    #[cfg(feature = "test-support")]
    pub fn show_small_pictures_for_test(&self, pictures: HashMap<ShareId, Arc<VideoPicture>>) {
        self.inner.small_pictures.send_replace(pictures);
    }

    /// Gives pictures of the cameras to the readers, as the session of a call does, for a
    /// test of a reader.
    #[cfg(feature = "test-support")]
    pub fn show_camera_pictures_for_test(&self, pictures: HashMap<ShareId, Arc<VideoPicture>>) {
        self.inner.camera_pictures.send_replace(pictures);
    }

    #[cfg(feature = "test-support")]
    pub fn simulate_full_reconnect(&self) {
        self.inner.tell(SessionCommand::SimulateFullReconnect);
    }
    #[cfg(feature = "test-support")]
    pub fn simulate_connection_loss(&self) {
        self.inner.tell(SessionCommand::SimulateConnectionLoss);
    }

    /// Queries the connection statistics for a failed live test.
    #[cfg(feature = "test-support")]
    pub fn statistics(
        &self,
    ) -> impl Future<Output = Result<livekit::rtc_engine::SessionStats, String>> + Send + 'static + use<>
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
    ) -> impl Future<Output = Option<crate::engine::statistics::SoundCheck>> + Send + 'static + use<>
    {
        let (reply, check) = oneshot::channel();
        self.inner.tell(SessionCommand::SoundCheck(reply));
        async move { check.await.ok() }
    }

    /// Holds the task of the media engine of the call of this device, for a test of what goes
    /// on while it does not run, until the returned value is dropped. Ends when the task
    /// waits. Nothing when this device has no call.
    #[cfg(feature = "test-support")]
    pub fn hold_engine(
        &self,
    ) -> impl Future<Output = Option<crate::engine::HeldEngine>> + Send + 'static + use<> {
        let (reply, held) = oneshot::channel();
        self.inner.tell(SessionCommand::HoldEngine(reply));
        async move {
            let (waits, held) = held.await.ok()?;
            waits.await.ok()?;
            Some(held)
        }
    }

    /// Starts the camera of this device at the seat that the person saw. At another seat
    /// nothing starts and the view tells it (ADR 0788).
    pub fn start_camera_in(&self, seat: &Seat, device: Option<String>) {
        self.inner
            .tell(SessionCommand::StartCamera(seat.clone(), device));
    }

    /// Starts the camera in the call of the moment, for a test.
    #[cfg(feature = "test-support")]
    pub fn start_camera(&self, device: Option<String>) {
        if let Some(seat) = self.seat() {
            self.start_camera_in(&seat, device);
        }
    }

    pub fn camera_devices(
        &self,
    ) -> Result<Vec<crate::camera::CameraDevice>, crate::camera::CameraError> {
        self.inner.options.camera.devices()
    }

    pub fn stop_camera(&self) {
        self.inner.tell(SessionCommand::StopCamera);
    }

    pub fn camera_pictures(&self) -> watch::Receiver<HashMap<ShareId, Arc<VideoPicture>>> {
        self.inner.camera_pictures.subscribe()
    }

    pub fn main_picture(&self) -> watch::Receiver<Option<MainPicture>> {
        self.inner.main_picture.subscribe()
    }

    /// A small picture of each share of the call of this device.
    pub fn small_pictures(&self) -> watch::Receiver<HashMap<ShareId, Arc<VideoPicture>>> {
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
            let own = own_account(server);
            let other = if own == Some(*low_account_id) {
                *high_account_id
            } else {
                *low_account_id
            };
            api.join_direct_call(other).await
        }
    }
}

/// A request for a new token of the seat `identity` (ADR 0520): the server renews that
/// seat or refuses with `seat_ended`; it makes, ends and takes no seat.
async fn request_renewal(
    server: &ServerHandle,
    place: &CallPlace,
    identity: &str,
) -> Result<CallJoin, ApiError> {
    let api = server.api();
    match place {
        CallPlace::Channel { channel_id, .. } => {
            api.renew_channel_call(*channel_id, identity).await
        }
        CallPlace::Direct {
            low_account_id,
            high_account_id,
        } => {
            let own = own_account(server);
            let other = if own == Some(*low_account_id) {
                *high_account_id
            } else {
                *low_account_id
            };
            api.renew_direct_call(other, identity).await
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

/// True when the answer of a server to a request for a place does not tell the blocks of
/// the person: the call hides who is inside, and the server has not seen the device with
/// the identity of the place in the room, so the answer names nobody and no block
/// (ADR 0517). The server tells the blocks with the entry of that device.
pub(crate) fn blocks_untold(call: &Call, identity: &str) -> bool {
    call.hides_occupants && !lists_identity(call, identity)
}

fn lists_identity(call: &Call, identity: &str) -> bool {
    call.participants
        .iter()
        .any(|participant| participant.identity == identity)
}

/// The account of this device on a server: the account of its session, or, while the
/// client core logs in again after an ended session and holds no session, the account
/// that the access flow shows (ADR 0787). A login that gives another account ends the
/// call when the session reads the state (`read_as`).
fn own_account(server: &ServerHandle) -> Option<AccountId> {
    server
        .account()
        .or_else(|| server.owner().map(|owner| owner.account_id))
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
    let Some(account_id) = own_account(&server) else {
        return false;
    };
    // Without a position and without a state the answer belongs to a history that no
    // state has: the session then loads the first state of the server again and takes it.
    let unknown = Cursor {
        epoch: StreamEpoch::generate(),
        position: 0,
    };
    let read_at = read_at(&answer, &server, unknown);
    // A grant that the server states with the answer came before this session read any
    // event: it closes the setting once (ADR 0521).
    let granted = (answer.meeting_grant == Some(true)).then(|| {
        answer
            .call
            .participants
            .iter()
            .find(|person| person.identity == answer.identity)
            .is_some_and(|person| person.hand_raised)
    });
    let mut grants = MeetingHistory::new(
        read_at,
        &answer.call,
        account_id,
        answer.microphone_block.is_some(),
    );
    if answer.cursor.is_none() {
        grants.unordered();
    }
    let (commands, taken) = mpsc::unbounded_channel();
    let mut shared = inner.shared();
    if shared.joining.is_none_or(|joining| joining.key != join) {
        return false;
    }
    shared.next_key += 1;
    let key = shared.next_key;
    // The state of the server lists the person only after the media connection is made.
    // The blocks of the answer hold from the first moment (ADR 0503). An answer that does
    // not tell the blocks keeps the sound off until the server tells them.
    let blocks_untold = blocks_untold(&answer.call, &answer.identity);
    let admitted = false;
    let refresh = None;
    let accepted = read_at;
    let controls = Controls {
        muted: shared.muted,
        deafened: shared.deafened,
        microphone_blocked: true,
        sound_blocked: true,
        camera_held: true,
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
            camera: inner.options.camera.clone(),
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
    if let Some(views) = &shared.camera_views {
        engine.set_camera_views(views.clone());
    }
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
        gate: engine.gate(),
    });
    drop(shared);
    let timing = inner.options.timing;
    let mut place =
        Place::new(answer.identity, Instant::now(), timing.settle).on_device(device_of(&server));
    if let Some(cursor) = answer.cursor {
        place.told(cursor.position);
    }
    let session = Session {
        inner: inner.clone(),
        key,
        server,
        target,
        place,
        fate: Fate::Undecided,
        review: true,
        engine,
        controls,
        microphone_block: answer.microphone_block,
        sound_block: answer.sound_block,
        blocks_untold,
        refresh,
        removal_read: None,
        accepted,
        seat_told: answer.cursor,
        grants,
        admitted,
        connected_once: false,
        interrupted_at: None,
        retry: None,
        backoff: Backoff::new(timing.reconnect_first, timing.reconnect_longest),
        rejoining: false,
        renew_open: false,
        #[cfg(test)]
        renewals: Vec::new(),
        grant_closed: granted.is_some(),
        state_report: Reporter::new(timing.state_spacing, None),
        speaking_report: Reporter::new(timing.speaking_spacing, Some(timing.speaking_renewal)),
        share_report: Reporter::holding(timing.share_spacing, None),
        moving: None,
        probes: Vec::new(),
        speaking: false,
        timing,
    };
    let mut session = session;
    if let Some(hand_raised) = granted {
        session.close_for_grant(read_at.position, hand_raised);
    }
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
    /// The device of this client on the server. A seat of the account on another device
    /// is not this place.
    device: Option<DeviceId>,
    /// The position of the answer to a request for a new token that gave this seat in
    /// place of an earlier one. The events up to it tell why the earlier seat ended; it is
    /// absent when the events have passed it.
    given: Option<u64>,
    /// The last event that showed a seat of the account on another device.
    taken: Option<u64>,
    /// The number of the last event that was read.
    read: Option<u64>,
    /// Events were not delivered since a state last showed the seat.
    unread: bool,
    /// The seat is not used, for this reason: a request for a new token gave it after the
    /// server had ended the earlier seat by a rule, or after another device had taken the
    /// call, or while the events that tell this were not delivered (ADR 0785).
    void: Option<EndReason>,
}

/// True when the server ended a seat because no media connection used it. Only then a
/// request for a new token continues the call at another seat (ADR 0734).
fn lapsed(reason: Option<LeaveReason>) -> bool {
    matches!(
        reason,
        Some(LeaveReason::ConnectionLost | LeaveReason::NeverConnected)
    )
}

fn device_of(server: &ServerHandle) -> Option<DeviceId> {
    match server.entry().membership {
        Membership::Member { device_id, .. } => Some(device_id),
        Membership::None
        | Membership::JoinRequested { .. }
        | Membership::Banned { .. }
        | Membership::Inactive { .. }
        | Membership::Deleted { .. }
        | Membership::Removed => None,
    }
}

/// The place that an account holds in a state: its identity, and its device when the state
/// names it.
type Holder<'a> = (&'a str, Option<DeviceId>);

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
            device: None,
            given: None,
            taken: None,
            read: None,
            unread: false,
            void: None,
        }
    }

    pub(crate) fn on_device(mut self, device: Option<DeviceId>) -> Self {
        self.device = device;
        self
    }

    pub(crate) fn identity(&self) -> &str {
        &self.identity
    }

    /// Why the seat is not used, when it is not.
    pub(crate) fn void(&self) -> Option<&EndReason> {
        self.void.as_ref()
    }

    /// Events were not delivered: notices were missed, or a snapshot replaced the state.
    /// Why an earlier seat ended can be among them.
    pub(crate) fn untold(&mut self) {
        self.unread = true;
        if self.given.is_some() {
            self.void.get_or_insert(EndReason::NotRead);
        }
    }

    /// A request for a new token gave this seat, in place of an earlier one, with an
    /// answer at `position`. The events up to that position tell why the earlier seat
    /// ended; when they were read before the answer, nothing is waited for.
    pub(crate) fn given(&mut self, position: u64) {
        if self.read.is_none_or(|read| read < position) {
            self.given = Some(position);
        }
    }

    /// True while the events up to the answer that gave this seat are not all read: the
    /// seat can still be void, and nothing of the person is sent at it.
    pub(crate) fn unsettled(&self) -> bool {
        self.given.is_some()
    }

    /// True when an event after the last sign of the seat showed a seat of the account on
    /// another device: the call is there.
    fn taken(&self) -> bool {
        self.taken
            .zip(self.shown)
            .is_some_and(|(taken, shown)| taken > shown)
    }

    fn elsewhere(&self, device: Option<DeviceId>) -> bool {
        self.device
            .zip(device)
            .is_some_and(|(own, other)| own != other)
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
        self.read = self.read.max(Some(id));
        // An event from before the answer that gave this seat tells what happened to the
        // earlier one.
        let earlier = self.given.is_some_and(|given| id <= given);
        match event {
            Event::OwnCallChanged {
                call: Some(own), ..
            } if own.identity == self.identity => {
                self.shown = self.shown.max(Some(id));
            }
            Event::OwnCallChanged {
                call: Some(own), ..
            } if self.elsewhere(Some(own.device_id)) => {
                self.taken = self.taken.max(Some(id));
                if earlier {
                    self.void.get_or_insert(EndReason::Elsewhere);
                }
            }
            Event::OwnCallChanged { call: None, left } => {
                if self.left.is_none_or(|(before, _)| id > before) {
                    self.left = Some((id, *left));
                }
                if earlier && !lapsed(*left) {
                    self.void.get_or_insert(EndReason::Server(*left));
                }
            }
            _ => {}
        }
        if !earlier {
            self.given = None;
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

    /// An answer of the server stated that the place exists at `position` (ADR 0519).
    pub(crate) fn told(&mut self, position: u64) {
        self.shown = self.shown.max(Some(position));
    }

    /// Decides as [`Place::fate`] from an answer of the server that stated at `cursor`
    /// that the place exists, when no state is as new as that answer (ADR 0785).
    pub(crate) fn fate_told(&mut self, cursor: Cursor, asking: bool, now: Instant) -> Fate {
        let own = self.identity.clone();
        self.decide(
            Some((cursor, Some((own.as_str(), self.device)))),
            asking,
            now,
        )
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
        let state = state.map(|(cursor, own)| {
            let own = own.map(|own| (own.identity.as_str(), Some(own.device_id)));
            (cursor, own)
        });
        self.decide(state, asking, now)
    }

    /// `state`: a cursor, and the place that the account holds there.
    fn decide(
        &mut self,
        state: Option<(Cursor, Option<Holder<'_>>)>,
        asking: bool,
        now: Instant,
    ) -> Fate {
        if let Some(reason) = &self.void {
            return Fate::End(reason.clone());
        }
        if let Some((cursor, _)) = state
            && self.history != Some(cursor.epoch)
        {
            if self.history.is_some() {
                // Another history: what the earlier one told is void.
                self.shown = None;
                self.left = None;
                self.taken = None;
                self.read = None;
            }
            self.history = Some(cursor.epoch);
            self.events.get_or_insert(cursor.epoch);
        }
        let own = state.and_then(|(_, own)| own);
        let here = own.is_some_and(|(identity, _)| identity == self.identity);
        // The account holds a seat on another device: the call is there now. An event tells
        // it whatever a state shows, as it tells an end.
        if self.taken() {
            return Fate::End(EndReason::Elsewhere);
        }
        let elsewhere = !here && own.is_some_and(|(_, device)| self.elsewhere(device));
        // A request for a place runs. Its answer can name a new seat when the server ended
        // this one because no media connection used it. An end by a rule of the server, and
        // a seat on another device, are not waited for: the answer would undo them.
        let waits = match self.ended() {
            Some(reason) => lapsed(reason),
            None => !here,
        };
        if asking && !elsewhere && waits {
            return Fate::Undecided;
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
            Some(_) if here => {
                self.shown = self.shown.max(Some(cursor.position));
                self.unread = false;
                Fate::Held
            }
            // The events of a join arrive after its answer. Until the server has shown the
            // place once, or the time for that has passed, a state without it decides
            // nothing.
            _ if self.shown.is_none() && now < self.since + self.settle => Fate::Undecided,
            Some(_) if asking && !elsewhere => Fate::Undecided,
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
        if self.taken() {
            // The request took the call back from another device.
            self.void.get_or_insert(EndReason::Elsewhere);
        }
        identity.clone_into(&mut self.identity);
        self.since = now;
        self.shown = None;
        self.left = None;
        self.given = None;
        self.taken = None;
        if std::mem::take(&mut self.unread) {
            // Why the earlier seat ended is not known.
            self.void.get_or_insert(EndReason::NotRead);
        }
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

/// Asks the server for a new token for the place of a session, in its turn: for the seat
/// that the session holds, or with `None` for a place, after the server stated that the
/// seat ended because no media connection used it (ADR 0520).
async fn renew(
    inner: Arc<Inner>,
    server: ServerHandle,
    target: CallTarget,
    seat: Option<String>,
    done: mpsc::UnboundedSender<Done>,
) {
    if done.is_closed() {
        // The call ended while the request waited for its turn.
        return;
    }
    let answer = match &seat {
        Some(identity) => request_renewal(&server, &target.place, identity).await,
        None => request(&server, &target.place, Entry::Open).await,
    };
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

/// The position up to which an answer to a request for a place holds the events
/// (ADR 0519). A server from before that ADR names none. Its answer is taken as read at
/// the position of the state that the client holds now: the state then replaces what the
/// answer told as soon as it lists the seat, as it did before that ADR. Without a state it
/// is taken at `otherwise`.
fn read_at(answer: &CallJoin, server: &ServerHandle, otherwise: Cursor) -> Cursor {
    answer.cursor.unwrap_or_else(|| {
        let view = server.live().view();
        view.model
            .as_deref()
            .map_or(otherwise, |model| model.cursor)
    })
}

struct MeetingHistory {
    cursor: Cursor,
    call_id: CallId,
    account_id: AccountId,
    presenter: Option<AccountId>,
    blocked: bool,
    handled: Option<u64>,
    /// The position of the last state or answer that showed the microphone block.
    block_seen: Option<u64>,
    /// The position of the last event that showed the seat without a microphone block: a
    /// change of the person, or the end of meeting mode, which removes every block
    /// (ADR 0512).
    unblocked: Option<u64>,
    /// The events up to this position were not all delivered: notices were missed, or a
    /// snapshot replaced the state. After missed notices the position is not known until
    /// the next event.
    untold: Option<u64>,
    /// A state or an answer showed the end of the block, and the session keeps the block
    /// until an event tells how it ended.
    kept: bool,
}

impl MeetingHistory {
    fn new(cursor: Cursor, call: &Call, account_id: AccountId, blocked: bool) -> Self {
        Self {
            cursor,
            call_id: call.id,
            account_id,
            presenter: call.meeting.as_ref().map(|meeting| meeting.presenter),
            blocked,
            handled: None,
            block_seen: blocked.then_some(cursor.position),
            unblocked: None,
            untold: None,
            kept: false,
        }
    }

    /// The block came with an answer without a position. Such an answer has no order with
    /// the events (ADR 0785), so the end of its block is not looked for in them.
    fn unordered(&mut self) {
        self.block_seen = None;
    }

    /// The server has another history: its positions are not compared with the ones
    /// before, and what the earlier history told of the meeting and the block is void.
    fn restarted(&mut self, cursor: Cursor) {
        self.cursor = cursor;
        self.presenter = None;
        self.blocked = false;
        self.handled = None;
        self.block_seen = None;
        self.unblocked = None;
        self.untold = None;
        self.kept = false;
    }

    /// Notices were missed. Which events they held is not known.
    fn missed(&mut self) {
        self.untold = Some(u64::MAX);
    }

    /// A snapshot at `position` replaced the state. The events up to it are not told.
    fn replaced(&mut self, position: u64) {
        self.untold = self.untold.max(Some(position));
    }

    /// True when the events told how the block ended that a state or an answer last
    /// showed at `seen`, or the person was told a grant or opened the microphone after it.
    fn explains(&self, seen: u64) -> bool {
        let after = |at: Option<u64>| at.is_some_and(|at| at > seen);
        after(self.unblocked) || after(self.handled)
    }

    /// True when the events of the time after `seen` were not all delivered.
    fn lost_after(&self, seen: u64) -> bool {
        self.untold.is_some_and(|untold| untold > seen)
    }

    fn note(&mut self, id: u64, event: &Event, identity: &str) -> Option<bool> {
        if id <= self.cursor.position {
            return None;
        }
        if self.untold == Some(u64::MAX) {
            // The first event after missed notices: the ones before it were not read.
            self.untold = Some(id - 1);
        }
        self.cursor.position = id;
        match event {
            Event::CallMeetingStarted {
                call_id, meeting, ..
            } if *call_id == self.call_id => {
                self.presenter = Some(meeting.presenter);
                // The event carries no participant: every person except the presenter has
                // a block from it (ADR 0512, ADR 0719).
                if meeting.presenter != self.account_id {
                    self.blocked = true;
                }
            }
            Event::CallMeetingEnded { call_id, .. } if *call_id == self.call_id => {
                self.presenter = None;
                self.blocked = false;
                self.unblocked = Some(id);
            }
            Event::CallParticipantJoined {
                call_id,
                participant,
                ..
            }
            | Event::CallParticipantUpdated {
                call_id,
                participant,
                ..
            } if *call_id == self.call_id && participant.identity == identity => {
                if let Some(block) = &participant.microphone_block
                    && block.reason == junto_client_core::protocol::call::BlockReason::Meeting
                {
                    self.presenter = Some(block.by);
                }
                let blocked = participant.microphone_block.is_some();
                let grant = self.blocked
                    && !blocked
                    && self
                        .presenter
                        .is_some_and(|presenter| presenter != self.account_id);
                self.blocked = blocked;
                if !blocked {
                    self.unblocked = Some(id);
                }
                if grant {
                    return Some(participant.hand_raised);
                }
            }
            _ => {}
        }
        None
    }
}

fn account_ended(
    account: Option<AccountId>,
    access: &junto_client_core::flow::access::AccessState,
    expected: AccountId,
) -> bool {
    !matches!(access, junto_client_core::flow::access::AccessState::LoggedIn { account_id, .. }
        if *account_id == expected && account.is_none_or(|account| account == expected))
}

type FreshState = Pin<Box<dyn Future<Output = Cursor> + Send>>;

fn fresh_state(server: ServerHandle) -> FreshState {
    let first = server.live().refresh();
    Box::pin(async move {
        if let Ok(cursor) = first.await {
            return cursor;
        }
        loop {
            tokio::time::sleep(REPORT_RETRY).await;
            if let Ok(cursor) = server.live().refresh().await {
                return cursor;
            }
        }
    })
}

async fn refreshed(refresh: &mut Option<FreshState>) -> Cursor {
    match refresh {
        Some(refresh) => refresh.await,
        None => std::future::pending().await,
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
    /// The server has not told the blocks of the person for this place yet (see
    /// [`blocks_untold`]). The sound stays off until it has, so that a sound block holds
    /// from the first moment of the media connection (ADR 0503).
    blocks_untold: bool,
    refresh: Option<FreshState>,
    /// The number of the media connection after whose removal by the media server a state
    /// was asked for.
    removal_read: Option<u64>,
    accepted: Cursor,
    /// The cursor of the newest answer of the server that stated the seat of the place
    /// (ADR 0519). It is the membership of the person until a state is as new (ADR 0785).
    seat_told: Option<Cursor>,
    grants: MeetingHistory,
    admitted: bool,
    connected_once: bool,
    interrupted_at: Option<OffsetDateTime>,
    /// When the next request for a new token is due.
    retry: Option<Instant>,
    backoff: Backoff,
    rejoining: bool,
    /// The server stated that the seat ended because no media connection used it: the
    /// next request asks for a place without naming the seat (ADR 0520).
    renew_open: bool,
    /// The seat that each request for a new token named, in the order of the requests.
    #[cfg(test)]
    renewals: Vec<Option<String>>,
    /// The setting was closed for a grant that a read of the server stated, and no read
    /// has stated the end of that statement since (ADR 0521).
    grant_closed: bool,
    state_report: Reporter<(bool, bool)>,
    speaking_report: Reporter<bool>,
    /// The sound of the share that the server was told. Absent: no share.
    share_report: Reporter<Option<ShareAudio>>,
    /// The move of the person of this device that waits (ADR 0511).
    moving: Option<Moving>,
    /// The work of moves that have ended. Each closes the connection that it made.
    probes: Vec<tokio::task::JoinHandle<()>>,
    speaking: bool,
    timing: CallTiming,
}

/// A move of the person of this device that has no outcome yet.
struct Moving {
    call_move: CallMove,
    /// Ends the connection of this device to the room of the destination: the work that
    /// holds the connection closes it when this is sent or dropped.
    _stop: oneshot::Sender<()>,
    /// The work that asks for the token of the move and holds the connection.
    task: tokio::task::JoinHandle<()>,
    /// When the move is given up. The server ends a move after 30 seconds (ADR 0511); an
    /// outcome that has not arrived by this time was missed.
    deadline: Instant,
    /// The state of the server was replaced by a snapshot since the move began: the event
    /// with the outcome can be among the changes that were not told.
    reloaded: bool,
}

/// How long the outcome of a move is waited for.
const MOVE_OUTCOME_LIMIT: Duration = Duration::from_secs(45);

/// What became of the connection of a device to the room of the destination of a move.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Probed {
    /// The move ended before a connection was started. Nothing was connected.
    Stopped,
    /// The server gave no token, or the connection was not made.
    Failed,
    /// The connection was made, held until the move ended, and closed.
    Closed,
}

/// Asks for the token of a move, connects with it, holds the connection until the move
/// ends, and closes it. `stopped` ends when the move has its outcome or is given up, or
/// when the call ends. A move that has ended starts no connection: not while the server
/// is asked, and not with an answer that arrives afterwards. A connection that is being
/// made is not cut off, because a connection that is half made can stay in the room; it
/// is closed as soon as it is made.
pub(crate) async fn probe<T, R>(
    mut stopped: oneshot::Receiver<()>,
    token: impl Future<Output = Option<T>>,
    connect: impl AsyncFnOnce(T) -> Option<R>,
    close: impl AsyncFnOnce(R),
) -> Probed {
    let token = tokio::select! {
        biased;
        _ = &mut stopped => return Probed::Stopped,
        token = token => token,
    };
    let Some(token) = token else {
        return Probed::Failed;
    };
    let Some(room) = connect(token).await else {
        return Probed::Failed;
    };
    let _ = stopped.await;
    close(room).await;
    Probed::Closed
}

fn far() -> Instant {
    Instant::now() + Duration::from_secs(86_400)
}

impl Session {
    async fn run(mut self, mut commands: mpsc::UnboundedReceiver<SessionCommand>) {
        let mut engine_state = self.engine.state();
        let mut levels = self.engine.levels();
        let mut main_picture = self.engine.main_picture();
        let mut small_pictures = self.engine.small_pictures();
        let mut camera_pictures = self.engine.camera_pictures();
        let mut live = self.server.live().watch();
        let mut notices = self.server.live().notices();
        self.subscribed(self.server.live().view().model.as_deref());
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
                changed = camera_pictures.changed() => {
                    if changed.is_ok() { self.inner.camera_pictures.send_replace(camera_pictures.borrow_and_update().clone()); }
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
                notice = notices.next() => match notice {
                    Some(Ok(notice)) => self.note(&notice),
                    Some(Err(Missed(_))) => self.missed(),
                    None => {}
                },
                cursor = refreshed(&mut self.refresh) => self.reloaded_at(cursor),
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
            probes,
            timing,
            ..
        } = self;
        // The media connection and the devices close at once. The cause of the end is
        // told a moment later, when the event that states it has arrived.
        engine.close().await;
        if place.void().is_some() {
            // The server holds the seat that this session did not use.
            let (inner, identity) = (inner.clone(), place.identity.clone());
            drop(inner.clone().in_turn(async move {
                give_up(&inner, target.server_id, &identity).await;
            }));
        }
        inner.levels.send_replace(Levels::default());
        inner.main_picture.send_replace(None);
        inner.small_pictures.send_replace(HashMap::new());
        inner.camera_pictures.send_replace(HashMap::new());
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
        // The work of a move closes the connection that it made before the call counts as
        // left: what follows the end can use the same identity in the room of the
        // destination.
        for task in probes {
            let _ = tokio::time::timeout(timing.move_end, task).await;
        }
        if let Some(ended) = ended {
            let _ = ended.send(place.identity);
        }
    }

    fn note(&mut self, notice: &Notice) {
        match notice {
            Notice::Event { id, event, .. } => {
                if let Some(hand_raised) = self.grants.note(*id, event, self.place.identity()) {
                    self.close_for_grant(*id, hand_raised);
                }
                self.end_kept_block();
                self.place.note(*id, event);
                self.note_move(event);
            }
            Notice::Reloaded { cursor } => {
                if self.grants.cursor.epoch != cursor.epoch {
                    self.grants.restarted(*cursor);
                } else {
                    self.grants.replaced(cursor.position);
                    self.end_kept_block();
                }
                self.place.untold();
                self.place.reloaded(cursor.epoch);
                if let Some(moving) = &mut self.moving {
                    moving.reloaded = true;
                }
            }
            Notice::Ended { .. } => {}
        }
        self.review = true;
    }

    /// A state that the session asked for is loaded: its cursor.
    fn reloaded_at(&mut self, cursor: Cursor) {
        self.refresh = None;
        if self.accepted.epoch != cursor.epoch {
            self.microphone_block = None;
            self.sound_block = None;
            self.blocks_untold = true;
            // Another history: what the events of the earlier one told of the place is
            // void. In the same history it stays: the end of the place and its reason.
            self.place.left = None;
            self.place.taken = None;
            self.place.shown = None;
            self.place.read = None;
            self.place.history = None;
            self.place.events = None;
        }
        if self.grants.cursor.epoch != cursor.epoch {
            self.grants.restarted(cursor);
        }
        self.accepted = cursor;
        self.seat_told = None;
        if let Some(moving) = &mut self.moving {
            moving.reloaded = true;
        }
        self.review = true;
    }

    /// The session reads the notices from now on, and `held` is the state of now. The
    /// notices of the events between the answer of the server and the state were given to
    /// nobody.
    fn subscribed(&mut self, held: Option<&ServerModel>) {
        if let Some(model) = held
            && model.cursor.epoch == self.grants.cursor.epoch
            && model.cursor.position > self.grants.cursor.position
        {
            self.grants.replaced(model.cursor.position);
        }
    }

    /// Notices were missed. What they told is taken from the state where the state holds
    /// it; a grant and the reason of an end are in no state, so the session decides without
    /// them in the direction that opens nothing (ADR 0785).
    fn missed(&mut self) {
        self.grants.missed();
        self.end_kept_block();
        self.place.untold();
        if let Some(moving) = &mut self.moving {
            moving.reloaded = true;
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
                if call_move.account_id == self.grants.account_id
                    && call_move.from == self.target.place =>
            {
                self.start_move(call_move);
            }
            Event::CallMoveResolved { move_id, outcome } => self.move_resolved(*move_id, *outcome),
            _ => {}
        }
    }

    fn start_move(&mut self, call_move: &CallMove) {
        if call_move.expires_at <= OffsetDateTime::now_utc()
            || self
                .moving
                .as_ref()
                .is_some_and(|moving| moving.call_move.id == call_move.id)
        {
            return;
        }
        self.end_move();
        let (api, move_id) = (self.server.api(), call_move.id);
        let (stop, stopped) = oneshot::channel::<()>();
        let task = self.inner.runtime.spawn(async move {
            let token = async { api.join_move(move_id).await.ok() };
            let connect = |answer: CallJoin| async move {
                let connecting = Room::connect(
                    &answer.media_url,
                    answer.token.expose(),
                    RoomOptions::default(),
                );
                tokio::time::timeout(MOVE_CONNECT_LIMIT, crate::engine::connecting(connecting))
                    .await
                    .ok()?
                    .ok()
            };
            // A connection that is only dropped stays in the room of the destination.
            let close = |(room, _events): (Room, _)| async move {
                if let Err(error) = room.close().await {
                    tracing::debug!(%error, "the connection of a move was not closed");
                }
            };
            if probe(stopped, token, connect, close).await == Probed::Failed
                && let Err(error) = api.fail_move(move_id).await
            {
                tracing::debug!(%error, "the failure of a move was not told");
            }
        });
        self.moving = Some(Moving {
            call_move: call_move.clone(),
            _stop: stop,
            task,
            deadline: Instant::now() + MOVE_OUTCOME_LIMIT,
            reloaded: false,
        });
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
        if let Some(moving) = self.moving.take() {
            // The stop is dropped here. The work is kept until it has closed what it made.
            self.probes.retain(|task| !task.is_finished());
            self.probes.push(moving.task);
        }
    }

    /// Decides a move whose outcome was not told. After a snapshot replaced the state, a
    /// move that the state no longer holds has ended: the person is in the call of the
    /// destination, or the move had no effect. A move without an outcome at its time
    /// limit is given up.
    fn review_move(&mut self, model: Option<&ServerModel>) {
        if let Some(moving) = &self.moving {
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
        if self.moving.is_none()
            && let Some(model) = model
            && model
                .own_call
                .as_ref()
                .is_some_and(|own| own.identity == self.place.identity())
            && let Some(call_move) = model.moves.iter().find(|call_move| {
                call_move.account_id == model.account.id
                    && call_move.from == self.target.place
                    && call_move.expires_at > OffsetDateTime::now_utc()
            })
        {
            self.start_move(call_move);
        }
    }

    fn command(&mut self, command: SessionCommand) {
        match command {
            SessionCommand::StartCamera(seat, device) => {
                if self.holds(&seat, Unused::Camera) {
                    self.engine.start_camera(device);
                }
            }
            SessionCommand::StopCamera => self.engine.stop_camera(),
            SessionCommand::Controls => self.apply_controls(),
            SessionCommand::Muted(muted) => {
                if !muted {
                    self.opened_by_the_person();
                }
                self.apply_controls();
            }
            SessionCommand::Unmute(seat, closes) => {
                if self.holds(&seat, Unused::Microphone) {
                    let opened = {
                        let mut shared = self.inner.shared();
                        // A close that the person made after this open holds.
                        let opened = shared.closes == closes;
                        if opened {
                            shared.muted = false;
                        }
                        opened
                    };
                    if opened {
                        self.inner.view.send_modify(|view| view.muted = false);
                        self.opened_by_the_person();
                        self.apply_controls();
                    }
                }
            }
            SessionCommand::Talking(talking, stops) => {
                self.engine.set_talking_at(talking, stops);
            }
            SessionCommand::Volume(account_id, gain) => self.engine.set_volume(account_id, gain),
            SessionCommand::Config(config) => self.engine.set_config(config),
            SessionCommand::RetryMicrophone(seat) => {
                if self.holds(&seat, Unused::Microphone) {
                    self.engine.retry_microphone();
                }
            }
            SessionCommand::StartShare(seat, request) => {
                if self.holds(&seat, Unused::Share) {
                    self.engine.start_share(request);
                }
            }
            SessionCommand::StopShare => self.engine.stop_share(),
            SessionCommand::Watch(choice) => self.engine.watch(choice),
            SessionCommand::ShareVolume(gain) => self.engine.set_share_volume(gain),
            SessionCommand::ViewSize(width, height) => self.engine.set_view_size(width, height),
            SessionCommand::CameraViews(views) => self.engine.set_camera_views(views),
            #[cfg(feature = "test-support")]
            SessionCommand::Statistics(reply) => self.engine.statistics(reply),
            #[cfg(feature = "test-support")]
            SessionCommand::SoundCheck(reply) => self.engine.sound_check(reply),
            #[cfg(feature = "test-support")]
            SessionCommand::HoldEngine(reply) => {
                let _ = reply.send(self.engine.hold());
            }
            #[cfg(feature = "test-support")]
            SessionCommand::SimulateFullReconnect => self.engine.simulate_full_reconnect(),
            #[cfg(feature = "test-support")]
            SessionCommand::SimulateConnectionLoss => self.engine.simulate_connection_loss(),
            // The loop takes this command itself.
            SessionCommand::End { .. } => {}
        }
    }

    /// The person opened the microphone. A grant up to the state of now is answered by
    /// it, and a block whose end waited for its event waits no longer.
    fn opened_by_the_person(&mut self) {
        self.grants.handled = self.grants.handled.max(Some(self.accepted.position));
        self.end_kept_block();
    }

    /// Ends a block that the session kept after a state or an answer showed its end
    /// (see [`Session::take_blocks`]), when its end is told now, or when the events that
    /// told it were not delivered: the end is then taken as a grant.
    fn end_kept_block(&mut self) {
        let Some(seen) = self.grants.block_seen.filter(|_| self.grants.kept) else {
            return;
        };
        if !self.grants.explains(seen) {
            if !self.grants.lost_after(seen) {
                return;
            }
            let position = self.accepted.position.max(seen.saturating_add(1));
            self.close_for_unread_end(position, false);
            // The state that showed the end had no meeting of another presenter.
            self.grants.blocked = false;
            self.grants.presenter = None;
        }
        self.grants.kept = false;
        self.set_blocks((None, self.sound_block.clone()));
    }

    /// True when an action names the call and the seat of this session. An action for
    /// another one has no effect here, and the view tells it (ADR 0788).
    fn holds(&self, seat: &Seat, action: Unused) -> bool {
        let held = seat.target == self.target && seat.identity == self.place.identity();
        if !held {
            self.inner.unused(action);
        }
        held
    }

    fn apply_controls(&mut self) {
        // The count of closes is read with the setting, under the lock that a close of the
        // person holds: controls that open the microphone carry a count from before a
        // close that follows them, so they cannot open the gate after it.
        let (muted, deafened, stops) = {
            let shared = self.inner.shared();
            (shared.muted, shared.deafened, self.engine.stops())
        };
        let controls = Controls {
            muted,
            deafened,
            microphone_blocked: self.microphone_block.is_some() || !self.admitted || self.rejoining,
            sound_blocked: self.sound_block.is_some()
                || self.blocks_untold
                || !self.admitted
                || self.rejoining,
            camera_held: !self.admitted,
        };
        if controls != self.controls {
            self.controls = controls;
            self.engine.set_controls_at(controls, stops);
        }
    }

    /// Reads from the state of the server what concerns this call: whether the place of
    /// this device still exists, and the blocks of the person.
    fn read_server(&mut self) -> Fate {
        let account = self.server.account();
        let access = self.server.access().state;
        self.read_as(account, &access)
    }

    /// `account`: the account of the session that the device holds now. `access`: the
    /// state of the access flow. While the device has its account and no session, because
    /// the client core logs in again (ADR 0787), the call is kept as while the event
    /// stream is interrupted (ADR 0734): nothing here changes without a live state.
    fn read_as(
        &mut self,
        account: Option<AccountId>,
        access: &junto_client_core::flow::access::AccessState,
    ) -> Fate {
        if self.inner.client.server(self.target.server_id).is_none()
            || account_ended(account, access, self.grants.account_id)
        {
            return Fate::End(EndReason::Server(None));
        }
        if let Some(reason) = self.place.void() {
            return Fate::End(reason.clone());
        }
        if self.refresh.is_some() {
            self.review_move(None);
            // No state decides while one is loaded. An end that the events told is an end.
            let asking = self.asking();
            return match self.place.fate(None, asking, Instant::now()) {
                Fate::End(reason) => Fate::End(reason),
                Fate::Held | Fate::Undecided => Fate::Undecided,
            };
        }
        let view = self.server.live().view();
        self.read_state(view.status, view.model.as_deref())
    }

    /// This client asks the server of the call for a place now.
    fn asking(&self) -> bool {
        self.rejoining
            || self.moving.is_some()
            || self
                .inner
                .shared()
                .joining
                .is_some_and(|joining| joining.target.server_id == self.target.server_id)
    }

    /// `status` and `held`: the status of the event stream and the state that it holds.
    fn read_state(&mut self, status: StreamStatus, held: Option<&ServerModel>) -> Fate {
        if let Some(model) = held
            && status == StreamStatus::Live
            && model.cursor.epoch != self.accepted.epoch
        {
            self.admitted = false;
            self.apply_controls();
            self.show();
            self.refresh = Some(fresh_state(self.server.clone()));
            return Fate::Undecided;
        }
        // Without a current state the place can still exist.
        let live = held.filter(|model| {
            status == StreamStatus::Live && model.account.id == self.grants.account_id
        });
        let current = live.filter(|model| {
            model.call_cursor(&self.target.place).position >= self.accepted.position
        });
        if let Some(model) = current {
            self.read_blocks(model);
        }
        // Membership and moves belong to the stream cursor, not an ahead-of-stream call read.
        // A state from before the answer that gave or stated the seat tells nothing of it.
        let told = self
            .seat_told
            .filter(|told| told.epoch == self.accepted.epoch);
        let floor = told.map_or(self.accepted.position, |told| told.position);
        let model = live.filter(|model| model.cursor.position >= floor);
        self.review_move(model);
        // This client asks the server for a place now: a join of the person, which ends
        // this call when it has its answer, or a new token for this call.
        let asking = self.asking();
        let now = Instant::now();
        match (model, told) {
            (Some(model), _) => {
                self.place
                    .fate(Some((model.cursor, model.own_call.as_ref())), asking, now)
            }
            // The stream is live and has not reached the answer that stated the seat: the
            // answer is the membership. An interrupted stream decides nothing (ADR 0734).
            (None, Some(told)) if live.is_some() => self.place.fate_told(told, asking, now),
            (None, _) => self.place.fate(None, asking, now),
        }
    }

    fn read_blocks(&mut self, model: &ServerModel) {
        if self.refresh.is_some() {
            return;
        }
        let cursor = model.call_cursor(&self.target.place);
        if cursor.epoch != self.accepted.epoch || cursor.position < self.accepted.position {
            return;
        }
        self.accepted = cursor;
        // The state lists the person when the media connection is made. Until then the
        // blocks are the ones that the server gave with the place. A server that says
        // that its admission check has not seen this seat is believed before the list;
        // a server that says nothing of the seat is read by the list alone (ADR 0517).
        let own = model
            .call_at(&self.target.place)
            .and_then(|call| call.participant(model.account.id))
            .filter(|own| own.identity == self.place.identity())
            .filter(|own| !model.seat_stated_outside(&own.identity))
            // A seat that a request for a new token gave can still be void (ADR 0785).
            .filter(|_| !self.place.unsettled());
        let Some(own) = own else {
            if std::mem::replace(&mut self.admitted, false) {
                self.apply_controls();
                self.show();
            }
            return;
        };
        let newly_inside = !std::mem::replace(&mut self.admitted, true);
        let blocks = (own.microphone_block.clone(), own.sound_block.clone());
        let hand_raised = own.hand_raised;
        // The state can still list the person with the place of another device of the
        // account. The server has told the blocks of this place when it lists this one.
        let told = std::mem::take(&mut self.blocks_untold);
        let run_by_another = model
            .call_at(&self.target.place)
            .and_then(|call| call.meeting.as_ref())
            .is_some_and(|meeting| meeting.presenter != model.account.id);
        self.take_blocks(blocks, run_by_another, hand_raised);
        match model.grant_stated(self.place.identity()) {
            Some(position) => self.grant_stated(true, position, hand_raised),
            None => self.grant_stated(false, cursor.position, hand_raised),
        }
        if told || newly_inside {
            self.apply_controls();
        }
        if newly_inside {
            self.show();
        }
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
        mut blocks: (Option<MicrophoneBlock>, Option<SoundBlock>),
        run_by_another: bool,
        hand_raised: bool,
    ) {
        // The events that the session has read end before events that were not delivered:
        // what they told of the block and of the meeting is older than this state.
        let behind = self.grants.lost_after(self.grants.cursor.position);
        let shown = blocks.0.is_some();
        let mut kept = false;
        if shown {
            self.grants.block_seen = Some(self.accepted.position);
        } else if self.microphone_block.is_some() && run_by_another {
            // The event of this grant follows the last state that showed the block. When
            // the person was told a grant after that state, or opened the microphone
            // after it, this state tells the same grant again.
            let told = self
                .grants
                .handled
                .zip(self.grants.block_seen)
                .is_some_and(|(handled, seen)| handled > seen);
            if !told {
                self.close_for_grant(self.accepted.position, hand_raised);
            }
        } else if self.microphone_block.is_some()
            && let Some(seen) = self.grants.block_seen
            && !self.grants.explains(seen)
        {
            // No meeting of another presenter is shown, and the block is gone. That does
            // not tell how the block ended: by a grant, after which the person opens the
            // microphone, or without one. Only an event tells it (ADR 0785).
            if self.grants.lost_after(seen) {
                // The events of that time were not delivered. The end is taken as a
                // grant.
                let position = self.accepted.position.max(seen.saturating_add(1));
                self.close_for_unread_end(position, hand_raised);
            } else {
                // The block holds on this device until the event of its end is read.
                blocks.0.clone_from(&self.microphone_block);
                kept = true;
            }
        } else if self.microphone_block.is_some() && self.grants.block_seen.is_none() {
            // The block has no position among the events: an answer without a position
            // told it. Its end cannot be put in order with the event of a grant.
            self.close_for_unread_end(self.accepted.position, hand_raised);
        } else if behind && self.grants.blocked {
            // The events told a block that no state showed to the session, and the events
            // of its end were not delivered.
            self.close_for_unread_end(self.accepted.position, hand_raised);
        }
        self.grants.kept = kept;
        if behind && !kept {
            // Later events start from what this state shows.
            self.grants.blocked = shown;
            if !run_by_another {
                self.grants.presenter = None;
            }
        }
        self.set_blocks(blocks);
    }

    /// Returns whether the setting was closed for this grant: a grant at or before the
    /// last handled position was answered already.
    fn close_for_grant(&mut self, position: u64, hand_raised: bool) -> bool {
        if self
            .grants
            .handled
            .is_some_and(|handled| handled >= position)
        {
            return false;
        }
        self.grants.handled = Some(position);
        self.inner.shared().muted = true;
        self.inner.view.send_modify(|view| view.muted = true);
        if hand_raised && self.admitted {
            let api = self.server.api();
            self.inner.runtime.spawn(async move {
                if let Err(error) = api.lower_hand().await {
                    tracing::debug!(%error, "the hand was not lowered");
                }
            });
        }
        self.apply_controls();
        self.show();
        true
    }

    /// The server states a grant of meeting mode that the person has not answered, at
    /// `position` (ADR 0521). The event of a grant closes the setting when the session
    /// reads it; the statement closes it only when events were not delivered after the
    /// last grant that the session handled, so that the event of this grant can be among
    /// them. A grant that the session handled, or a microphone that the person opened,
    /// after `position` is not closed again.
    ///
    /// `stated` is false for a read that states no grant: the next statement of one is a
    /// grant again. A statement closes the setting once.
    fn grant_stated(&mut self, stated: bool, position: u64, hand_raised: bool) {
        if !stated {
            self.grant_closed = false;
            return;
        }
        let since = self.grants.handled.unwrap_or(0);
        if !self.grant_closed && self.grants.lost_after(since) {
            // A grant handled, or a microphone opened, at or after the position of the
            // statement answered it: nothing is closed then.
            self.close_for_grant(position, hand_raised);
            self.grant_closed = true;
        }
    }

    /// A block ended, and the session cannot read how. The end is taken as a grant: the
    /// setting is closed, and the person is told why (ADR 0785).
    fn close_for_unread_end(&mut self, position: u64, hand_raised: bool) {
        let open = !self.inner.shared().muted;
        if self.close_for_grant(position, hand_raised) && open {
            self.inner
                .view
                .send_modify(|view| view.closed = Some(MicrophoneClosed::EndOfBlockNotRead));
        }
    }

    /// Closes the person's own setting by a rule of the calls, and tells the person why.
    fn close_setting(&mut self, why: MicrophoneClosed) {
        self.inner.shared().muted = true;
        self.inner.view.send_modify(|view| {
            view.muted = true;
            view.closed = Some(why);
        });
        self.apply_controls();
        self.show();
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
            Connection::Lost(lost) => {
                self.interrupted_at
                    .get_or_insert_with(OffsetDateTime::now_utc);
                if self.retry.is_none() && !self.rejoining {
                    self.retry = Some(Instant::now() + self.backoff.take());
                    if lost == Lost::Removed
                        && self.refresh.is_none()
                        && self.removal_read != Some(state.attempt)
                    {
                        // The media server removed the device. A rule of the server can
                        // be the cause, and its event can be on its way: a new token is
                        // asked for only when a state from after the loss shows the
                        // place. One state per removed connection is asked for, also when
                        // the engine tells the same loss again after the token.
                        self.refresh = Some(fresh_state(self.server.clone()));
                        self.removal_read = Some(state.attempt);
                    }
                }
            }
        }
        if state.input_replaced && !self.inner.shared().muted {
            self.close_setting(MicrophoneClosed::DeviceChanged);
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
                        let read_at = read_at(&answer, &self.server, self.accepted);
                        let same_history = read_at.epoch == self.accepted.epoch;
                        // This answer identifies the seat requested by this renewal, even
                        // when the stream has already described that seat at a later cursor.
                        let new_seat = self.place.renewed(&answer.identity, now);
                        if new_seat {
                            self.grants = MeetingHistory::new(
                                read_at,
                                &answer.call,
                                self.grants.account_id,
                                answer.microphone_block.is_some(),
                            );
                            if answer.cursor.is_none() {
                                self.grants.unordered();
                            }
                            self.state_report = Reporter::new(self.timing.state_spacing, None);
                            self.share_report = Reporter::holding(self.timing.share_spacing, None);
                        }
                        // The answer states that the seat exists at its cursor. The newest
                        // such answer is the membership until a state is as new.
                        if new_seat || !same_history {
                            self.seat_told = None;
                        }
                        if let Some(cursor) = answer.cursor.filter(|_| same_history) {
                            if self
                                .seat_told
                                .is_none_or(|held| cursor.position >= held.position)
                            {
                                self.seat_told = Some(cursor);
                            }
                            self.place.told(cursor.position);
                            if new_seat {
                                self.place.given(cursor.position);
                            }
                        }
                        if self.place.void().is_some() {
                            // The seat is not used (ADR 0785). The call ends at the next
                            // reading, and its end gives the seat up.
                            self.admitted = false;
                            self.apply_controls();
                            return;
                        }
                        let untold = blocks_untold(&answer.call, &answer.identity);
                        // An answer with a position for the seat that the session holds
                        // tells the state of the person at that position (ADR 0785), and
                        // the admission stays as it was. Every other answer closes the
                        // microphone and the sound until a state lists the seat.
                        let tells_the_seat =
                            answer.cursor.is_some() && same_history && !new_seat && !untold;
                        if !tells_the_seat {
                            self.admitted = false;
                        }
                        if same_history && read_at.position >= self.accepted.position {
                            self.accepted = read_at;
                            self.blocks_untold = untold;
                            if !untold {
                                let own = self.grants.account_id;
                                let run_by_another = answer
                                    .call
                                    .meeting
                                    .as_ref()
                                    .is_some_and(|meeting| meeting.presenter != own);
                                let hand_raised = answer
                                    .call
                                    .participants
                                    .iter()
                                    .find(|person| person.identity == answer.identity)
                                    .is_some_and(|person| person.hand_raised);
                                if new_seat {
                                    // What the earlier seat held is not a block of this
                                    // one.
                                    self.microphone_block = None;
                                }
                                let seen = self.grants.block_seen;
                                self.take_blocks(
                                    (answer.microphone_block.clone(), answer.sound_block.clone()),
                                    run_by_another,
                                    hand_raised,
                                );
                                if let Some(stated) = answer.meeting_grant.filter(|_| !new_seat) {
                                    self.grant_stated(stated, read_at.position, hand_raised);
                                }
                                if answer.cursor.is_none() {
                                    // No order with the events: what the states showed
                                    // of the block stays as it was.
                                    self.grants.block_seen = seen;
                                }
                            }
                        } else if !same_history {
                            self.refresh = Some(fresh_state(self.server.clone()));
                        }
                        self.apply_controls();
                        self.engine.reconnect(media_join(&answer));
                        self.show();
                    }
                    Err(ApiError::Rejected(rejection))
                        if rejection.code == ErrorCode::SeatEnded =>
                    {
                        match rejection.left {
                            // The seat ended because no media connection used it. A
                            // person who is still in the call asks for a place at once.
                            Some(LeaveReason::ConnectionLost | LeaveReason::NeverConnected) => {
                                self.renew_open = true;
                                self.retry = Some(now);
                            }
                            // The seat ended by a move. The events of the move decide: the
                            // session follows the move when it reads its completion, as
                            // without the request. Nothing is asked again.
                            Some(LeaveReason::Moved) => {}
                            // Every other reason ends the call with it: a removal, a seat
                            // on another device and an end of the access are not undone
                            // (ADR 0520).
                            Some(LeaveReason::Switched) => {
                                self.fate = Fate::End(EndReason::Elsewhere);
                                self.review = false;
                            }
                            reason => {
                                self.fate = Fate::End(EndReason::Server(reason));
                                self.review = false;
                            }
                        }
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

    /// The seat that the next request for a new token names: the seat of this device,
    /// unless the server stated that it ended without a media connection, and then none, so
    /// that a place is asked for (ADR 0520). That statement serves one request.
    fn renewal_seat(&mut self) -> Option<String> {
        (!std::mem::take(&mut self.renew_open)).then(|| self.place.identity().to_owned())
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
            } else if self.fate == Fate::Held && self.refresh.is_none() {
                // A new token is asked for only while the server shows the place of this
                // device. A place that the server has ended is not asked for again: a
                // person whom a moderator removed stays out.
                self.retry = None;
                self.rejoining = true;
                self.apply_controls();
                let seat = self.renewal_seat();
                #[cfg(test)]
                self.renewals.push(seat.clone());
                drop(self.inner.in_turn(renew(
                    self.inner.clone(),
                    self.server.clone(),
                    self.target,
                    seat,
                    done.clone(),
                )));
            }
        }
        let state = self.engine.state().borrow().clone();
        if state.connection != Connection::Connected || !self.admitted {
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
        let phase = connection_phase(
            state.connection,
            self.connected_once,
            self.interrupted_at.is_some(),
            self.admitted,
        );
        let call = ActiveCall {
            camera: state.camera.clone(),
            cameras: state.cameras.clone(),
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
    fn media_connection_alone_does_not_offer_participant_actions() {
        assert_eq!(
            connection_phase(Connection::Connected, true, false, false),
            Phase::Entering
        );
        assert_eq!(
            connection_phase(Connection::Connected, true, false, true),
            Phase::Connected
        );
    }

    /// A capture that waits for the answer of the system with a timer of Tokio, as the
    /// capture of macOS does: its futures need the runtime of the client.
    struct TimedCapture;

    impl TimedCapture {
        async fn answer<T: Send + 'static>(value: T) -> Result<T, CaptureError> {
            let (answer, answered) = oneshot::channel();
            std::thread::spawn(move || {
                let _ = answer.send(value);
            });
            match tokio::time::timeout(Duration::from_secs(10), answered).await {
                Ok(Ok(value)) => Ok(value),
                Ok(Err(_)) | Err(_) => Err(CaptureError::Failed("no answer".to_owned())),
            }
        }
    }

    impl ScreenCapture for TimedCapture {
        fn access(&self) -> ScreenAccess {
            ScreenAccess::Allowed
        }
        fn request_access(&self) {}
        fn open_settings(&self) {}
        fn sources(&self) -> Pending<Result<ShareSources, CaptureError>> {
            Box::pin(Self::answer(ShareSources::default()))
        }
        fn start(
            &self,
            _: CaptureRequest,
            _: Arc<dyn crate::share::CaptureSink>,
        ) -> Pending<Result<crate::share::CaptureStream, CaptureError>> {
            Box::pin(async {
                Self::answer(()).await?;
                Err(CaptureError::Unsupported)
            })
        }
    }

    /// How long a test waits for futures that a thread without a Tokio runtime polls. A
    /// future that never ends fails the test here, and does not hold it.
    const WITHOUT_RUNTIME_LIMIT: Duration = Duration::from_secs(30);

    /// Runs `work` on a new thread, which has no Tokio runtime, and waits at most
    /// [`WITHOUT_RUNTIME_LIMIT`] for its end. The wait uses a channel of the standard
    /// library and no timer of Tokio.
    fn without_runtime<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> T {
        let (done, ended) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                assert!(tokio::runtime::Handle::try_current().is_err());
                work()
            }));
            let _ = done.send(result);
        });
        match ended.recv_timeout(WITHOUT_RUNTIME_LIMIT) {
            Ok(Ok(value)) => value,
            Ok(Err(panic)) => std::panic::resume_unwind(panic),
            Err(_) => panic!(
                "the futures did not end within {WITHOUT_RUNTIME_LIMIT:?} on a thread without \
                 a Tokio runtime"
            ),
        }
    }

    /// The media options of these tests, with this capture.
    fn options_with(capture: Arc<dyn ScreenCapture>) -> MediaOptions {
        MediaOptions {
            camera: Arc::new(crate::camera::synthetic::SyntheticCamera::new()),
            host: Arc::new(crate::audio::synthetic::SyntheticHost::with_tone(440.0)),
            permission: Arc::new(crate::permission::FixedPermission::allowed()),
            capture,
            timing: CallTiming::default(),
        }
    }

    /// A client that knows one server, whose address answers nothing: every request to it
    /// fails.
    #[allow(clippy::unwrap_used)]
    fn client_with_a_silent_server(root: &std::path::Path) -> (Client, ServerId) {
        use junto_client_core::address::ServerAddress;
        use junto_client_core::book::{ConnectionMode, ProfileSync, ServerBook, ServerEntry};
        use junto_client_core::paths::DataDir;
        let server_id = ServerId::generate();
        ServerBook::new(DataDir::open(root).unwrap())
            .add(ServerEntry {
                server_id,
                generation: 0,
                address: ServerAddress::parse("http://127.0.0.1:1").unwrap(),
                name: "Silent".into(),
                logo_path: None,
                api_versions: vec![1],
                connection_mode: ConnectionMode::Always,
                membership: Membership::Member {
                    account_id: AccountId::generate(),
                    device_id: DeviceId::generate(),
                },
                last_cursor: None,
                profile_sync: ProfileSync::NotNeeded,
                moderation_notices_shown: Vec::new(),
                logged_out: false,
            })
            .unwrap();
        let client = Client::open(junto_client_core::ClientOptions::new(root)).unwrap();
        (client, server_id)
    }

    /// The window polls the futures of the calls on the executor of gpui, which has no
    /// Tokio runtime (ADR 0733). Here a plain thread polls each of them with an executor
    /// that has no runtime either. The server of the joins is known to the client, so each
    /// join takes its turn on the runtime of the client and its future waits for the answer
    /// of that turn: a refusal, because the server answers nothing.
    #[test]
    #[allow(clippy::unwrap_used)]
    fn every_future_of_the_calls_ends_on_an_executor_without_a_tokio_runtime() {
        use junto_client_core::protocol::ids::{ChannelId, WorkspaceId};
        let directory = tempfile::Builder::new()
            .prefix("junto-calls-executor-")
            .tempdir()
            .unwrap();
        let (client, server_id) = client_with_a_silent_server(&directory.path().join("client"));
        let calls = Calls::new(client, options_with(Arc::new(TimedCapture)));
        let target = CallTarget {
            server_id,
            place: CallPlace::Channel {
                workspace_id: WorkspaceId::generate(),
                channel_id: ChannelId::generate(),
            },
        };
        without_runtime(move || {
            use futures_executor::block_on;
            assert_eq!(block_on(calls.share_sources()), Ok(ShareSources::default()));
            let joined = block_on(calls.join(target));
            assert!(matches!(joined, Err(JoinError::Refused(_))), "{joined:?}");
            let joined = block_on(calls.join_invited(target, CallInvitationId::generate()));
            assert!(matches!(joined, Err(JoinError::Refused(_))), "{joined:?}");
            block_on(calls.leave());
            // Without a call nothing asks the engine; the media live tier asks it in a call.
            assert!(block_on(calls.statistics()).is_err());
            assert!(block_on(calls.sound_check()).is_none());
        });
    }

    /// The sources are read in a task of the runtime of the client. When that runtime has
    /// shut down, a reading that waited and a reading asked for afterwards both end as
    /// cancelled, and neither panics nor waits forever.
    #[test]
    #[allow(clippy::unwrap_used)]
    fn a_reading_of_the_sources_ends_as_cancelled_when_the_runtime_has_shut_down() {
        /// A capture whose list never arrives.
        struct WaitingCapture;
        impl ScreenCapture for WaitingCapture {
            fn access(&self) -> ScreenAccess {
                ScreenAccess::Allowed
            }
            fn request_access(&self) {}
            fn open_settings(&self) {}
            fn sources(&self) -> Pending<Result<ShareSources, CaptureError>> {
                Box::pin(std::future::pending())
            }
            fn start(
                &self,
                _: CaptureRequest,
                _: Arc<dyn crate::share::CaptureSink>,
            ) -> Pending<Result<crate::share::CaptureStream, CaptureError>> {
                Box::pin(std::future::pending())
            }
        }
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .unwrap();
        let directory = tempfile::Builder::new()
            .prefix("junto-calls-shut-down-")
            .tempdir()
            .unwrap();
        // The client uses the runtime of its caller.
        let calls = {
            let _inside = runtime.enter();
            let client = Client::open(junto_client_core::ClientOptions::new(
                directory.path().join("client"),
            ))
            .unwrap();
            Calls::new(client, options_with(Arc::new(WaitingCapture)))
        };
        let waiting = calls.share_sources();
        runtime.shutdown_timeout(Duration::from_secs(10));
        without_runtime(move || {
            use futures_executor::block_on;
            for result in [block_on(waiting), block_on(calls.share_sources())] {
                assert!(
                    matches!(&result, Err(CaptureError::Failed(text)) if text.contains("cancelled")),
                    "{result:?}"
                );
            }
        });
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
            hides_occupants: None,
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
    fn an_answer_for_a_hidden_call_that_does_not_list_the_person_tells_no_blocks() {
        let own = AccountId::generate();
        let listed = |identity: &str| junto_client_core::protocol::call::CallParticipant {
            avatar: None,
            account_id: own,
            identity: identity.to_owned(),
            joined_at: OffsetDateTime::UNIX_EPOCH,
            muted: false,
            deafened: false,
            microphone_block: None,
            sound_block: None,
            hand_raised: false,
            screen_share: None,
        };
        let call = |hides_occupants, participants| Call {
            id: junto_client_core::protocol::ids::CallId::generate(),
            place: CallPlace::direct(own, AccountId::generate()),
            started_at: None,
            lock: None,
            meeting: None,
            hides_occupants,
            participants,
            recording: None,
        };
        // A new place in a call that hides who is inside: the server names nobody.
        assert!(blocks_untold(&call(true, Vec::new()), "ours"));
        // The account is listed with the place of another device.
        assert!(blocks_untold(&call(true, vec![listed("other")]), "ours"));
        // The place of a device that the server has seen: the answer lists it.
        assert!(!blocks_untold(&call(true, vec![listed("ours")]), "ours"));
        // A call that shows who is inside states the blocks in every answer.
        assert!(!blocks_untold(&call(false, Vec::new()), "ours"));
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

#[cfg(test)]
#[path = "calls/ordering_tests.rs"]
mod ordering_tests;
