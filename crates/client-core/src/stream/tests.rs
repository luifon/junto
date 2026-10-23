//! The stream client against a connection and a server that the test plays, with a clock
//! that the test moves.

use std::collections::VecDeque;
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use junto_protocol::account::RecoveryNotice;
use junto_protocol::call::{
    Call, CallInvitation, CallMove, EntryRequest, MoveOutcome, OwnCallState, OwnCallView,
};
use junto_protocol::error::ErrorCode;
use junto_protocol::events::{
    ClientMessage, Cursor, EndReason, Event, Level, ResyncReason, ServerMessage,
};
use junto_protocol::ids::{
    AccountId, CallId, DeviceId, EntryRequestId, RecoveryId, ServerId, StreamEpoch,
};
use junto_protocol::snapshot::Snapshot;
use junto_protocol::structure::{ChannelKind, WorkspaceMember};
use tokio::sync::{Semaphore, mpsc, watch};
use tokio::time::Instant;

use super::connection::run;
use super::*;
use crate::address::ServerAddress;
use crate::api::{HttpRequest, HttpResponse, TransportError, TransportErrorKind, TransportFuture};
use crate::live::{LiveServer, LiveView, LoadError, Notice, Notices, StreamFault, StreamStatus};
use crate::model::tests::{Scene, channel, member, participant, scene, time};
use crate::model::{Loadable, ServerModel};
use crate::version::SupportedVersions;

mod admin;
mod discovery;
mod messages;
mod recordings;
mod refresh;

const PATIENCE: Duration = Duration::from_secs(24 * 60 * 60);

/// Waits on the clock of the test. A wait that never ends fails the test.
async fn soon<T>(what: &str, waiting: impl Future<Output = T>) -> T {
    match tokio::time::timeout(PATIENCE, waiting).await {
        Ok(value) => value,
        Err(_) => panic!("this did not happen: {what}"),
    }
}

/// The server side of one connection.
struct Peer {
    request: StreamRequest,
    to_client: mpsc::Sender<Incoming>,
    from_client: mpsc::Receiver<String>,
}

impl Peer {
    async fn send(&self, message: &ServerMessage) {
        let text = serde_json::to_string(message).unwrap();
        self.to_client.send(Incoming::Text(text)).await.unwrap();
    }

    async fn hello(&self, epoch: StreamEpoch) {
        self.send(&ServerMessage::Hello {
            server_version: "0.1.0".to_owned(),
            api_versions: vec![1],
            epoch,
            heartbeat_seconds: 25,
            idle_seconds: 60,
        })
        .await;
    }

    async fn event(&self, id: u64, event: Event) {
        self.send(&ServerMessage::Event {
            id,
            at: time(),
            event,
        })
        .await;
    }

    async fn subscribe(&mut self) -> (Level, Cursor) {
        let text = soon("a subscribe", self.from_client.recv())
            .await
            .expect("the client closed the connection");
        match serde_json::from_str(&text).unwrap() {
            ClientMessage::Subscribe { level, cursor } => (level, cursor.unwrap()),
        }
    }

    /// Answers the `subscribe` of the client as a server that serves it.
    async fn serve(&mut self) -> (Level, Cursor) {
        let (level, cursor) = self.subscribe().await;
        self.send(&ServerMessage::Subscribed { level, cursor })
            .await;
        (level, cursor)
    }

    /// Sends a frame at each `every`, as a server does at each heartbeat (ADR 0116), until
    /// the client lets go of the connection.
    fn beat(&self, every: Duration) {
        let pings = self.to_client.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(every).await;
                if pings.send(Incoming::Alive).await.is_err() {
                    return;
                }
            }
        });
    }

    async fn close(&self, code: u16) {
        let closed = Incoming::Closed { code: Some(code) };
        self.to_client.send(closed).await.unwrap();
    }

    /// True when the client has let go of the connection.
    async fn dropped(&mut self) -> bool {
        soon("the end of the connection", self.from_client.recv())
            .await
            .is_none()
    }
}

/// The connections that the client opens, and the refusals that the test planned.
struct Wire {
    peers: mpsc::UnboundedSender<Peer>,
    refusals: Mutex<VecDeque<OpenError>>,
    opened_at: Mutex<Vec<Instant>>,
}

impl StreamTransport for Wire {
    fn open(&self, request: StreamRequest) -> OpenFuture {
        self.opened_at.lock().unwrap().push(Instant::now());
        if let Some(refusal) = self.refusals.lock().unwrap().pop_front() {
            return Box::pin(async move { Err(refusal) });
        }
        let (to_client, incoming) = mpsc::channel(64);
        let (outgoing, from_client) = mpsc::channel(4);
        let _ = self.peers.send(Peer {
            request,
            to_client,
            from_client,
        });
        Box::pin(async move { Ok(StreamSocket { incoming, outgoing }) })
    }
}

