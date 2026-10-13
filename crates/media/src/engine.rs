//! The media connection of one call (ADR 0733): the room of the media server, the
//! microphone track of this device, the tracks of the other people, and the two sound
//! devices.
//!
//! One task owns all of it. A caller changes it through [`Engine`] and reads two values
//! that the task keeps current: the state and the levels. The engine knows no rule of a
//! server: who is in a call, who is blocked and when a call ends is told to it
//! (see [`crate::calls`]).

use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use futures_util::StreamExt;
use junto_client_core::protocol::ids::AccountId;
use junto_client_core::protocol::secret::CallToken;
use livekit::DisconnectReason;
use livekit::options::TrackPublishOptions;
use livekit::prelude::{
    LocalAudioTrack, LocalTrack, LocalTrackPublication, Participant, RemoteTrack, Room, RoomEvent,
    RoomOptions, TrackSid, TrackSource,
};
use livekit::webrtc::audio_frame::AudioFrame;
use livekit::webrtc::audio_source::native::NativeAudioSource;
use livekit::webrtc::audio_source::{AudioSourceOptions, RtcAudioSource};
use livekit::webrtc::audio_stream::native::NativeAudioStream;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinHandle;
use tokio::time::Instant;

use crate::audio::capture::{CaptureChain, CaptureLink, Frame, FrameQueue};
use crate::audio::host::usable;
use crate::audio::level::{self, LevelCell, Speaking};
use crate::audio::playout::{MAX_GAIN, Mixer, PlayoutChain};
use crate::audio::processing::Processor;
use crate::audio::{
    AudioError, AudioHost, DeviceId, DeviceInfo, FRAME_SAMPLES, OpenStream, PLAYOUT_CHANNELS,
    Processing, SAMPLE_RATE,
};
use crate::permission::{MicrophoneAccess, MicrophonePermission};
use crate::share::picture::PictureFrame;
use crate::share::{CaptureError, CaptureRequest, ScreenCapture};

mod order;
mod share;
#[cfg(feature = "test-support")]
pub(crate) mod statistics;

pub use share::{
    DEFAULT_VIEW, MainPicture, SMALL_PICTURE, SeenShare, ShareId, ShareInfo, ShareState, watched,
};
use share::{OwnShare, Pictures, RemotePicture, RemoteSound, ShareTrack, View};

/// The frames of the microphone that wait for the media connection: half a second.
const WAITING_FRAMES: usize = 50;
/// How often the levels are told.
const LEVEL_INTERVAL: Duration = Duration::from_millis(50);
/// A publication of the microphone that the media server refused is tried again after
/// this time, while the microphone may be sent.
const PUBLISH_RETRY: Duration = Duration::from_secs(1);
/// How long one publication may take.
const PUBLISH_LIMIT: Duration = Duration::from_secs(10);
/// The number of the microphone among the track sources in the protocol of the media
/// server (`TrackSource.MICROPHONE`). A permission lists the sources that a participant may
/// send by these numbers; an empty list allows every source.
const MICROPHONE_SOURCE: i32 = 2;
/// How long the media server has to confirm that this device leaves a room. The devices
/// are closed before, and the engine does not wait longer for a server that does not answer.
const ROOM_CLOSE_LIMIT: Duration = Duration::from_secs(3);
/// A device that fails is opened again once. A stream that the engine opened in this way
/// and that fails again within this time stays closed until the person acts.
const RECOVERY_SPAN: Duration = Duration::from_secs(5);

/// What a device needs to enter the room of a call: the answer of the server to a join
/// (ADR 0508).
#[derive(Debug, Clone)]
pub struct MediaJoin {
    pub url: String,
    pub token: CallToken,
}

/// The sound settings of the person (ADR 0701, ADR 0507).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct VoiceConfig {
    /// The chosen devices. Absent: the default of the system.
    pub input_device: Option<DeviceId>,
    pub output_device: Option<DeviceId>,
    pub processing: Processing,
    /// The microphone is sent only while the key is held. Without it the open microphone
    /// is sent all the time.
    pub push_to_talk: bool,
}

