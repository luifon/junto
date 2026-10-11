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
    CallJoin, CallPlace, LeaveReason, MicrophoneBlock, OwnCall, SoundBlock,
};
use junto_client_core::protocol::error::ErrorCode;
use junto_client_core::protocol::events::{Cursor, Event};
use junto_client_core::protocol::ids::{AccountId, ServerId, StreamEpoch};
use junto_client_core::{Client, ServerHandle};
use time::OffsetDateTime;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::time::Instant;

use crate::audio::{AudioError, AudioHost, DeviceInfo, Devices};
use crate::engine::{
    Connection, Controls, Engine, EngineOptions, EngineState, Levels, MediaJoin, MicrophoneState,
    OutputState, VoiceConfig,
};
use crate::permission::{MicrophoneAccess, MicrophonePermission};

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
            speaking_spacing: Duration::from_millis(700),
            speaking_renewal: Duration::from_secs(3),
            settle: Duration::from_secs(10),
        }
    }
}

/// The sound devices, the permission and the times that the calls of a client use.
#[derive(Clone)]
pub struct MediaOptions {
    pub host: Arc<dyn AudioHost>,
    pub permission: Arc<dyn MicrophonePermission>,
    pub timing: CallTiming,
}

impl MediaOptions {
    /// The devices and the permission of the machine.
    pub fn system() -> Self {
        Self {
            host: Arc::new(crate::audio::cpal_host::CpalHost::new()),
            permission: Arc::new(crate::permission::SystemPermission),
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
        let asked = ask(&self.inner, target);
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
    Ok(inner.in_turn(join(inner.clone(), key, server, target)))
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
/// The leave request of the server ends the call of the account on whichever device
/// (ADR 0510), so the client reads first which place the server holds, and sends the
/// request only for the identity that this device was given. A place that another device
/// or session of the account took meanwhile stays.
async fn give_up(inner: &Arc<Inner>, server_id: ServerId, identity: &str) {
    let Some(server) = inner.client.server(server_id) else {
        return;
    };
    let api = server.api();
    match api.own_call().await {
        Ok(view)
            if view
                .call
                .as_ref()
                .is_some_and(|held| held.own.identity == identity) =>
        {
            if let Err(error) = api.leave_call().await {
                tracing::debug!(%error, "the server was not told that the call was left");
            }
        }
        // The account is in no call, or in a call with another identity.
        Ok(_) => {}
        // Without the answer the place is not known to be the one of this device. The
        // server ends a place without a media connection by itself (ADR 0509).
        Err(error) => {
            tracing::debug!(%error, "the place of this device was not read, and not left");
        }
    }
}

async fn request(server: &ServerHandle, place: &CallPlace) -> Result<CallJoin, ApiError> {
    let api = server.api();
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
) -> Result<(), JoinError> {
    if !inner.joins(key) {
        // The person left while the join waited for its turn. The server was not asked.
        return Err(JoinError::Cancelled);
    }
    let answer = request(&server, &target.place).await;
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
    let controls = Controls {
        muted: shared.muted,
        deafened: shared.deafened,
        ..Controls::default()
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
        },
        media_join(&answer),
        shared.voice.clone(),
        controls,
        volumes,
    );
    if shared.talking {
        engine.set_talking(true);
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
        microphone_block: None,
        sound_block: None,
        connected_once: false,
        interrupted_at: None,
        retry: None,
        backoff: Backoff::new(timing.reconnect_first, timing.reconnect_longest),
        rejoining: false,
        state_report: Reporter::new(timing.state_spacing, None),
        speaking_report: Reporter::new(timing.speaking_spacing, Some(timing.speaking_renewal)),
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

/// A report that failed is sent again after this time.
const REPORT_RETRY: Duration = Duration::from_secs(3);
/// How long the end of a call waits for the event that states its cause.
const CAUSE_WAIT: Duration = Duration::from_millis(500);

enum Done {
    State(bool),
    Speaking(bool),
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
    let answer = request(&server, &target.place).await;
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
    speaking: bool,
    timing: CallTiming,
}

fn far() -> Instant {
    Instant::now() + Duration::from_secs(86_400)
}

impl Session {
    async fn run(mut self, mut commands: mpsc::UnboundedReceiver<SessionCommand>) {
        let mut engine_state = self.engine.state();
        let mut levels = self.engine.levels();
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
            Notice::Event { id, event, .. } => self.place.note(*id, event),
            Notice::Reloaded { cursor } => self.place.reloaded(cursor.epoch),
            Notice::Ended { .. } => {}
        }
        self.review = true;
    }

    fn command(&mut self, command: SessionCommand) {
        match command {
            SessionCommand::Controls => self.apply_controls(),
            SessionCommand::Talking(talking) => self.engine.set_talking(talking),
            SessionCommand::Volume(account_id, gain) => self.engine.set_volume(account_id, gain),
            SessionCommand::Config(config) => self.engine.set_config(config),
            SessionCommand::RetryMicrophone => self.engine.retry_microphone(),
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
        // This client asks the server for a place now: a join of the person, which ends
        // this call when it has its answer, or a new token for this call.
        let asking = self.rejoining
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
        let own = model
            .call_at(&self.target.place)
            .and_then(|call| call.participant(model.account.id));
        let blocks = (
            own.and_then(|own| own.microphone_block.clone()),
            own.and_then(|own| own.sound_block.clone()),
        );
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
            Done::Rejoined(answer, taken) => {
                let _ = taken.send(());
                self.rejoining = false;
                self.review = true;
                match *answer {
                    Ok(answer) => {
                        if self.place.renewed(&answer.identity, now) {
                            // The server gave a new place: the earlier one had ended.
                            self.state_report = Reporter::new(self.timing.state_spacing, None);
                            self.show();
                        }
                        self.engine.reconnect(media_join(&answer));
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
        // 30 reports of the state in 10 seconds (ADR 0510), 20 of speaking (ADR 0513).
        let timing = CallTiming::default();
        assert!(Duration::from_secs(10).as_millis() / timing.state_spacing.as_millis() <= 30);
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