fn refusal(code: &str, status: u16, retry_after: Option<u64>) -> OpenError {
    OpenError::Refused(HttpResponse {
        status,
        retry_after: retry_after.map(Duration::from_secs),
        body: format!(r#"{{"error":{{"code":"{code}","message":""}}}}"#)
            .into_bytes()
            .into(),
        receipt: crate::api::Receipt::NotStated,
    })
}

fn unreachable() -> OpenError {
    OpenError::Failed(TransportError::new(TransportErrorKind::Connect, "refused"))
}

/// The wait that a played server names when it limits a request.
const LIMITED_WAIT: Duration = Duration::from_secs(30);

#[derive(Default)]
struct Routes {
    snapshot: Option<Snapshot>,
    members: Vec<WorkspaceMember>,
    own_call: Option<OwnCallState>,
    own_call_cursor: Option<Cursor>,
    /// The answer of `GET /call` names no position, as a server from before ADR 0519.
    own_call_unplaced: bool,
    /// How many requests are on their way now, and the most that were at one time.
    active: usize,
    most_active: usize,
    /// Paths of the requests in the order of their arrival, with their tokens.
    seen: Vec<(String, String)>,
    /// The next requests whose path ends with the text fail with the status and code.
    failures: Vec<(&'static str, u16, &'static str)>,
    /// Requests whose path ends with the text wait for a permit. A text that ends with
    /// `/` holds every path that contains it. A text that starts with `PUT ` holds only
    /// the requests of that method.
    held: Option<(&'static str, Arc<Semaphore>)>,
    /// The answers to requests whose path ends with the text are made and then wait for a
    /// permit: the server has done the work, and the answer arrives later.
    held_answers: Option<(&'static str, Arc<Semaphore>)>,
    /// The messages of the one place that the tests of messages use, as the server holds
    /// them, and what the stream client did with them.
    chat: messages::Chat,
    /// The recordings and the transcripts that the played server holds.
    tape: recordings::Tape,
}

/// The routes that the stream client reads.
#[derive(Default)]
struct Http {
    routes: Arc<Mutex<Routes>>,
}

impl Http {
    fn count(&self, end: &str) -> usize {
        let routes = self.routes.lock().unwrap();
        routes
            .seen
            .iter()
            .filter(|(path, _)| path.ends_with(end))
            .count()
    }

    fn hold(&self, end: &'static str) -> Arc<Semaphore> {
        let gate = Arc::new(Semaphore::new(0));
        self.routes.lock().unwrap().held = Some((end, gate.clone()));
        gate
    }

    fn hold_answers(&self, end: &'static str) -> Arc<Semaphore> {
        let gate = Arc::new(Semaphore::new(0));
        self.routes.lock().unwrap().held_answers = Some((end, gate.clone()));
        gate
    }

    fn fail(&self, end: &'static str, status: u16, code: &'static str) {
        self.routes
            .lock()
            .unwrap()
            .failures
            .push((end, status, code));
    }
}

impl crate::api::Transport for Http {
    fn send(&self, request: HttpRequest) -> TransportFuture {
        let path = request.url.path().to_owned();
        let token = request.bearer.as_ref().unwrap().expose().to_owned();
        let gate = {
            let mut routes = self.routes.lock().unwrap();
            routes.seen.push((path.clone(), token));
            routes
                .held
                .as_ref()
                .filter(|(end, _)| match end.strip_prefix("PUT ") {
                    Some(end) => request.method == crate::api::Method::Put && path.ends_with(end),
                    None => path.ends_with(end) || (end.ends_with('/') && path.contains(end)),
                })
                .map(|(_, gate)| gate.clone())
        };
        let request = request.clone();
        let routes = self.routes.clone();
        Box::pin(async move {
            let _active = Active::start(&routes);
            if let Some(gate) = gate {
                gate.acquire().await.unwrap().forget();
            }
            // The answer is made after the wait, from the routes of that moment.
            let answer = answer_of(&routes, &request, &path);
            let after = {
                let routes = routes.lock().unwrap();
                routes
                    .held_answers
                    .as_ref()
                    .filter(|(end, _)| path.ends_with(end))
                    .map(|(_, gate)| gate.clone())
            };
            if let Some(after) = after {
                after.acquire().await.unwrap().forget();
            }
            answer
        })
    }
}

/// The answer of the played routes to a request.
fn answer_of(
    routes: &Mutex<Routes>,
    request: &HttpRequest,
    path: &str,
) -> Result<HttpResponse, TransportError> {
    let mut routes = routes.lock().unwrap();
    let failure = routes
        .failures
        .iter()
        .position(|(end, _, _)| path.ends_with(end));
    if let Some(position) = failure {
        let (_, status, code) = routes.failures.remove(position);
        let body = format!(r#"{{"error":{{"code":"{code}","message":""}}}}"#);
        return Ok(HttpResponse {
            status,
            // A limited request names the time after which the server takes the next one.
            retry_after: (code == "rate_limited").then_some(LIMITED_WAIT),
            body: body.into_bytes().into(),
            receipt: crate::api::Receipt::NotStated,
        });
    }
    if let Some(answer) = routes.chat.answer(request) {
        return answer;
    }
    if let Some(answer) = routes.tape.answer(request) {
        return answer;
    }
    let body = if path.ends_with("/snapshot") {
        serde_json::to_vec(routes.snapshot.as_ref().unwrap())
    } else if path.ends_with("/members") {
        serde_json::to_vec(&routes.members)
    } else if path.ends_with("/api/v1/call") {
        serde_json::to_vec(&OwnCallView {
            cursor: (!routes.own_call_unplaced).then(|| {
                routes
                    .own_call_cursor
                    .unwrap_or(routes.snapshot.as_ref().unwrap().cursor)
            }),
            call: routes.own_call.clone(),
        })
    } else {
        panic!("the stream client asked for {path}");
    };
    Ok(HttpResponse {
        status: 200,
        retry_after: None,
        body: body.unwrap().into(),
        receipt: crate::api::Receipt::NotStated,
    })
}

/// Counts a request from its start to its answer or to the moment it is dropped.
struct Active(Arc<Mutex<Routes>>);

impl Active {
    fn start(routes: &Arc<Mutex<Routes>>) -> Self {
        let mut held = routes.lock().unwrap();
        held.active += 1;
        held.most_active = held.most_active.max(held.active);
        Self(routes.clone())
    }
}

impl Drop for Active {
    fn drop(&mut self) {
        self.0.lock().unwrap().active -= 1;
    }
}

#[derive(Default)]
struct Probe {
    ended: Mutex<Vec<String>>,
    in_call: Mutex<Vec<bool>>,
}

impl Host for Probe {
    fn session_ended(&self, token: &SecretToken, _reason: Option<EndReason>) {
        self.ended.lock().unwrap().push(token.expose().to_owned());
    }

    fn in_call(&self, in_call: bool) {
        self.in_call.lock().unwrap().push(in_call);
    }
}

/// One stream client with everything around it played by the test.
struct Rig {
    scene: Scene,
    live: LiveServer,
    host: Arc<Probe>,
    wire: Arc<Wire>,
    peers: mpsc::UnboundedReceiver<Peer>,
    http: Arc<Http>,
    desired: watch::Sender<Desired>,
    address: ServerAddress,
    server_id: ServerId,
}

fn rig() -> Rig {
    rig_with(StreamPolicy::default())
}

fn rig_with(policy: StreamPolicy) -> Rig {
    let scene = scene();
    let http = Arc::new(Http::default());
    http.routes.lock().unwrap().snapshot = Some(scene.snapshot.clone());
    let (peers, accepted) = mpsc::unbounded_channel();
    let wire = Arc::new(Wire {
        peers,
        refusals: Mutex::default(),
        opened_at: Mutex::default(),
    });
    let environment = Arc::new(Environment {
        transport: http.clone(),
        sockets: wire.clone(),
        retry: RetryPolicy::default(),
        policy,
    });
    let shared = Shared::new();
    let host = Arc::new(Probe::default());
    let (desired, wanted) = watch::channel(Desired {
        level: ConnectionLevel::Off,
        target: None,
    });
    let (commands, received) = mpsc::unbounded_channel();
    shared.attach(commands);
    tokio::spawn(run(
        shared.clone(),
        host.clone(),
        environment,
        wanted,
        received,
    ));
    Rig {
        scene,
        live: LiveServer { shared },
        host,
        wire,
        peers: accepted,
        http,
        desired,
        address: ServerAddress::parse("chat.example.test").unwrap(),
        server_id: ServerId::generate(),
    }
}

impl Rig {
    fn target(&self, token: &str, account_id: AccountId) -> StreamTarget {
        StreamTarget {
            server_id: self.server_id,
            address: self.address.clone(),
            api_version: SupportedVersions::THIS_BUILD.negotiate(&[1]).unwrap(),
            token: SecretToken::new(token.to_owned()),
            account_id,
            device_id: self.scene.device_id,
        }
    }

    fn want(&self, level: ConnectionLevel, token: &str) {
        let target = self.target(token, self.scene.account_id);
        self.desired.send_replace(Desired {
            level,
            target: Some(target),
        });
    }

    fn off(&self) {
        self.desired.send_replace(Desired {
            level: ConnectionLevel::Off,
            target: None,
        });
    }

    async fn peer(&mut self) -> Peer {
        soon("a connection", self.peers.recv()).await.unwrap()
    }

    /// The next connection, greeted and with its `subscribe` served.
    async fn served(&mut self) -> (Peer, Level, Cursor) {
        let mut peer = self.peer().await;
        peer.hello(self.epoch()).await;
        let (level, cursor) = peer.serve().await;
        self.when("live", |view| view.status == StreamStatus::Live)
            .await;
        (peer, level, cursor)
    }

    fn epoch(&self) -> StreamEpoch {
        self.scene.snapshot.cursor.epoch
    }

    async fn when(&self, what: &str, holds: impl FnMut(&LiveView) -> bool) -> LiveView {
        let mut watch = self.live.watch();
        soon(what, watch.wait_for(holds)).await.unwrap()
    }

    async fn model_when(
        &self,
        what: &str,
        mut holds: impl FnMut(&ServerModel) -> bool,
    ) -> Arc<ServerModel> {
        self.when(what, |view| view.model.as_deref().is_some_and(&mut holds))
            .await
            .model
            .unwrap()
    }

    fn model(&self) -> Arc<ServerModel> {
        self.live.view().model.unwrap()
    }

    fn set_snapshot(&self, position: u64, change: impl FnOnce(&mut Snapshot)) {
        let mut routes = self.http.routes.lock().unwrap();
        let snapshot = routes.snapshot.as_mut().unwrap();
        snapshot.cursor.position = position;
        change(snapshot);
    }

    fn opens(&self) -> usize {
        self.wire.opened_at.lock().unwrap().len()
    }
}

fn created(scene: &Scene, name: &str) -> Event {
    Event::ChannelCreated {
        channel: channel(scene.workspace_id, ChannelKind::Text, name, 9),
    }
}

fn has_channel(model: &ServerModel, name: &str) -> bool {
    model.workspaces.iter().any(|workspace| {
        workspace
            .channels
            .iter()
            .any(|held| held.channel.name == name)
    })
}

async fn next_event(notices: &mut Notices) -> (u64, Arc<Event>) {
    loop {
        match soon("a notice", notices.next()).await {
            Some(Ok(Notice::Event { id, event, .. })) => return (id, event),
            Some(Ok(_)) => {}
            other => panic!("no notice: {other:?}"),
        }
    }
}

#[tokio::test(start_paused = true)]
async fn the_client_loads_the_state_subscribes_with_its_cursor_and_applies_events() {
    let mut rig = rig();
    assert_eq!(rig.live.view().status, StreamStatus::Off);
    assert!(rig.live.view().model.is_none());
    let mut notices = rig.live.notices();

    rig.want(ConnectionLevel::Selected, "jst_one");
    let mut peer = rig.peer().await;
    assert_eq!(
        peer.request.url.as_str(),
        "https://chat.example.test/api/v1/events"
    );
    assert_eq!(peer.request.bearer.expose(), "jst_one");
    assert_eq!(rig.live.view().status, StreamStatus::Connecting);
    peer.hello(rig.epoch()).await;
    assert_eq!(
        peer.subscribe().await,
        (Level::Full, rig.scene.snapshot.cursor)
    );
    // No event is applied before the server has answered the subscribe.
    peer.event(41, created(&rig.scene, "too-early")).await;
    peer.send(&ServerMessage::Subscribed {
        level: Level::Full,
        cursor: rig.scene.snapshot.cursor,
    })
    .await;
    let view = rig
        .when("live", |view| view.status == StreamStatus::Live)
        .await;
    assert_eq!(view.level, Some(Level::Full));
    assert!(!has_channel(&view.model.unwrap(), "too-early"));
    assert_eq!(rig.http.count("/snapshot"), 1);
    assert!(matches!(
        soon("a notice", notices.next()).await,
        Some(Ok(Notice::Reloaded { cursor })) if cursor == rig.scene.snapshot.cursor
    ));

    peer.event(41, created(&rig.scene, "plans")).await;
    let model = rig
        .model_when("the event", |model| model.cursor.position == 41)
        .await;
    assert!(has_channel(&model, "plans"));
    let (id, event) = next_event(&mut notices).await;
    assert_eq!((id, event.name()), (41, "channel_created"));

    // A repeat is not applied and not told again. An event and a message of a kind that
    // this version does not know are skipped; the event moves the cursor.
    peer.event(41, created(&rig.scene, "repeat")).await;
    peer.event(40, created(&rig.scene, "older")).await;
    let unknown_event = r#"{"type":"event","id":43,"at":"2030-01-01T00:00:00Z",
        "event":{"type":"message_created","text":"hello"}}"#;
    for text in [r#"{"type":"notice","text":"hello"}"#, unknown_event] {
        peer.to_client
            .send(Incoming::Text(text.to_owned()))
            .await
            .unwrap();
    }
    let model = rig
        .model_when("the cursor", |model| model.cursor.position == 43)
        .await;
    assert!(!has_channel(&model, "repeat") && !has_channel(&model, "older"));
    peer.send(&ServerMessage::Checkpoint {
        cursor: Cursor {
            epoch: rig.epoch(),
            position: 50,
        },
    })
    .await;
    rig.model_when("the checkpoint", |model| model.cursor.position == 50)
        .await;
    // A reader that gets the notice of an event finds the event in the state.
    peer.event(51, created(&rig.scene, "after")).await;
    let (id, _) = next_event(&mut notices).await;
    assert_eq!(id, 51);
    let model = rig.model();
    assert!(model.cursor.position == 51 && has_channel(&model, "after"));
    assert_eq!(rig.opens(), 1);
    assert!(rig.host.ended.lock().unwrap().is_empty());
}

#[tokio::test(start_paused = true)]
async fn a_lost_connection_is_continued_from_the_cursor_without_a_snapshot() {
    let mut rig = rig();
    rig.want(ConnectionLevel::Selected, "jst_one");
    let (peer, _, _) = rig.served().await;
    peer.event(41, created(&rig.scene, "plans")).await;
    rig.model_when("the event", |model| model.cursor.position == 41)
        .await;

    let lost_at = Instant::now();
    peer.close(4003).await;
    let view = rig
        .when("interrupted", |view| view.status != StreamStatus::Live)
        .await;
    assert_eq!(
        view.status,
        StreamStatus::Interrupted(StreamFault::Closed(Some(4003)))
    );
    assert!(has_channel(&view.model.unwrap(), "plans"));

    let mut peer = rig.peer().await;
    let waited = lost_at.elapsed();
    assert!(
        waited >= Duration::from_millis(500) && waited <= Duration::from_secs(1),
        "{waited:?}"
    );
    peer.hello(rig.epoch()).await;
    let (level, cursor) = peer.serve().await;
    assert_eq!((level, cursor.position), (Level::Full, 41));
    rig.when("live", |view| view.status == StreamStatus::Live)
        .await;
    // The server sends again what the client may have: it is not applied twice.
    peer.event(41, created(&rig.scene, "repeat")).await;
    peer.event(42, created(&rig.scene, "next")).await;
    let model = rig
        .model_when("the event", |model| model.cursor.position == 42)
        .await;
    assert!(has_channel(&model, "next") && !has_channel(&model, "repeat"));
    assert_eq!(rig.http.count("/snapshot"), 1);
}

#[tokio::test(start_paused = true)]
async fn repeated_successful_resyncs_are_rate_limited() {
    let mut rig = rig();
    rig.want(ConnectionLevel::Selected, "jst_one");
    let (mut peer, _, _) = rig.served().await;
    // A server sends a frame at each heartbeat also while the client waits for a snapshot.
    peer.beat(Duration::from_secs(25));
    // The server asks for a reload as soon as the client subscribed after each snapshot.
    // The first reload reads at once. Each read after it waits a random time between a half
    // and the whole of a wait that starts at the first wait of the policy and doubles up to
    // its longest wait.
    let policy = StreamPolicy::default();
    let mut whole = Duration::ZERO;
    for reload in 0..12 {
        let position = rig.model().cursor.position + 1;
        rig.set_snapshot(position, |_| {});
        let asked = Instant::now();
        peer.send(&ServerMessage::Resync {
            reason: ResyncReason::TooFarBehind,
        })
        .await;
        rig.model_when("the snapshot", |model| model.cursor.position == position)
            .await;
        let waited = asked.elapsed();
        assert!(
            waited >= whole / 2 && waited <= whole,
            "reload {reload}: waited {waited:?} of {whole:?}"
        );
        whole = if reload == 0 {
            policy.first_delay
        } else {
            whole.saturating_mul(2).min(policy.longest_delay)
        };
        peer.serve().await;
    }
    assert_eq!(whole, policy.longest_delay);
    // A state that stays for the settled time is replaced at once again.
    tokio::time::sleep(policy.settled_after).await;
    let position = rig.model().cursor.position + 1;
    rig.set_snapshot(position, |_| {});
    let before = Instant::now();
    peer.send(&ServerMessage::Resync {
        reason: ResyncReason::TooFarBehind,
    })
    .await;
    rig.model_when("the snapshot", |model| model.cursor.position == position)
        .await;
    assert!(before.elapsed() < Duration::from_secs(1));
    peer.serve().await;
}

#[tokio::test(start_paused = true)]
async fn a_resync_replaces_the_state_and_subscribes_again_on_the_same_connection() {
    let mut rig = rig();
    rig.want(ConnectionLevel::Selected, "jst_one");
    let (mut peer, _, _) = rig.served().await;
    peer.event(41, created(&rig.scene, "stale")).await;
    rig.model_when("the event", |model| model.cursor.position == 41)
        .await;
    let mut notices = rig.live.notices();

    for reason in [
        ResyncReason::TooFarBehind,
        ResyncReason::UnknownCursor,
        ResyncReason::SkippedEvent,
    ] {
        let position = rig.model().cursor.position + 100;
        rig.set_snapshot(position, |_| {});
        let before = rig.http.count("/snapshot");
        peer.send(&ServerMessage::Resync { reason }).await;
        let (level, cursor) = peer.subscribe().await;
        assert_eq!(
            (level, cursor.position),
            (Level::Full, position),
            "{reason:?}"
        );
        assert_eq!(rig.http.count("/snapshot"), before + 1);
        // Events of the subscription before are not applied to the new state.
        peer.event(position + 1, created(&rig.scene, "between"))
            .await;
        peer.send(&ServerMessage::Subscribed { level, cursor })
            .await;
        let view = rig
            .when("live", |view| view.status == StreamStatus::Live)
            .await;
        let model = view.model.unwrap();
        assert_eq!(model.cursor.position, position);
        assert!(!has_channel(&model, "stale") && !has_channel(&model, "between"));
        assert!(matches!(
            soon("a notice", notices.next()).await,
            Some(Ok(Notice::Reloaded { cursor })) if cursor.position == position
        ));
    }
    assert_eq!(rig.opens(), 1);
}

#[tokio::test(start_paused = true)]
async fn another_epoch_in_hello_or_in_a_checkpoint_loads_a_snapshot() {
    let mut rig = rig();
    rig.want(ConnectionLevel::Selected, "jst_one");
    let (peer, _, _) = rig.served().await;
    peer.close(1001).await;

    let restored = StreamEpoch::generate();
    rig.set_snapshot(3, |snapshot| snapshot.cursor.epoch = restored);
    let mut peer = rig.peer().await;
    peer.hello(restored).await;
    let (_, cursor) = peer.serve().await;
    assert_eq!(
        cursor,
        Cursor {
            epoch: restored,
            position: 3
        }
    );
    assert_eq!(rig.http.count("/snapshot"), 2);
    rig.when("live", |view| view.status == StreamStatus::Live)
        .await;

    let again = StreamEpoch::generate();
    rig.set_snapshot(1, |snapshot| snapshot.cursor.epoch = again);
    peer.send(&ServerMessage::Checkpoint {
        cursor: Cursor {
            epoch: again,
            position: 1,
        },
    })
    .await;
    let (_, cursor) = peer.serve().await;
    assert_eq!(
        cursor,
        Cursor {
            epoch: again,
            position: 1
        }
    );
    assert_eq!(rig.http.count("/snapshot"), 3);
}

#[tokio::test(start_paused = true)]
async fn a_permission_change_reloads_the_state_and_drops_what_arrives_meanwhile() {
    let mut rig = rig();
    rig.want(ConnectionLevel::Selected, "jst_one");
    let (mut peer, _, _) = rig.served().await;

    for (round, event) in [
        Event::PermissionsChanged { workspace_id: None },
        Event::CallAccessEnded {},
    ]
    .into_iter()
    .enumerate()
    {
        let base = 41 + 10 * round as u64;
        let gate = rig.http.hold("/snapshot");
        let name = format!("granted-{round}");
        let granted = channel(rig.scene.workspace_id, ChannelKind::Text, &name, 7);
        rig.set_snapshot(base + 4, |snapshot| {
            snapshot.workspaces[0].channels.push(granted);
        });
        peer.event(base, event).await;
        rig.when("the reload", |view| view.status == StreamStatus::Connecting)
            .await;
        // The snapshot holds the effect of this event, and the next subscription
        // delivers the later one again.
        peer.event(base + 1, created(&rig.scene, "during")).await;
        peer.event(base + 5, created(&rig.scene, "later")).await;
        tokio::task::yield_now().await;
        assert!(!has_channel(&rig.model(), "during"));
        gate.add_permits(1);
        let (level, cursor) = peer.subscribe().await;
        assert_eq!((level, cursor.position), (Level::Full, base + 4));
        peer.send(&ServerMessage::Subscribed { level, cursor })
            .await;
        let view = rig
            .when("live", |view| view.status == StreamStatus::Live)
            .await;
        let model = view.model.unwrap();
        assert!(has_channel(&model, &name));
        assert!(!has_channel(&model, "during") && !has_channel(&model, "later"));
        rig.http.routes.lock().unwrap().held = None;
    }
    assert_eq!(rig.http.count("/snapshot"), 3);
    assert_eq!(rig.opens(), 1);
}

#[tokio::test(start_paused = true)]
async fn an_ended_session_is_reported_once_and_the_next_session_continues_the_state() {
    let mut rig = rig();
    rig.want(ConnectionLevel::Selected, "jst_one");
    let (mut peer, _, _) = rig.served().await;
    let mut notices = rig.live.notices();
    let recovery = RecoveryNotice {
        id: RecoveryId::generate(),
        device_id: None,
        performed_by: None,
        performed_at: time(),
    };
    peer.send(&ServerMessage::Ended {
        reason: EndReason::Recovered,
        recovery: Some(recovery.clone()),
    })
    .await;
    assert!(peer.dropped().await);
    let view = rig
        .when("ended", |view| view.status != StreamStatus::Live)
        .await;
    assert_eq!(
        view.status,
        StreamStatus::Interrupted(StreamFault::SessionEnded(Some(EndReason::Recovered)))
    );
    assert!(matches!(
        soon("a notice", notices.next()).await,
        Some(Ok(Notice::Ended { reason: EndReason::Recovered, recovery: Some(told) }))
            if told == recovery
    ));
    assert_eq!(*rig.host.ended.lock().unwrap(), ["jst_one"]);

    // Nothing is tried with a session that ended.
    tokio::time::sleep(Duration::from_secs(3600)).await;
    assert_eq!(rig.opens(), 1);
    assert_eq!(*rig.host.ended.lock().unwrap(), ["jst_one"]);

    // The login gave the same account another session: the state and its cursor stay.
    rig.want(ConnectionLevel::Selected, "jst_two");
    let mut peer = rig.peer().await;
    assert_eq!(peer.request.bearer.expose(), "jst_two");
    peer.hello(rig.epoch()).await;
    assert_eq!(peer.serve().await.1, rig.scene.snapshot.cursor);
    rig.when("live", |view| view.status == StreamStatus::Live)
        .await;
    assert_eq!(rig.http.count("/snapshot"), 1);

    // The close code of an ended session without the message is the same report.
    peer.close(4001).await;
    rig.when("ended", |view| {
        view.status == StreamStatus::Interrupted(StreamFault::SessionEnded(None))
    })
    .await;
    assert_eq!(*rig.host.ended.lock().unwrap(), ["jst_one", "jst_two"]);
}

#[tokio::test(start_paused = true)]
async fn a_login_without_an_answer_tries_the_ended_session_again_and_keeps_the_state() {
    let mut rig = rig();
    rig.want(ConnectionLevel::Selected, "jst_one");
    let (peer, _, _) = rig.served().await;
    peer.send(&ServerMessage::Ended {
        reason: EndReason::SessionEnded,
        recovery: None,
    })
    .await;
    rig.when("ended", |view| {
        matches!(
            view.status,
            StreamStatus::Interrupted(StreamFault::SessionEnded(_))
        )
    })
    .await;
    assert_eq!(*rig.host.ended.lock().unwrap(), ["jst_one"]);
    let ended_at = Instant::now();

    // The login got no answer of the server: the client core keeps the session.
    assert!(rig.live.shared.send(crate::live::Command::LoginUnanswered));
    rig.wire
        .refusals
        .lock()
        .unwrap()
        .push_back(refusal("unauthenticated", 401, None));
    rig.when("ended again", |_| rig.host.ended.lock().unwrap().len() == 2)
        .await;
    assert_eq!(*rig.host.ended.lock().unwrap(), ["jst_one", "jst_one"]);
    assert_eq!(rig.opens(), 2);
    // The attempt waited as an attempt after a lost connection does.
    assert!(ended_at.elapsed() >= Duration::from_millis(500));
    assert!(rig.live.view().model.is_some());

    // The next login succeeds: the state and its cursor stay.
    rig.want(ConnectionLevel::Selected, "jst_two");
    let mut peer = rig.peer().await;
    assert_eq!(peer.request.bearer.expose(), "jst_two");
    peer.hello(rig.epoch()).await;
    assert_eq!(peer.serve().await.1, rig.scene.snapshot.cursor);
    assert_eq!(rig.http.count("/snapshot"), 1);
}

#[tokio::test(start_paused = true)]
async fn a_refused_upgrade_ends_the_session_or_waits_for_the_time_that_the_server_names() {
    let mut rig = rig();
    rig.wire.refusals.lock().unwrap().extend([
        refusal("too_many_connections", 429, Some(7)),
        refusal("rate_limited", 429, Some(600)),
        refusal("unauthenticated", 401, None),
    ]);
    rig.want(ConnectionLevel::Background, "jst_one");
    rig.when("refused", |view| {
        view.status
            == StreamStatus::Interrupted(StreamFault::Refused(ErrorCode::TooManyConnections))
    })
    .await;
    rig.when("refused", |view| {
        view.status == StreamStatus::Interrupted(StreamFault::Refused(ErrorCode::RateLimited))
    })
    .await;
    rig.when("ended", |view| {
        view.status == StreamStatus::Interrupted(StreamFault::SessionEnded(None))
    })
    .await;
    let opened_at = rig.wire.opened_at.lock().unwrap().clone();
    assert_eq!(opened_at.len(), 3);
    assert!(opened_at[1] - opened_at[0] >= Duration::from_secs(7));
    // A stated time above the longest wait of the client is cut to it.
    let second = opened_at[2] - opened_at[1];
    assert!(
        second >= Duration::from_secs(60) && second <= Duration::from_secs(61),
        "{second:?}"
    );
    assert_eq!(*rig.host.ended.lock().unwrap(), ["jst_one"]);
    // The state was loaded and is shown although no connection was made.
    assert!(rig.live.view().model.is_some());

    rig.want(ConnectionLevel::Background, "jst_two");
    let (_, level, _) = rig.served().await;
    assert_eq!(level, Level::Background);
}

#[tokio::test(start_paused = true)]
async fn the_waits_between_attempts_double_up_to_a_bound_and_start_again_after_a_settled_connection()
 {
    let mut rig = rig();
    rig.wire
        .refusals
        .lock()
        .unwrap()
        .extend((0..9).map(|_| unreachable()));
    rig.want(ConnectionLevel::Selected, "jst_one");
    let (peer, _, _) = rig.served().await;
    assert_eq!(
        rig.http.count("/snapshot"),
        1,
        "a connection that fails asks for no further snapshot"
    );
    let opened_at = rig.wire.opened_at.lock().unwrap().clone();
    assert_eq!(opened_at.len(), 10);
    let mut whole = Duration::from_secs(1);
    for pair in opened_at.windows(2) {
        let waited = pair[1] - pair[0];
        assert!(
            waited >= whole / 2 && waited <= whole,
            "{waited:?} of {whole:?}"
        );
        whole = (whole * 2).min(Duration::from_secs(60));
    }
    assert_eq!(whole, Duration::from_secs(60));

    // A connection that is lost at once does not start the waits again.
    let lost_at = Instant::now();
    peer.close(1011).await;
    let peer = rig.served().await.0;
    assert!(lost_at.elapsed() >= Duration::from_secs(30));

    // One that delivered for the settling time does. The server shows that it lives.
    for _ in 0..3 {
        tokio::time::sleep(Duration::from_secs(20)).await;
        peer.to_client.send(Incoming::Alive).await.unwrap();
    }
    let lost_at = Instant::now();
    peer.close(1001).await;
    rig.served().await;
    assert!(lost_at.elapsed() <= Duration::from_secs(1));
}

#[tokio::test(start_paused = true)]
async fn a_connection_without_a_frame_for_two_heartbeats_is_taken_as_lost() {
    let mut rig = rig();
    rig.want(ConnectionLevel::Selected, "jst_one");

    // No `hello` in time.
    let mut silent = rig.peer().await;
    let opened = Instant::now();
    assert!(silent.dropped().await);
    assert_eq!(opened.elapsed(), Duration::from_secs(10));
    assert_eq!(
        rig.live.view().status,
        StreamStatus::Interrupted(StreamFault::Silent)
    );

    // No answer to the `subscribe` in time.
    let mut unanswered = rig.peer().await;
    unanswered.hello(rig.epoch()).await;
    unanswered.subscribe().await;
    let asked = Instant::now();
    assert!(unanswered.dropped().await);
    assert_eq!(asked.elapsed(), Duration::from_secs(30));

    // Pings keep a connection; without them it ends after 2 x 25 s and 10 s.
    let (mut peer, _, _) = rig.served().await;
    for _ in 0..5 {
        tokio::time::sleep(Duration::from_secs(50)).await;
        peer.to_client.send(Incoming::Alive).await.unwrap();
    }
    assert_eq!(rig.live.view().status, StreamStatus::Live);
    let heard = Instant::now();
    assert!(peer.dropped().await);
    assert_eq!(heard.elapsed(), Duration::from_secs(60));
    assert_eq!(
        rig.live.view().status,
        StreamStatus::Interrupted(StreamFault::Silent)
    );
    rig.served().await;
}

#[tokio::test(start_paused = true)]
async fn a_change_to_the_full_level_loads_a_snapshot_and_a_change_back_does_not() {
    let mut rig = rig();
    rig.want(ConnectionLevel::Background, "jst_one");
    let (mut peer, level, _) = rig.served().await;
    assert_eq!(level, Level::Background);
    assert_eq!(rig.live.view().level, Some(Level::Background));
    peer.event(
        41,
        Event::DirectMuteRemoved {
            account_id: AccountId::generate(),
        },
    )
    .await;
    rig.model_when("the event", |model| model.cursor.position == 41)
        .await;

    rig.set_snapshot(45, |_| {});
    rig.want(ConnectionLevel::Selected, "jst_one");
    let (level, cursor) = peer.serve().await;
    assert_eq!((level, cursor.position), (Level::Full, 45));
    rig.when("full", |view| view.level == Some(Level::Full))
        .await;
    assert_eq!(rig.http.count("/snapshot"), 2);

    rig.want(ConnectionLevel::Background, "jst_one");
    let (level, cursor) = peer.serve().await;
    assert_eq!((level, cursor.position), (Level::Background, 45));
    rig.when("background", |view| view.level == Some(Level::Background))
        .await;
    assert_eq!(rig.http.count("/snapshot"), 2);
    assert_eq!(rig.opens(), 1);
}

#[tokio::test(start_paused = true)]
async fn the_client_sends_fewer_messages_in_a_minute_than_the_server_allows() {
    let mut rig = rig();
    rig.want(ConnectionLevel::Background, "jst_one");
    let (mut peer, _, _) = rig.served().await;
    peer.beat(Duration::from_secs(25));
    let started = Instant::now();
    let mut sent_at = vec![started];
    for round in 0..45 {
        let level = if round % 2 == 0 {
            ConnectionLevel::Selected
        } else {
            ConnectionLevel::Background
        };
        rig.want(level, "jst_one");
        peer.serve().await;
        sent_at.push(Instant::now());
        rig.when("the level", |view| view.level == wire_level(level))
            .await;
    }
    for (position, at) in sent_at.iter().enumerate() {
        let in_a_minute = sent_at[position..]
            .iter()
            .filter(|later| **later - *at < Duration::from_secs(60))
            .count();
        assert!(in_a_minute <= 20, "{in_a_minute} messages in one minute");
    }
    assert!(started.elapsed() >= Duration::from_secs(120));
    assert_eq!(rig.opens(), 1);
}

fn wire_level(level: ConnectionLevel) -> Option<Level> {
    match level {
        ConnectionLevel::Off => None,
        ConnectionLevel::Background => Some(Level::Background),
        ConnectionLevel::Selected => Some(Level::Full),
    }
}

#[tokio::test(start_paused = true)]
async fn members_are_read_on_demand_with_the_events_of_the_time_of_the_read() {
    let mut rig = rig();
    let workspace_id = rig.scene.workspace_id;
    let (ana, bruno, carla) = (member("Ana"), member("Bruno"), member("Carla"));
    rig.http.routes.lock().unwrap().members = vec![ana.clone(), carla.clone()];
    assert!(matches!(
        rig.live.load_members(workspace_id).await,
        Err(LoadError::Off)
    ));

    rig.want(ConnectionLevel::Selected, "jst_one");
    let (mut peer, _, _) = rig.served().await;
    assert!(matches!(
        rig.live
            .load_members(junto_protocol::ids::WorkspaceId::generate())
            .await,
        Err(LoadError::NotInState)
    ));

    let gate = rig.http.hold("/members");
    let loading = tokio::spawn(rig.live.load_members(workspace_id));
    rig.model_when("loading", |model| {
        model.workspace(workspace_id).unwrap().members == Loadable::Loading
    })
    .await;
    peer.event(
        41,
        Event::WorkspaceMemberAdded {
            workspace_id,
            member: bruno.clone(),
        },
    )
    .await;
    peer.event(
        42,
        Event::WorkspaceMemberRemoved {
            workspace_id,
            account_id: carla.account_id,
        },
    )
    .await;
    rig.model_when("the events", |model| model.cursor.position == 42)
        .await;
    gate.add_permits(1);
    loading.await.unwrap().unwrap();
    let names = |model: &ServerModel| {
        let mut names: Vec<String> = model
            .workspace(workspace_id)
            .unwrap()
            .members
            .loaded()
            .map(|members| members.iter().map(|member| member.name.clone()).collect())
            .unwrap_or_default();
        names.sort();
        names
    };
    assert_eq!(names(&rig.model()), ["Ana", "Bruno"]);
    // A second reader gets the members that are held.
    rig.live.load_members(workspace_id).await.unwrap();
    assert_eq!(rig.http.count("/members"), 1);

    // A reload discards them (ADR 0207) and reads them again without a new request.
    rig.http.routes.lock().unwrap().members = vec![ana.clone()];
    rig.set_snapshot(60, |_| {});
    gate.add_permits(1);
    peer.send(&ServerMessage::Resync {
        reason: ResyncReason::TooFarBehind,
    })
    .await;
    peer.serve().await;
    let model = rig
        .model_when("the members again", |model| {
            model.cursor.position == 60
                && model
                    .workspace(workspace_id)
                    .unwrap()
                    .members
                    .loaded()
                    .is_some()
        })
        .await;
    assert_eq!(names(&model), ["Ana"]);
    assert_eq!(rig.http.count("/members"), 2);

    // A read that fails gives its reader the error and leaves the members unloaded.
    rig.live.release_members(workspace_id);
    rig.model_when("released", |model| {
        model.workspace(workspace_id).unwrap().members == Loadable::NotLoaded
    })
    .await;
    rig.http.routes.lock().unwrap().held = None;
    rig.http.fail("/members", 403, "forbidden");
    let error = rig.live.load_members(workspace_id).await.unwrap_err();
    assert!(matches!(error, LoadError::Request(error)
        if error.code() == Some(ErrorCode::Forbidden)));
    assert_eq!(
        rig.model().workspace(workspace_id).unwrap().members,
        Loadable::NotLoaded
    );
    rig.live.load_members(workspace_id).await.unwrap();
    assert_eq!(names(&rig.model()), ["Ana"]);
}

#[tokio::test(start_paused = true)]
async fn the_call_of_the_account_is_read_with_its_requests_and_reported_to_the_client_core() {
    let mut rig = rig();
    rig.want(ConnectionLevel::Background, "jst_one");
    let (peer, _, _) = rig.served().await;
    assert!(rig.host.in_call.lock().unwrap().is_empty());

    let (call_id, place) = (CallId::generate(), rig.scene.place(rig.scene.daily));
    let own = rig.scene.own_call(call_id, rig.scene.daily);
    let asking = AccountId::generate();
    let request = EntryRequest {
        id: EntryRequestId::generate(),
        call_id,
        place,
        account_id: asking,
        created_at: time(),
        expires_at: time(),
    };
    rig.http.routes.lock().unwrap().own_call = Some(OwnCallState {
        inside: None,
        meeting_grant: None,
        own: own.clone(),
        call: Call {
            id: call_id,
            place,
            started_at: Some(time()),
            lock: None,
            meeting: None,
            hides_occupants: false,
            participants: vec![participant(rig.scene.account_id)],
            recording: None,
        },
        entry_requests: vec![request.clone()],
    });
    peer.event(
        41,
        Event::OwnCallChanged {
            call: Some(own.clone()),
            left: None,
        },
    )
    .await;
    let model = rig
        .model_when("the call", |model| !model.entry_requests.is_empty())
        .await;
    assert_eq!(model.entry_requests, [request]);
    assert_eq!(model.call_at(&place).unwrap().participants.len(), 1);
    assert_eq!(rig.http.count("/api/v1/call"), 1);
    assert_eq!(*rig.host.in_call.lock().unwrap(), [true]);

    // The call moves to another device of the account.
    let elsewhere = junto_protocol::call::OwnCall {
        hides_occupants: None,
        device_id: DeviceId::generate(),
        ..own
    };
    peer.event(
        42,
        Event::OwnCallChanged {
            call: Some(elsewhere),
            left: None,
        },
    )
    .await;
    rig.model_when("the move", |model| model.cursor.position == 42)
        .await;
    assert_eq!(*rig.host.in_call.lock().unwrap(), [true, false]);

    // A snapshot with a call of this device reads the call again.
    rig.set_snapshot(70, |snapshot| {
        snapshot.own_call = Some(rig.scene.own_call(call_id, rig.scene.daily));
    });
    peer.close(1001).await;
    let mut peer = rig.peer().await;
    peer.hello(StreamEpoch::generate()).await;
    peer.subscribe().await;
    rig.model_when("the requests", |model| {
        model.cursor.position == 70 && !model.entry_requests.is_empty()
    })
    .await;
    assert_eq!(*rig.host.in_call.lock().unwrap(), [true, false, true]);

    rig.off();
    let view = rig
        .when("off", |view| view.status == StreamStatus::Off)
        .await;
    assert!(view.model.is_none() && view.level.is_none());
    assert!(peer.dropped().await);
    assert_eq!(
        *rig.host.in_call.lock().unwrap(),
        [true, false, true, false]
    );
}

#[tokio::test(start_paused = true)]
async fn a_message_that_cannot_be_read_ends_the_connection_and_the_state_is_loaded_again() {
    let mut rig = rig();
    rig.want(ConnectionLevel::Selected, "jst_one");
    let (mut peer, _, _) = rig.served().await;
    rig.set_snapshot(90, |_| {});
    // An event of a known type without its fields.
    let broken = r#"{"type":"event","id":41,"at":"2030-01-01T00:00:00Z",
        "event":{"type":"channel_created"}}"#;
    peer.to_client
        .send(Incoming::Text(broken.to_owned()))
        .await
        .unwrap();
    assert!(peer.dropped().await);
    assert_eq!(
        rig.live.view().status,
        StreamStatus::Interrupted(StreamFault::Unreadable)
    );
    let mut peer = rig.peer().await;
    peer.hello(rig.epoch()).await;
    assert_eq!(peer.serve().await.1.position, 90);
    assert_eq!(rig.http.count("/snapshot"), 2);

    // A first message that is no `hello`.
    let (peer, _, _) = {
        peer.close(1001).await;
        let peer = rig.peer().await;
        peer.send(&ServerMessage::Checkpoint {
            cursor: rig.scene.snapshot.cursor,
        })
        .await;
        (peer, (), ())
    };
    let mut peer = peer;
    assert!(peer.dropped().await);
}

#[tokio::test(start_paused = true)]
async fn another_account_starts_from_its_own_snapshot() {
    let mut rig = rig();
    rig.want(ConnectionLevel::Selected, "jst_one");
    let (mut peer, _, _) = rig.served().await;
    let workspace_id = rig.scene.workspace_id;
    rig.live.load_members(workspace_id).await.unwrap();

    let other = AccountId::generate();
    rig.set_snapshot(7, |snapshot| snapshot.account.id = other);
    rig.desired.send_replace(Desired {
        level: ConnectionLevel::Selected,
        target: Some(rig.target("jst_other", other)),
    });
    assert!(peer.dropped().await);
    let mut peer = rig.peer().await;
    assert_eq!(peer.request.bearer.expose(), "jst_other");
    peer.hello(rig.epoch()).await;
    assert_eq!(peer.serve().await.1.position, 7);
    let model = rig
        .model_when("the other account", |model| model.account.id == other)
        .await;
    // What a reader asked for belonged to the account before.
    assert_eq!(
        model.workspace(workspace_id).unwrap().members,
        Loadable::NotLoaded
    );
    let seen = rig.http.routes.lock().unwrap().seen.clone();
    assert_eq!(
        seen.last().unwrap(),
        &("/api/v1/snapshot".to_owned(), "jst_other".to_owned())
    );
}

#[tokio::test(start_paused = true)]
async fn a_snapshot_that_fails_is_asked_for_again_and_an_ended_session_is_reported() {
    let mut rig = rig();
    rig.http.fail("/snapshot", 500, "internal");
    rig.http.fail("/snapshot", 500, "internal");
    let started = Instant::now();
    rig.want(ConnectionLevel::Selected, "jst_one");
    let mut peer = rig.peer().await;
    peer.hello(rig.epoch()).await;
    rig.when("the fault", |view| {
        view.status == StreamStatus::Interrupted(StreamFault::Refused(ErrorCode::Internal))
    })
    .await;
    peer.serve().await;
    rig.when("live", |view| view.status == StreamStatus::Live)
        .await;
    assert_eq!(rig.http.count("/snapshot"), 3);
    let waited = started.elapsed();
    assert!(
        waited >= Duration::from_millis(1500) && waited <= Duration::from_secs(3),
        "{waited:?}"
    );

    rig.http.fail("/snapshot", 401, "unauthenticated");
    peer.send(&ServerMessage::Resync {
        reason: ResyncReason::UnknownCursor,
    })
    .await;
    rig.when("ended", |view| {
        view.status == StreamStatus::Interrupted(StreamFault::SessionEnded(None))
    })
    .await;
    assert_eq!(*rig.host.ended.lock().unwrap(), ["jst_one"]);
    assert!(peer.dropped().await);
}

#[tokio::test(start_paused = true)]
async fn a_server_cannot_make_the_state_grow_without_a_bound() {
    let mut rig = rig();
    rig.want(ConnectionLevel::Selected, "jst_one");
    let (mut peer, _, _) = rig.served().await;

    // 64 MiB of events are applied; then the state is replaced by a snapshot.
    let name = "n".repeat(200 * 1024);
    rig.set_snapshot(10_000, |_| {});
    for id in 41..41 + 330 {
        peer.event(id, created(&rig.scene, &name)).await;
    }
    let (_, cursor) = peer.serve().await;
    assert_eq!(cursor.position, 10_000);
    assert_eq!(rig.http.count("/snapshot"), 2);
    let view = rig
        .when("live", |view| view.status == StreamStatus::Live)
        .await;
    assert!(!has_channel(&view.model.unwrap(), &name));

    // A message above the limit of the client is not asked for again: the connection
    // ends, and the next one continues after the cursor of a snapshot.
    rig.set_snapshot(20_000, |_| {});
    peer.to_client.send(Incoming::TooLarge).await.unwrap();
    assert!(peer.dropped().await);
    assert_eq!(
        rig.live.view().status,
        StreamStatus::Interrupted(StreamFault::Unreadable)
    );
    let mut peer = rig.peer().await;
    peer.hello(rig.epoch()).await;
    assert_eq!(peer.serve().await.1.position, 20_000);
    assert_eq!(rig.http.count("/snapshot"), 3);
}

#[tokio::test(start_paused = true)]
async fn a_server_that_ends_every_session_gets_its_logins_at_the_waits_of_lost_connections() {
    let mut rig = rig();
    let mut ended_at = Vec::new();
    for round in 0..6 {
        rig.want(ConnectionLevel::Selected, &format!("jst_{round}"));
        let peer = rig.peer().await;
        ended_at.push(Instant::now());
        peer.hello(rig.epoch()).await;
        peer.send(&ServerMessage::Ended {
            reason: EndReason::SessionEnded,
            recovery: None,
        })
        .await;
        rig.when("ended", |view| {
            matches!(
                view.status,
                StreamStatus::Interrupted(StreamFault::SessionEnded(_))
            )
        })
        .await;
    }
    assert_eq!(rig.host.ended.lock().unwrap().len(), 6);
    let mut whole = Duration::from_secs(1);
    for pair in ended_at.windows(2) {
        let waited = pair[1] - pair[0];
        assert!(
            waited >= whole / 2 && waited <= whole,
            "{waited:?} of {whole:?}"
        );
        whole *= 2;
    }
}

/// Ends the process when a test does not end. A stream client that never waits takes the
/// only thread of the test, so no wait on the clock of the test can end such a test.
struct Guard(std::sync::mpsc::Sender<()>);

impl Drop for Guard {
    fn drop(&mut self) {
        let _ = self.0.send(());
    }
}

fn guard(what: &'static str) -> Guard {
    let (done, waiting) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        if waiting.recv_timeout(Duration::from_secs(120))
            == Err(std::sync::mpsc::RecvTimeoutError::Timeout)
        {
            eprintln!("the test did not end: {what}");
            std::process::abort();
        }
    });
    Guard(done)
}