/// What the person and the server have set for this device in the call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Controls {
    /// The person closed the microphone.
    pub muted: bool,
    /// The person turned the sound off.
    pub deafened: bool,
    /// The server closed every sound source of the person (ADR 0510).
    pub microphone_blocked: bool,
    /// A moderator blocked the sound. The client applies it (ADR 0503).
    pub sound_blocked: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lost {
    /// No connection was made, or it ended and the call library gave up.
    Failed,
    /// The media server removed this participant or closed the room.
    Removed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Connection {
    Connecting,
    Connected,
    /// The connection is interrupted and the call library restores it.
    Reconnecting,
    /// The connection has ended. It starts again only with a new token.
    Lost(Lost),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MicrophoneState {
    /// The microphone is not open: there is no connection, or the server or the media
    /// server does not let this person send it.
    Closed,
    /// The system asks the person for the permission.
    Asking,
    /// The microphone is open. Its frames are sent, or silence while it is muted.
    Running,
    /// The system does not allow this application to use a microphone.
    NoPermission,
    Failed(AudioError),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutputState {
    Closed,
    Running,
    Failed(AudioError),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineState {
    pub connection: Connection,
    pub microphone: MicrophoneState,
    /// The microphone permission as the system held it when the engine last read it.
    pub access: MicrophoneAccess,
    /// The microphone track is published in the room.
    pub published: bool,
    pub input_device: Option<DeviceInfo>,
    pub output: OutputState,
    pub output_device: Option<DeviceInfo>,
    /// The share of this device.
    pub share: ShareState,
    /// The shares of the other people, in the order in which they were seen.
    pub shares: Vec<SeenShare>,
    /// The share whose picture and sound the person gets now.
    pub watched: Option<ShareId>,
}

/// The level of one person: the share of a meter from 0 to 1, and whether the person
/// speaks.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct PersonLevel {
    pub level: f32,
    pub speaking: bool,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Levels {
    /// The microphone after the processing, also while it is muted.
    pub input: f32,
    /// What this device sends.
    pub own: PersonLevel,
    /// The other people whose sound arrives.
    pub people: HashMap<AccountId, PersonLevel>,
}

/// The devices, the permission and the screen capture that an engine uses.
#[derive(Clone)]
pub struct EngineOptions {
    pub host: Arc<dyn AudioHost>,
    pub permission: Arc<dyn MicrophonePermission>,
    pub capture: Arc<dyn ScreenCapture>,
}

/// What the person set for the shares of a call.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Watching {
    /// The share that the person chose to watch. Absent: no choice was made.
    pub choice: Option<ShareId>,
    /// The volume of the sound of the watched share: 1.0 is 100 %, and 0.0 is its mute.
    pub volume: f32,
}

impl Default for Watching {
    fn default() -> Self {
        Self {
            choice: None,
            volume: 1.0,
        }
    }
}

enum Command {
    Controls(Controls),
    Talking(bool),
    Volume(AccountId, f32),
    Config(VoiceConfig),
    Reconnect(MediaJoin),
    RetryMicrophone,
    StartShare(CaptureRequest),
    /// Ends the own share, and puts away the statement that a share ended by itself.
    StopShare,
    Watch(Option<ShareId>),
    ShareVolume(f32),
    ViewSize(u32, u32),
    /// Makes the call library connect anew and publish its tracks again, as it does by
    /// itself after some interruptions.
    #[cfg(feature = "test-support")]
    SimulateFullReconnect,
    #[cfg(feature = "test-support")]
    Statistics(statistics::Reply),
    #[cfg(feature = "test-support")]
    SoundCheck(statistics::SoundReply),
    Close(oneshot::Sender<()>),
}

type Events = mpsc::UnboundedReceiver<RoomEvent>;

enum Internal {
    Connected(u64, Result<(Room, Events), String>),
    Permission(MicrophoneAccess),
    InputFailed(u64, AudioError),
    OutputFailed(u64, AudioError),
    Published(u64, Result<LocalTrackPublication, String>),
    PublishRetry(u64),
    /// The capture of a share has answered. The share holds the answer.
    ShareStarted(u64),
    ShareEnded(u64, CaptureError),
    SharePicturePublished(u64, Result<LocalTrackPublication, String>),
    ShareSoundPublished((u64, u64), Result<LocalTrackPublication, String>),
    ShareSoundRetry(u64),
    ShareTrackDue(u64, u64, ShareTrack),
    /// The time for the microphone track to return has passed.
    MicrophoneDue(u64),
    SharePictureSize(TrackSid, (u32, u32)),
}

/// The media connection of one call. Dropping it ends the connection.
pub struct Engine {
    commands: mpsc::UnboundedSender<Command>,
    state: watch::Receiver<EngineState>,
    levels: watch::Receiver<Levels>,
    main_picture: watch::Receiver<Option<MainPicture>>,
    small_pictures: watch::Receiver<HashMap<ShareId, Arc<PictureFrame>>>,
}

impl Engine {
    /// Starts the connection on `runtime`. The result is in [`Engine::state`].
    pub fn start(
        runtime: &tokio::runtime::Handle,
        options: EngineOptions,
        join: MediaJoin,
        config: VoiceConfig,
        controls: Controls,
        volumes: HashMap<AccountId, f32>,
        watching: Watching,
    ) -> Self {
        let access = options.permission.state();
        let (state, state_seen) = watch::channel(EngineState {
            connection: Connection::Connecting,
            microphone: MicrophoneState::Closed,
            access,
            published: false,
            input_device: None,
            output: OutputState::Closed,
            output_device: None,
            share: ShareState::Off,
            shares: Vec::new(),
            watched: None,
        });
        let pictures = Pictures {
            main: Arc::new(watch::Sender::new(None)),
            small: Arc::new(watch::Sender::new(HashMap::new())),
        };
        let (main_picture, small_pictures) =
            (pictures.main.subscribe(), pictures.small.subscribe());
        let (levels, levels_seen) = watch::channel(Levels::default());
        let (commands, taken) = mpsc::unbounded_channel();
        let (internal, internal_taken) = mpsc::unbounded_channel();
        let (frames, frames_taken) = mpsc::channel(WAITING_FRAMES);
        let source: Arc<Mutex<Option<NativeAudioSource>>> = Arc::default();
        let link = Arc::new(CaptureLink::default());
        let pump = runtime.spawn(pump(
            FrameQueue::new(link.clone(), frames_taken),
            source.clone(),
        ));
        let actor = Actor {
            runtime: runtime.clone(),
            options,
            config: config.clone(),
            controls,
            talking: false,
            volumes,
            state,
            levels,
            internal,
            processor: Arc::new(Processor::new(config.processing)),
            mixer: Mixer::shared(),
            link,
            frames,
            source,
            pump,
            room: None,
            #[cfg(feature = "test-support")]
            connected_at: None,
            attempt: 0,
            capture: None,
            capture_generation: 0,
            capture_recovered: None,
            capture_stopped: false,
            asking: false,
            playout: None,
            playout_generation: 0,
            playout_recovered: None,
            publication: Publication::None,
            publish_generation: 0,
            microphone_returns: None,
            remotes: HashMap::new(),
            share: None,
            share_generation: 0,
            remote_pictures: HashMap::new(),
            remote_sounds: HashMap::new(),
            share_order: 0,
            watch_choice: watching.choice,
            share_volume: watching.volume.clamp(0.0, MAX_GAIN),
            view_size: DEFAULT_VIEW,
            view: watch::Sender::new(View {
                watched: None,
                size: DEFAULT_VIEW,
            }),
            pictures,
        };
        runtime.spawn(actor.run(join, taken, internal_taken));
        Self {
            commands,
            state: state_seen,
            levels: levels_seen,
            main_picture,
            small_pictures,
        }
    }

    /// The picture of the share that the person watches, at the size of its drawing.
    pub fn main_picture(&self) -> watch::Receiver<Option<MainPicture>> {
        self.main_picture.clone()
    }

    /// A small picture of each share of the call, made again about once a second.
    pub fn small_pictures(&self) -> watch::Receiver<HashMap<ShareId, Arc<PictureFrame>>> {
        self.small_pictures.clone()
    }

    /// Starts the share of this device. The result is in [`EngineState::share`].
    pub fn start_share(&self, request: CaptureRequest) {
        let _ = self.commands.send(Command::StartShare(request));
    }

    /// Ends the share of this device, and puts away the statement that one ended.
    pub fn stop_share(&self) {
        let _ = self.commands.send(Command::StopShare);
    }

    /// The share that the person chose to watch.
    pub fn watch(&self, choice: Option<ShareId>) {
        let _ = self.commands.send(Command::Watch(choice));
    }

    /// The volume of the sound of the watched share: 1.0 is 100 %.
    pub fn set_share_volume(&self, gain: f32) {
        let _ = self.commands.send(Command::ShareVolume(gain));
    }

    /// The size in pixels inside which the interface draws the watched share.
    pub fn set_view_size(&self, width: u32, height: u32) {
        let _ = self.commands.send(Command::ViewSize(width, height));
    }

    /// Makes the call library connect anew and publish its tracks again, for a test.
    #[cfg(feature = "test-support")]
    pub fn simulate_full_reconnect(&self) {
        let _ = self.commands.send(Command::SimulateFullReconnect);
    }

    pub fn state(&self) -> watch::Receiver<EngineState> {
        self.state.clone()
    }

    pub fn levels(&self) -> watch::Receiver<Levels> {
        self.levels.clone()
    }

    pub fn set_controls(&self, controls: Controls) {
        let _ = self.commands.send(Command::Controls(controls));
    }

    /// The push-to-talk key is held or released.
    pub fn set_talking(&self, talking: bool) {
        let _ = self.commands.send(Command::Talking(talking));
    }

    /// The volume of one person: 1.0 is 100 %, and 2.0 is the most.
    pub fn set_volume(&self, account_id: AccountId, gain: f32) {
        let _ = self.commands.send(Command::Volume(account_id, gain));
    }

    pub fn set_config(&self, config: VoiceConfig) {
        let _ = self.commands.send(Command::Config(config));
    }

    /// Connects again with a new token, after the connection was lost.
    pub fn reconnect(&self, join: MediaJoin) {
        let _ = self.commands.send(Command::Reconnect(join));
    }

    /// Opens the microphone again after it failed or was refused: the person changed the
    /// permission or connected a device.
    pub fn retry_microphone(&self) {
        let _ = self.commands.send(Command::RetryMicrophone);
    }

    /// Leaves the room and closes the devices.
    pub fn close(self) -> impl Future<Output = ()> + Send + 'static {
        let (done, closed) = oneshot::channel();
        let _ = self.commands.send(Command::Close(done));
        async move {
            let _ = closed.await;
        }
    }
}

/// Gives the frames of the microphone to the audio source of the published track.
async fn pump(mut frames: FrameQueue, source: Arc<Mutex<Option<NativeAudioSource>>>) {
    while let Some(samples) = frames.next().await {
        let current = source
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let Some(current) = current else {
            continue;
        };
        let frame = AudioFrame {
            data: samples.into(),
            sample_rate: SAMPLE_RATE,
            num_channels: 1,
            samples_per_channel: FRAME_SAMPLES as u32,
        };
        match current.capture_frame(&frame).await {
            Ok(()) => frames.taken(),
            Err(error) => {
                tracing::debug!(error = %error.message, "a frame of the microphone was not taken");
            }
        }
    }
}

/// The account of a participant: the first part of its identity (ADR 0508).
pub fn account_of(identity: &str) -> Option<AccountId> {
    identity.split(':').next()?.parse().ok()
}

enum Publication {
    None,
    Publishing,
    Published(LocalTrackPublication),
}

/// One remote audio track that is played.
struct Remote {
    account_id: Option<AccountId>,
    level: Arc<LevelCell>,
    speaking: Arc<std::sync::atomic::AtomicBool>,
    gain: watch::Sender<f32>,
    task: JoinHandle<()>,
}

impl Drop for Remote {
    fn drop(&mut self) {
        self.task.abort();
    }
}

struct Actor {
    runtime: tokio::runtime::Handle,
    options: EngineOptions,
    config: VoiceConfig,
    controls: Controls,
    talking: bool,
    volumes: HashMap<AccountId, f32>,
    state: watch::Sender<EngineState>,
    levels: watch::Sender<Levels>,
    internal: mpsc::UnboundedSender<Internal>,
    processor: Arc<Processor>,
    mixer: Arc<Mixer>,
    link: Arc<CaptureLink>,
    frames: mpsc::Sender<Frame>,
    source: Arc<Mutex<Option<NativeAudioSource>>>,
    pump: JoinHandle<()>,
    room: Option<Room>,
    /// When the media connection was made, for the diagnosis of a live test.
    #[cfg(feature = "test-support")]
    connected_at: Option<Instant>,
    /// Counts the connections that were started. The result of an earlier one is not used.
    attempt: u64,
    capture: Option<OpenStream>,
    capture_generation: u64,
    /// When the engine opened the microphone again after a failure of its stream.
    capture_recovered: Option<Instant>,
    /// The microphone failed and is opened again only when the person acts.
    capture_stopped: bool,
    asking: bool,
    playout: Option<OpenStream>,
    playout_generation: u64,
    playout_recovered: Option<Instant>,
    publication: Publication,
    publish_generation: u64,
    /// The identifier of the microphone track that the call library withdrew and
    /// publishes again.
    microphone_returns: Option<TrackSid>,
    remotes: HashMap<TrackSid, Remote>,
    /// The share of this device.
    share: Option<OwnShare>,
    /// Counts the shares that were started. The result of an earlier one is not used.
    share_generation: u64,
    remote_pictures: HashMap<TrackSid, RemotePicture>,
    remote_sounds: HashMap<TrackSid, RemoteSound>,
    share_order: u64,
    watch_choice: Option<ShareId>,
    share_volume: f32,
    /// The size in pixels inside which the interface draws the watched share.
    view_size: (u32, u32),
    view: watch::Sender<View>,
    pictures: Pictures,
}

fn recent(at: Option<Instant>) -> bool {
    at.is_some_and(|at| at.elapsed() < RECOVERY_SPAN)
}

/// Leaves a room. The call library waits for the media server to confirm the leave.
async fn close_room(room: Room) {
    match tokio::time::timeout(ROOM_CLOSE_LIMIT, room.close()).await {
        Ok(Ok(())) => {}
        Ok(Err(error)) => tracing::debug!(%error, "the room was not left in order"),
        Err(_) => tracing::warn!("the media server did not confirm that the room was left"),
    }
}

async fn next_event(events: &mut Option<Events>) -> Option<RoomEvent> {
    match events {
        Some(events) => events.recv().await,
        None => std::future::pending().await,
    }
}

impl Actor {
    async fn run(
        mut self,
        join: MediaJoin,
        mut commands: mpsc::UnboundedReceiver<Command>,
        mut internal: mpsc::UnboundedReceiver<Internal>,
    ) {
        let mut events: Option<Events> = None;
        let mut tick = tokio::time::interval(LEVEL_INTERVAL);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        self.connect(join);
        let done = loop {
            tokio::select! {
                command = commands.recv() => match command {
                    Some(Command::Close(done)) => break Some(done),
                    Some(command) => self.command(command, &mut events).await,
                    // The engine was dropped.
                    None => break None,
                },
                Some(message) = internal.recv() => self.internal(message, &mut events).await,
                event = next_event(&mut events) => match event {
                    Some(event) => self.event(event, &mut events).await,
                    None => self.lost(Lost::Failed, &mut events).await,
                },
                _ = tick.tick() => self.tell_levels(),
            }
        };
        self.leave(&mut events).await;
        self.pump.abort();
        if let Some(done) = done {
            let _ = done.send(());
        }
    }

    fn change(&self, change: impl FnOnce(&mut EngineState)) {
        self.state.send_if_modified(|state| {
            let before = state.clone();
            change(state);
            *state != before
        });
    }

    fn connection(&self) -> Connection {
        self.state.borrow().connection
    }

    fn connect(&mut self, join: MediaJoin) {
        self.attempt += 1;
        let attempt = self.attempt;
        self.change(|state| state.connection = Connection::Connecting);
        let internal = self.internal.clone();
        self.runtime.spawn(async move {
            let result = Room::connect(&join.url, join.token.expose(), RoomOptions::default())
                .await
                .map_err(|error| error.to_string());
            if let Err(Internal::Connected(_, Ok((room, _)))) = internal
                .send(Internal::Connected(attempt, result))
                .map_err(|unsent| unsent.0)
            {
                // The engine ended while the connection was made.
                close_room(room).await;
            }
        });
    }

    async fn command(&mut self, command: Command, events: &mut Option<Events>) {
        match command {
            Command::Controls(controls) => {
                let before = std::mem::replace(&mut self.controls, controls);
                if before.microphone_blocked != controls.microphone_blocked {
                    self.sync_microphone();
                }
                self.sync_gate();
                self.sync_sound();
            }
            Command::Talking(talking) => {
                self.talking = talking;
                self.sync_gate();
            }
            Command::Volume(account_id, gain) => {
                let gain = gain.clamp(0.0, MAX_GAIN);
                self.volumes.insert(account_id, gain);
                for remote in self.remotes.values() {
                    if remote.account_id == Some(account_id) {
                        remote.gain.send_replace(gain);
                    }
                }
            }
            Command::Config(config) => {
                let before = std::mem::replace(&mut self.config, config);
                self.processor.set(self.config.processing);
                // A changed choice opens the device again, also after a failure: the
                // person may have chosen a device that works.
                if before.input_device != self.config.input_device {
                    self.close_capture();
                    self.retry_capture();
                }
                // An output that failed is also opened again by a choice that did not
                // change: the person asks for the device again.
                let failed = matches!(self.state.borrow().output, OutputState::Failed(_));
                if (before.output_device != self.config.output_device || failed) && self.joined() {
                    self.playout_recovered = None;
                    self.open_playout();
                }
                self.sync_gate();
            }
            Command::Reconnect(join) => {
                self.leave(events).await;
                self.connect(join);
            }
            Command::RetryMicrophone => {
                if self.capture.is_none() {
                    self.change(|state| state.microphone = MicrophoneState::Closed);
                    self.retry_capture();
                }
            }
            Command::StartShare(request) => self.start_share(request),
            Command::StopShare => self.end_share(None),
            Command::Watch(choice) => self.set_watch(choice),
            Command::ShareVolume(gain) => self.set_share_volume(gain),
            Command::ViewSize(width, height) => self.set_view_size(width, height),
            #[cfg(feature = "test-support")]
            Command::Statistics(reply) => self.statistics(reply).await,
            #[cfg(feature = "test-support")]
            Command::SoundCheck(reply) => self.sound_check(reply),
            #[cfg(feature = "test-support")]
            Command::SimulateFullReconnect => {
                if let Some(room) = &self.room {
                    let scenario = livekit::SimulateScenario::FullReconnect;
                    if let Err(error) = room.simulate_scenario(scenario).await {
                        tracing::warn!(%error, "the full reconnect was not started");
                    }
                }
            }
            // The loop takes this command itself.
            Command::Close(_) => {}
        }
    }

    async fn internal(&mut self, message: Internal, events: &mut Option<Events>) {
        match message {
            Internal::Connected(attempt, result) => {
                if attempt != self.attempt {
                    if let Ok((room, _)) = result {
                        close_room(room).await;
                    }
                    return;
                }
                match result {
                    Ok((room, taken)) => {
                        self.room = Some(room);
                        *events = Some(taken);
                        #[cfg(feature = "test-support")]
                        {
                            self.connected_at = Some(Instant::now());
                        }
                        self.change(|state| state.connection = Connection::Connected);
                        self.open_playout();
                        self.sync_microphone();
                    }
                    Err(error) => {
                        tracing::warn!(%error, "the media connection of the call was not made");
                        self.change(|state| state.connection = Connection::Lost(Lost::Failed));
                    }
                }
            }
            Internal::Permission(access) => {
                self.asking = false;
                self.change(|state| state.access = access);
                self.sync_microphone();
            }
            Internal::InputFailed(generation, error) => {
                if generation == self.capture_generation && self.capture.is_some() {
                    tracing::warn!(%error, "the microphone stopped");
                    self.close_capture();
                    if recent(self.capture_recovered) {
                        self.capture_stopped = true;
                        self.change(|state| state.microphone = MicrophoneState::Failed(error));
                        self.sync_publication();
                    } else {
                        // The list of devices has changed: the open goes to the default
                        // when the chosen device is gone.
                        self.capture_recovered = Some(Instant::now());
                        self.sync_microphone();
                    }
                }
            }
            Internal::OutputFailed(generation, error) => {
                if generation == self.playout_generation && self.playout.is_some() {
                    tracing::warn!(%error, "the sound output stopped");
                    if recent(self.playout_recovered) {
                        self.close_playout();
                        self.change(|state| state.output = OutputState::Failed(error));
                    } else {
                        self.playout_recovered = Some(Instant::now());
                        self.open_playout();
                    }
                }
            }
            Internal::Published(generation, result) => self.published(generation, result),
            Internal::PublishRetry(generation) => {
                if generation == self.publish_generation {
                    self.sync_publication();
                }
            }
            Internal::ShareStarted(generation) => self.share_started(generation),
            Internal::ShareEnded(generation, error) => self.share_ended(generation, error),
            Internal::SharePicturePublished(generation, result) => {
                self.share_picture_published(generation, result);
            }
            Internal::ShareSoundPublished(key, result) => self.share_sound_published(key, result),
            Internal::ShareSoundRetry(generation) => self.share_sound_retry(generation),
            Internal::ShareTrackDue(generation, wait, track) => {
                self.share_track_due(generation, wait, track);
            }
            Internal::MicrophoneDue(generation) => {
                if generation == self.publish_generation && self.microphone_returns.is_some() {
                    // The track did not return: it is published anew while the
                    // microphone may be sent.
                    self.forget_publication();
                    self.sync_publication();
                }
            }
            Internal::SharePictureSize(sid, size) => self.picture_size(&sid, size),
        }
    }

    async fn event(&mut self, event: RoomEvent, events: &mut Option<Events>) {
        match event {
            RoomEvent::Reconnecting => {
                self.change(|state| state.connection = Connection::Reconnecting);
            }
            RoomEvent::Reconnected => {
                self.change(|state| state.connection = Connection::Connected);
                self.sync_publication();
            }
            RoomEvent::Disconnected { reason } => {
                let lost = match reason {
                    DisconnectReason::ParticipantRemoved
                    | DisconnectReason::RoomDeleted
                    | DisconnectReason::RoomClosed
                    | DisconnectReason::DuplicateIdentity => Lost::Removed,
                    _ => Lost::Failed,
                };
                tracing::info!(?reason, "the media connection of the call ended");
                self.lost(lost, events).await;
            }
            RoomEvent::TrackSubscribed {
                track: RemoteTrack::Video(track),
                publication,
                participant,
            } => {
                // A camera is not shown in this version.
                if publication.source() == TrackSource::Screenshare {
                    self.remote_picture(&track, publication, participant.identity().as_str());
                }
            }
            RoomEvent::TrackSubscribed {
                track: RemoteTrack::Audio(track),
                publication,
                participant,
            } if publication.source() == TrackSource::ScreenshareAudio => {
                self.remote_sound(&track, publication, participant.identity().as_str());
            }
            RoomEvent::TrackSubscribed {
                track: RemoteTrack::Audio(track),
                publication,
                participant,
            } => {
                let sid = publication.sid();
                let account_id = account_of(participant.identity().as_str());
                let gain = account_id
                    .and_then(|account_id| self.volumes.get(&account_id).copied())
                    .unwrap_or(1.0);
                let (level, speaking) = (LevelCell::shared(), Arc::default());
                let (gain, gains) = watch::channel(gain);
                let stream = NativeAudioStream::new(
                    track.rtc_track(),
                    SAMPLE_RATE as i32,
                    PLAYOUT_CHANNELS as i32,
                );
                let task = self.runtime.spawn(play(
                    stream,
                    self.mixer.clone(),
                    gains,
                    level.clone(),
                    Arc::clone(&speaking),
                ));
                self.remotes.insert(
                    sid,
                    Remote {
                        account_id,
                        level,
                        speaking,
                        gain,
                        task,
                    },
                );
            }
            RoomEvent::TrackUnsubscribed { publication, .. }
            | RoomEvent::TrackUnpublished { publication, .. } => {
                let sid = publication.sid();
                if !self.remote_track_ended(&sid) {
                    self.remotes.remove(&sid);
                }
            }
            RoomEvent::LocalTrackUnpublished { publication, .. } => {
                // The track was withdrawn, and this device did not ask for it. The call
                // library does that when it connects anew, and publishes the same track
                // again; the media server does it when the permission to send the track
                // is gone. The microphone waits for its track for a bounded time, with
                // the audio source that the track is bound to.
                let sid = publication.sid();
                if let Publication::Published(held) = &self.publication
                    && held.sid() == sid
                {
                    self.publish_generation += 1;
                    self.publication = Publication::Publishing;
                    self.microphone_returns = Some(sid);
                    self.change(|state| state.published = false);
                    let (generation, internal) = (self.publish_generation, self.internal.clone());
                    self.runtime.spawn(async move {
                        tokio::time::sleep(PUBLISH_LIMIT).await;
                        let _ = internal.send(Internal::MicrophoneDue(generation));
                    });
                } else {
                    self.share_track_taken(&sid);
                }
            }
            RoomEvent::LocalTrackRepublished {
                previous_sid,
                publication,
                track,
                ..
            } => {
                // After a new connection the call library published the track again under
                // another identifier.
                if matches!(track, LocalTrack::Audio(_))
                    && publication.source() == TrackSource::Microphone
                {
                    let holds = match &self.publication {
                        Publication::Published(held) => Some(held.sid()),
                        Publication::None | Publication::Publishing => None,
                    };
                    let waits_for = self.microphone_returns.as_ref();
                    if share::is_return(waits_for, holds.as_ref(), &previous_sid) {
                        self.microphone_returns = None;
                        self.publication = Publication::Published(publication);
                        self.change(|state| state.published = true);
                        self.sync_gate();
                        // The microphone can have been closed while the track was away.
                        self.sync_publication();
                    } else {
                        // The microphone was closed, or has a new track.
                        self.withdraw(publication);
                    }
                } else {
                    self.share_track_republished(&previous_sid, publication);
                }
            }
            RoomEvent::ParticipantPermissionChanged {
                participant: Participant::Local(_),
                ..
            } => {
                self.sync_microphone();
                self.sync_share_sound();
            }
            _ => {}
        }
    }

    /// The connection ended. The devices close, and the engine waits for a new token.
    async fn lost(&mut self, lost: Lost, events: &mut Option<Events>) {
        self.leave(events).await;
        self.change(|state| state.connection = Connection::Lost(lost));
    }

    /// Leaves the room, if there is one, and closes the devices.
    async fn leave(&mut self, events: &mut Option<Events>) {
        self.attempt += 1;
        // A share does not outlive the connection that carried it (ADR 0737).
        if self.share.is_some() {
            self.end_share(Some(CaptureError::Interrupted));
        }
        self.forget_remote_shares();
        self.tell_shares();
        self.remotes.clear();
        self.forget_publication();
        self.close_capture();
        self.close_playout();
        (self.capture_recovered, self.playout_recovered) = (None, None);
        self.capture_stopped = false;
        *events = None;
        if let Some(room) = self.room.take() {
            close_room(room).await;
        }
        self.change(|state| {
            state.microphone = MicrophoneState::Closed;
        });
    }

    fn joined(&self) -> bool {
        self.room.is_some()
            && matches!(
                self.connection(),
                Connection::Connected | Connection::Reconnecting
            )
    }

    /// The media server lets this participant send a track of a source. A server closes the
    /// microphone of a person, and the sound of a share with it, with this permission, in
    /// the token of the join and in the room (ADR 0510), so it holds from the first moment
    /// of a connection, before the state of the server has told the block.
    fn may_send(&self, source: i32) -> bool {
        let Some(room) = &self.room else {
            return false;
        };
        room.local_participant()
            .permission()
            .is_none_or(|permission| {
                permission.can_publish
                    && (permission.can_publish_sources.is_empty()
                        || permission.can_publish_sources.contains(&source))
            })
    }

    fn microphone_wanted(&self) -> bool {
        self.joined() && !self.controls.microphone_blocked && self.may_send(MICROPHONE_SOURCE)
    }

    /// The person asks for the microphone again: after a failure, or with another device.
    fn retry_capture(&mut self) {
        self.capture_stopped = false;
        self.capture_recovered = None;
        self.sync_microphone();
    }

    /// Opens or closes the microphone by what holds now: it is open while the device is in
    /// the room and neither the server nor the media server blocks it.
    fn sync_microphone(&mut self) {
        if !self.microphone_wanted() {
            self.close_capture();
            self.capture_stopped = false;
            self.change(|state| state.microphone = MicrophoneState::Closed);
            self.sync_publication();
            return;
        }
        if self.capture.is_some() || self.asking || self.capture_stopped {
            self.sync_publication();
            return;
        }
        match self.options.permission.state() {
            MicrophoneAccess::Allowed => {
                self.change(|state| state.access = MicrophoneAccess::Allowed);
                self.open_capture();
            }
            MicrophoneAccess::NotAsked => {
                self.asking = true;
                self.change(|state| {
                    state.access = MicrophoneAccess::NotAsked;
                    state.microphone = MicrophoneState::Asking;
                });
                let (asked, internal) = (self.options.permission.request(), self.internal.clone());
                self.runtime.spawn(async move {
                    let _ = internal.send(Internal::Permission(asked.await));
                });
            }
            MicrophoneAccess::Refused => {
                self.capture_stopped = true;
                self.change(|state| {
                    state.access = MicrophoneAccess::Refused;
                    state.microphone = MicrophoneState::NoPermission;
                });
            }
        }
        self.sync_publication();
    }

    fn open_capture(&mut self) {
        self.capture_generation += 1;
        let generation = self.capture_generation;
        let connected = self.options.host.devices().unwrap_or_default();
        let chosen = usable(self.config.input_device.as_ref(), &connected.inputs);
        let mut chain = CaptureChain::new(
            self.link.clone(),
            self.processor.clone(),
            self.frames.clone(),
        );
        let internal = self.internal.clone();
        let opened = self.options.host.open_input(
            chosen,
            Box::new(move |samples, format| chain.deliver(samples, format)),
            Box::new(move |error| {
                let _ = internal.send(Internal::InputFailed(generation, error));
            }),
        );
        match opened {
            Ok(stream) => {
                let device = stream.device.clone();
                self.capture = Some(stream);
                self.change(|state| {
                    state.microphone = MicrophoneState::Running;
                    state.input_device = Some(device);
                });
                self.sync_gate();
            }
            Err(error) => {
                tracing::warn!(%error, "the microphone was not opened");
                self.capture_stopped = true;
                self.change(|state| {
                    state.microphone = if error == AudioError::Refused {
                        MicrophoneState::NoPermission
                    } else {
                        MicrophoneState::Failed(error)
                    };
                    state.input_device = None;
                });
            }
        }
    }

    fn close_capture(&mut self) {
        self.capture_generation += 1;
        if self.capture.take().is_some() {
            self.link.set_open(false);
            self.link.closed();
            self.change(|state| state.input_device = None);
        }
    }

    fn open_playout(&mut self) {
        self.playout = None;
        self.playout_generation += 1;
        let generation = self.playout_generation;
        let connected = self.options.host.devices().unwrap_or_default();
        let chosen = usable(self.config.output_device.as_ref(), &connected.outputs);
        let mut chain = PlayoutChain::new(self.mixer.clone(), self.processor.clone());
        let internal = self.internal.clone();
        let opened = self.options.host.open_output(
            chosen,
            Box::new(move |samples, format| chain.fill(samples, format)),
            Box::new(move |error| {
                let _ = internal.send(Internal::OutputFailed(generation, error));
            }),
        );
        self.sync_sound();
        match opened {
            Ok(stream) => {
                let device = stream.device.clone();
                self.playout = Some(stream);
                self.change(|state| {
                    state.output = OutputState::Running;
                    state.output_device = Some(device);
                });
            }
            Err(error) => {
                tracing::warn!(%error, "the sound output was not opened");
                self.change(|state| {
                    state.output = OutputState::Failed(error);
                    state.output_device = None;
                });
            }
        }
    }

    fn close_playout(&mut self) {
        self.playout_generation += 1;
        if self.playout.take().is_some() {
            self.change(|state| {
                state.output = OutputState::Closed;
                state.output_device = None;
            });
        }
    }

    fn sync_sound(&self) {
        self.mixer
            .set_sound_off(self.controls.deafened || self.controls.sound_blocked);
    }

    /// Sets what is sent: the frames of the microphone, or silence while the person muted
    /// it or push-to-talk waits for its key. A mute is also told to the room, so that the
    /// media server forwards nothing.
    fn sync_gate(&self) {
        let open = !self.controls.muted && (!self.config.push_to_talk || self.talking);
        self.link.set_open(open && self.capture.is_some());
        if let Publication::Published(publication) = &self.publication
            && publication.is_muted() != self.controls.muted
        {
            if self.controls.muted {
                publication.mute();
            } else {
                publication.unmute();
            }
        }
    }

    /// Publishes or withdraws the microphone track by what holds now: it is published
    /// while the device is in the room, the server does not block it, and it is open.
    fn sync_publication(&mut self) {
        let wanted = self.microphone_wanted() && self.capture.is_some();
        match (&self.publication, wanted) {
            (Publication::None, true) => self.publish(),
            (Publication::Published(_) | Publication::Publishing, false) => {
                self.forget_publication();
            }
            _ => {}
        }
    }

    fn publish(&mut self) {
        let Some(room) = &self.room else {
            return;
        };
        self.publish_generation += 1;
        let generation = self.publish_generation;
        self.publication = Publication::Publishing;
        // The processing is done before the source (ADR 0733), so the source applies none.
        let source = NativeAudioSource::new(
            AudioSourceOptions {
                echo_cancellation: false,
                noise_suppression: false,
                auto_gain_control: false,
            },
            SAMPLE_RATE,
            1,
            0,
        );
        *self.source.lock().unwrap_or_else(PoisonError::into_inner) = Some(source.clone());
        let track =
            LocalAudioTrack::create_audio_track("microphone", RtcAudioSource::Native(source));
        let options = TrackPublishOptions {
            source: TrackSource::Microphone,
            ..TrackPublishOptions::default()
        };
        let (participant, internal) = (room.local_participant(), self.internal.clone());
        self.runtime.spawn(async move {
            let publishing = participant.publish_track(LocalTrack::Audio(track), options);
            let result = match tokio::time::timeout(PUBLISH_LIMIT, publishing).await {
                Ok(Ok(publication)) => Ok(publication),
                Ok(Err(error)) => Err(error.to_string()),
                Err(_) => Err("no answer of the media server".to_owned()),
            };
            let _ = internal.send(Internal::Published(generation, result));
        });
    }

    fn published(&mut self, generation: u64, result: Result<LocalTrackPublication, String>) {
        let current = generation == self.publish_generation
            && matches!(self.publication, Publication::Publishing);
        match result {
            Ok(publication) if current => {
                self.publication = Publication::Published(publication);
                self.change(|state| state.published = true);
                self.sync_gate();
            }
            // The microphone was closed or blocked while the track was published.
            Ok(publication) => self.withdraw(publication),
            Err(error) if current => {
                tracing::info!(%error, "the microphone track was not published");
                self.forget_publication();
                self.retry_publication_later();
            }
            Err(_) => {}
        }
    }

    fn retry_publication_later(&self) {
        let (generation, internal) = (self.publish_generation, self.internal.clone());
        self.runtime.spawn(async move {
            tokio::time::sleep(PUBLISH_RETRY).await;
            let _ = internal.send(Internal::PublishRetry(generation));
        });
    }

    /// Ends the publication of the microphone, whatever state it is in.
    fn forget_publication(&mut self) {
        self.publish_generation += 1;
        self.microphone_returns = None;
        *self.source.lock().unwrap_or_else(PoisonError::into_inner) = None;
        if let Publication::Published(publication) =
            std::mem::replace(&mut self.publication, Publication::None)
        {
            self.withdraw(publication);
        }
        self.change(|state| state.published = false);
    }

    fn withdraw(&self, publication: LocalTrackPublication) {
        let Some(room) = &self.room else {
            return;
        };
        let participant = room.local_participant();
        self.runtime.spawn(async move {
            if let Err(error) = participant.unpublish_track(&publication.sid()).await {
                tracing::debug!(%error, "the microphone track was not withdrawn");
            }
        });
    }

    fn tell_levels(&self) {
        let mut people: HashMap<AccountId, PersonLevel> = HashMap::new();
        for remote in self.remotes.values() {
            let Some(account_id) = remote.account_id else {
                continue;
            };
            let entry = people.entry(account_id).or_default();
            entry.level = entry.level.max(level::meter(remote.level.read()));
            entry.speaking |= remote.speaking.load(std::sync::atomic::Ordering::Relaxed);
        }
        let levels = Levels {
            input: level::meter(self.link.input.read()),
            own: PersonLevel {
                level: level::meter(self.link.sent.read()),
                speaking: self.link.speaking(),
            },
            people,
        };
        self.levels.send_if_modified(|held| {
            if *held == levels {
                false
            } else {
                *held = levels;
                true
            }
        });
    }
}

/// Carries one remote track into the mix until the track ends.
async fn play(
    mut stream: NativeAudioStream,
    mixer: Arc<Mixer>,
    gains: watch::Receiver<f32>,
    level: Arc<LevelCell>,
    speaking: Arc<std::sync::atomic::AtomicBool>,
) {
    let source = mixer.add(*gains.borrow());
    let mut decision = Speaking::default();
    let mut gain = *gains.borrow();
    while let Some(frame) = stream.next().await {
        let now = *gains.borrow();
        if now != gain {
            gain = now;
            source.set_gain(gain);
        }
        let heard = level.frame(&frame.data);
        speaking.store(decision.frame(heard), std::sync::atomic::Ordering::Relaxed);
        source.push(&frame.data);
    }
    level.silence();
    speaking.store(false, std::sync::atomic::Ordering::Relaxed);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_account_of_a_participant_is_the_first_part_of_its_identity() {
        let account_id = AccountId::generate();
        let identity = format!("{account_id}:0123456789abcdef0123456789abcdef");
        assert_eq!(account_of(&identity), Some(account_id));
        assert_eq!(account_of(&account_id.to_string()), Some(account_id));
        assert_eq!(account_of("recorder"), None);
        assert_eq!(account_of(""), None);
    }
}