#[tokio::test(start_paused = true)]
async fn a_session_that_ends_at_the_snapshot_gets_its_logins_at_the_waits_of_lost_connections() {
    let mut rig = rig();
    for _ in 0..5 {
        rig.http.fail("/snapshot", 401, "unauthenticated");
    }
    let mut asked_at = Vec::new();
    for round in 0..5 {
        rig.want(ConnectionLevel::Selected, &format!("jst_{round}"));
        soon("the report", async {
            while rig.host.ended.lock().unwrap().len() <= round {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await;
        asked_at.push(Instant::now());
        // The connection of the session that ended is let go.
        while let Ok(mut peer) = rig.peers.try_recv() {
            assert!(peer.dropped().await);
        }
    }
    assert_eq!(rig.http.count("/snapshot"), 5);
    let mut whole = Duration::from_secs(1);
    for pair in asked_at.windows(2) {
        let waited = pair[1] - pair[0];
        assert!(
            waited >= whole / 2 && waited <= whole + Duration::from_millis(10),
            "{waited:?} of {whole:?}"
        );
        whole *= 2;
    }
}

#[tokio::test(start_paused = true)]
async fn a_read_that_is_replaced_ends_its_request() {
    let mut rig = rig();
    rig.want(ConnectionLevel::Selected, "jst_one");
    let (peer, _, _) = rig.served().await;
    let _gate = rig.http.hold("/api/v1/call");
    for round in 0..40 {
        let own = rig.scene.own_call(CallId::generate(), rig.scene.daily);
        peer.event(
            41 + round,
            Event::OwnCallChanged {
                call: Some(own),
                left: None,
            },
        )
        .await;
    }
    rig.model_when("the events", |model| model.cursor.position == 80)
        .await;
    tokio::task::yield_now().await;
    let routes = rig.http.routes.lock().unwrap();
    assert_eq!(
        routes.active, 1,
        "one read of the call of the account is on its way"
    );
    assert!(
        routes.most_active <= 2,
        "{} requests at one time",
        routes.most_active
    );
}

#[tokio::test(start_paused = true)]
async fn a_state_of_the_background_level_is_replaced_when_the_server_is_selected_without_a_connection()
 {
    let mut rig = rig();
    rig.want(ConnectionLevel::Background, "jst_one");
    let (peer, level, _) = rig.served().await;
    assert_eq!(level, Level::Background);
    // The checkpoint passes changes of the structure that this level did not deliver.
    peer.send(&ServerMessage::Checkpoint {
        cursor: Cursor {
            epoch: rig.epoch(),
            position: 60,
        },
    })
    .await;
    rig.model_when("the checkpoint", |model| model.cursor.position == 60)
        .await;
    peer.close(1001).await;
    rig.when("the loss", |view| view.status != StreamStatus::Live)
        .await;

    rig.set_snapshot(75, |snapshot| {
        let unseen = channel(rig.scene.workspace_id, ChannelKind::Text, "unseen", 5);
        snapshot.workspaces[0].channels.push(unseen);
    });
    rig.want(ConnectionLevel::Selected, "jst_one");
    let mut peer = rig.peer().await;
    peer.hello(rig.epoch()).await;
    let (level, cursor) = peer.serve().await;
    assert_eq!((level, cursor.position), (Level::Full, 75));
    assert_eq!(rig.http.count("/snapshot"), 2);
    let view = rig
        .when("live", |view| view.status == StreamStatus::Live)
        .await;
    assert!(has_channel(&view.model.unwrap(), "unseen"));
}

#[tokio::test(start_paused = true)]
async fn fast_changes_between_off_and_selected_stay_below_the_rate_of_upgrade_requests() {
    let mut rig = rig();
    for _ in 0..50 {
        rig.want(ConnectionLevel::Selected, "jst_one");
        let mut peer = rig.peer().await;
        rig.off();
        assert!(peer.dropped().await);
    }
    let opened_at = rig.wire.opened_at.lock().unwrap().clone();
    assert_eq!(opened_at.len(), 50);
    for (position, at) in opened_at.iter().enumerate() {
        let in_a_minute = opened_at[position..]
            .iter()
            .filter(|later| **later - *at < Duration::from_secs(60))
            .count();
        assert!(
            in_a_minute <= 20,
            "{in_a_minute} upgrade requests in one minute"
        );
    }
}

#[tokio::test(start_paused = true)]
async fn a_wait_for_the_next_message_that_has_passed_does_not_keep_the_client_busy() {
    let _guard = guard("the stream client waits for nothing");
    let mut rig = rig();
    rig.want(ConnectionLevel::Background, "jst_one");
    let (mut peer, _, _) = rig.served().await;
    peer.beat(Duration::from_secs(25));
    // The messages of one minute are used up, so the next subscribe has a time.
    for round in 0..19 {
        let level = if round % 2 == 0 {
            ConnectionLevel::Selected
        } else {
            ConnectionLevel::Background
        };
        rig.want(level, "jst_one");
        peer.serve().await;
        rig.when("the level", |view| view.level == wire_level(level))
            .await;
    }
    rig.want(ConnectionLevel::Background, "jst_one");
    tokio::task::yield_now().await;
    // A reload whose snapshot takes longer than that time.
    let gate = rig.http.hold("/snapshot");
    rig.set_snapshot(500, |_| {});
    peer.send(&ServerMessage::Resync {
        reason: ResyncReason::TooFarBehind,
    })
    .await;
    tokio::time::sleep(Duration::from_secs(90)).await;
    gate.add_permits(1);
    let (_, cursor) = peer.serve().await;
    assert_eq!(cursor.position, 500);
}

#[tokio::test(start_paused = true)]
async fn a_new_session_reads_again_every_part_that_was_on_its_way() {
    let mut rig = rig();
    rig.want(ConnectionLevel::Selected, "jst_one");
    let (peer, _, _) = rig.served().await;
    let (call_id, place) = (CallId::generate(), rig.scene.place(rig.scene.hidden));
    let own = rig.scene.own_call(call_id, rig.scene.hidden);
    let inside = participant(AccountId::generate());
    rig.http.routes.lock().unwrap().own_call = Some(OwnCallState {
        inside: None,
        meeting_grant: None,
        own: own.clone(),
        call: Call {
            id: call_id,
            place,
            started_at: Some(time()),
            lock: None,
            meeting: None,
            hides_occupants: true,
            participants: vec![inside.clone()],
            recording: None,
        },
        entry_requests: Vec::new(),
    });
    let gate = rig.http.hold("/call");
    // The account gets a place in a call: the client reads its call.
    peer.event(
        41,
        Event::OwnCallChanged {
            call: Some(own),
            left: None,
        },
    )
    .await;
    rig.model_when("the event", |model| model.cursor.position == 41)
        .await;
    soon("the read", async {
        while rig.http.count("/call") == 0 {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await;

    rig.want(ConnectionLevel::Selected, "jst_two");
    let mut peer = rig.peer().await;
    peer.hello(rig.epoch()).await;
    peer.serve().await;
    gate.add_permits(2);
    let model = rig
        .model_when("the people inside", |model| {
            model
                .call_at(&place)
                .is_some_and(|call| !call.participants.is_empty())
        })
        .await;
    assert_eq!(model.call_at(&place).unwrap().participants, [inside]);
    let seen = rig.http.routes.lock().unwrap().seen.clone();
    assert_eq!(seen.last().unwrap().1, "jst_two");
}

/// ADR 0517: an account with a place in a call that hides who is inside gets nothing of
/// the call until the server has seen its device in the room. The event of its own entry
/// is the first one of the call that it receives, and the client reads the call then.
#[tokio::test(start_paused = true)]
async fn the_call_of_the_account_is_read_again_when_the_server_has_seen_its_device() {
    let mut rig = rig();
    rig.want(ConnectionLevel::Selected, "jst_one");
    let (peer, _, _) = rig.served().await;
    let (call_id, place) = (CallId::generate(), rig.scene.place(rig.scene.hidden));
    let own = rig.scene.own_call(call_id, rig.scene.hidden);
    let answer = |participants: Vec<junto_protocol::call::CallParticipant>,
                  entry_requests: Vec<EntryRequest>| OwnCallState {
        inside: None,
        meeting_grant: None,
        own: own.clone(),
        call: Call {
            id: call_id,
            place,
            started_at: Some(time()),
            lock: None,
            meeting: None,
            hides_occupants: true,
            participants,
            recording: None,
        },
        entry_requests,
    };
    rig.http.routes.lock().unwrap().own_call = Some(answer(Vec::new(), Vec::new()));
    peer.event(
        41,
        Event::OwnCallChanged {
            call: Some(own.clone()),
            left: None,
        },
    )
    .await;
    let model = rig
        .model_when("the call without its people", |model| {
            model.call_at(&place).is_some()
        })
        .await;
    let call = model.call_at(&place).unwrap();
    assert!(call.hides_occupants && call.participants.is_empty());
    assert_eq!(rig.http.count("/api/v1/call"), 1);

    let (me, other) = (
        participant(rig.scene.account_id),
        participant(AccountId::generate()),
    );
    let request = EntryRequest {
        id: EntryRequestId::generate(),
        call_id,
        place,
        account_id: AccountId::generate(),
        created_at: time(),
        expires_at: time(),
    };
    rig.http.routes.lock().unwrap().own_call = Some(answer(
        vec![other.clone(), me.clone()],
        vec![request.clone()],
    ));
    peer.event(
        42,
        Event::CallParticipantJoined {
            call_id,
            place,
            participant: me,
        },
    )
    .await;
    let model = rig
        .model_when("the people inside", |model| {
            model
                .call_at(&place)
                .is_some_and(|call| call.participants.len() == 2)
        })
        .await;
    assert_eq!(model.entry_requests, [request]);
    assert_eq!(rig.http.count("/api/v1/call"), 2);

    // The entry of another person is told by its event and asks for no read.
    peer.event(
        43,
        Event::CallParticipantJoined {
            call_id,
            place,
            participant: participant(AccountId::generate()),
        },
    )
    .await;
    rig.model_when("the third person", |model| {
        model
            .call_at(&place)
            .is_some_and(|call| call.participants.len() == 3)
    })
    .await;
    assert_eq!(rig.http.count("/api/v1/call"), 2);
}

/// No event carries what the read of the own call gives of a call that hides who is inside
/// (ADR 0517), so a read that failed is sent again while the account is in the call.
#[tokio::test(start_paused = true)]
async fn a_read_of_the_own_call_that_failed_is_sent_again_while_the_account_is_in_the_call() {
    let mut rig = rig();
    rig.want(ConnectionLevel::Selected, "jst_one");
    let (peer, _, _) = rig.served().await;
    let (call_id, place) = (CallId::generate(), rig.scene.place(rig.scene.hidden));
    let own = rig.scene.own_call(call_id, rig.scene.hidden);
    let inside = participant(AccountId::generate());
    rig.http.routes.lock().unwrap().own_call = Some(OwnCallState {
        inside: None,
        meeting_grant: None,
        own: own.clone(),
        call: Call {
            id: call_id,
            place,
            started_at: Some(time()),
            lock: None,
            meeting: None,
            hides_occupants: true,
            participants: vec![inside.clone()],
            recording: None,
        },
        entry_requests: Vec::new(),
    });
    rig.http.fail("/api/v1/call", 403, "forbidden");
    peer.event(
        41,
        Event::OwnCallChanged {
            call: Some(own),
            left: None,
        },
    )
    .await;
    let model = rig
        .model_when("the people inside", |model| {
            model
                .call_at(&place)
                .is_some_and(|call| !call.participants.is_empty())
        })
        .await;
    assert_eq!(model.call_at(&place).unwrap().participants, [inside]);
    assert_eq!(rig.http.count("/api/v1/call"), 2);

    // A read that failed for a call that the account has left is not sent again.
    rig.http.fail("/api/v1/call", 403, "forbidden");
    peer.event(
        42,
        Event::CallParticipantJoined {
            call_id,
            place,
            participant: participant(rig.scene.account_id),
        },
    )
    .await;
    soon("the read that fails", async {
        while rig.http.count("/api/v1/call") < 3 {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await;
    peer.event(
        43,
        Event::OwnCallChanged {
            call: None,
            left: None,
        },
    )
    .await;
    rig.model_when("the account has left", |model| model.own_call.is_none())
        .await;
    tokio::time::sleep(Duration::from_secs(130)).await;
    assert_eq!(rig.http.count("/api/v1/call"), 3);
}

#[tokio::test(start_paused = true)]
async fn pings_do_not_move_the_time_for_the_hello() {
    let mut rig = rig();
    rig.want(ConnectionLevel::Selected, "jst_one");
    let mut peer = rig.peer().await;
    let opened = Instant::now();
    peer.beat(Duration::from_secs(5));
    assert!(peer.dropped().await);
    assert_eq!(opened.elapsed(), Duration::from_secs(10));
}

#[tokio::test(start_paused = true)]
async fn a_reload_keeps_only_what_a_snapshot_carries() {
    let mut rig = rig();
    rig.want(ConnectionLevel::Selected, "jst_one");
    let (mut peer, _, _) = rig.served().await;
    let (call_id, place) = (CallId::generate(), rig.scene.place(rig.scene.daily));
    let other = AccountId::generate();
    let call_move = CallMove {
        id: junto_protocol::ids::CallMoveId::generate(),
        account_id: rig.scene.account_id,
        from: place,
        to: rig.scene.place(rig.scene.hidden),
        moved_by: other,
        expires_at: time(),
    };
    let made = CallInvitation {
        id: junto_protocol::ids::CallInvitationId::generate(),
        call_id,
        place,
        account_id: other,
        invited_by: rig.scene.account_id,
        created_at: time(),
        expires_at: time(),
    };
    peer.event(
        41,
        Event::CallMoveRequested {
            call_move: call_move.clone(),
        },
    )
    .await;
    peer.event(42, Event::CallInvited { invitation: made })
        .await;
    let model = rig
        .model_when("the events", |model| model.cursor.position == 42)
        .await;
    assert!(model.moves.len() == 1 && model.invitations.len() == 1);

    // A snapshot carries no move and only the invitations that the account got
    // (ADR 0207). Their outcomes still arrive as notices.
    rig.set_snapshot(50, |_| {});
    let mut notices = rig.live.notices();
    peer.event(43, Event::PermissionsChanged { workspace_id: None })
        .await;
    peer.serve().await;
    let view = rig
        .when("live", |view| view.status == StreamStatus::Live)
        .await;
    let model = view.model.unwrap();
    assert!(model.moves.is_empty() && model.invitations.is_empty());
    peer.event(
        51,
        Event::CallMoveResolved {
            move_id: call_move.id,
            outcome: MoveOutcome::Completed,
        },
    )
    .await;
    loop {
        let (id, event) = next_event(&mut notices).await;
        if id == 51 {
            assert_eq!(event.name(), "call_move_resolved");
            break;
        }
    }
}

#[tokio::test(start_paused = true)]
async fn a_server_that_gives_another_account_at_every_login_waits_as_well() {
    let mut rig = rig();
    for _ in 0..5 {
        rig.http.fail("/snapshot", 401, "unauthenticated");
    }
    let mut asked_at = Vec::new();
    for round in 0..5 {
        let target = rig.target(&format!("jst_{round}"), AccountId::generate());
        rig.desired.send_replace(Desired {
            level: ConnectionLevel::Selected,
            target: Some(target),
        });
        soon("the report", async {
            while rig.host.ended.lock().unwrap().len() <= round {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await;
        asked_at.push(Instant::now());
        while let Ok(mut peer) = rig.peers.try_recv() {
            assert!(peer.dropped().await);
        }
    }
    let mut whole = Duration::from_secs(1);
    for pair in asked_at.windows(2) {
        let waited = pair[1] - pair[0];
        assert!(
            waited >= whole / 2 && waited <= whole + Duration::from_millis(10),
            "{waited:?} of {whole:?}"
        );
        whole *= 2;
    }
}

#[tokio::test(start_paused = true)]
async fn refresh_acknowledges_a_snapshot_requested_after_the_call() {
    let mut rig = rig();
    rig.want(ConnectionLevel::Selected, "jst_one");
    let (_peer, _, _) = rig.served().await;
    rig.set_snapshot(60, |snapshot| snapshot.account.name = "Changed".into());
    let cursor = soon("fresh state", rig.live.refresh()).await.unwrap();
    assert_eq!(cursor.position, 60);
    assert_eq!(rig.model().cursor, cursor);
    assert_eq!(rig.model().account.name, "Changed");
    assert_eq!(rig.http.count("/snapshot"), 2);
}
