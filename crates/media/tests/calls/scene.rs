//! The people of a test: each has a device with a client, a tone as its microphone, a sink
//! that measures what it would hear, and the calls of the client.

use std::sync::Arc;
use std::time::Duration;

use junto_client_core::ServerHandle;
use junto_client_core::flow::access::{AccessAction, AccessState};
use junto_client_core::flow::approve_link::ApproveLinkAction;
use junto_client_core::live::{LiveView, StreamStatus};
use junto_client_core::model::ServerModel;
use junto_client_core::protocol::call::CallPlace;
use junto_client_core::protocol::ids::{AccountId, ChannelId, WorkspaceId};
use junto_client_core::protocol::invite::CreateInvite;
use junto_client_core::protocol::secret::InviteCode;
use junto_client_core::protocol::structure::{ChannelKind, CreateChannel};
use junto_media::Calls;
use junto_media::audio::Processing;
use junto_media::audio::synthetic::{SyntheticHost, tone_level};
use junto_media::calls::{CallTarget, CallTiming, CallsView, MediaOptions, Phase};
use junto_media::engine::VoiceConfig;
use junto_media::permission::FixedPermission;
use junto_media::share::synthetic::SyntheticCapture;
use tokio::sync::{RwLock, RwLockReadGuard, RwLockWriteGuard};

use crate::limit::step;

use crate::support::{Device, TestServer, claim, join, logged_in};

/// How long a step of a test may take. It bounds one step, never a whole server.
pub const PATIENCE: Duration = Duration::from_secs(60);

/// A tone that arrives has about the level at which it was sent: -12 dBFS.
pub const HEARD_DBFS: f32 = -20.0;
/// A tone that does not arrive is below this.
pub const ABSENT_DBFS: f32 = -55.0;

/// The one LiveKit server of the test run. A test that restarts it holds it alone.
static MEDIA_SERVER: RwLock<()> = RwLock::const_new(());

/// The tests that hold the media server now. A test that waits for it notes them as its
/// step, so that a test that does not end names the test that it waits for.
static HOLDERS: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

/// The hold of a test on the media server.
pub struct MediaServer {
    _shared: Option<RwLockReadGuard<'static, ()>>,
    _alone: Option<RwLockWriteGuard<'static, ()>>,
    test: String,
}

impl Drop for MediaServer {
    fn drop(&mut self) {
        if self._alone.is_some() {
            give_the_media_server_back();
        }
        let mut holders = HOLDERS.lock().unwrap();
        if let Some(at) = holders.iter().position(|test| *test == self.test) {
            holders.remove(at);
        }
    }
}

fn holding(
    shared: Option<RwLockReadGuard<'static, ()>>,
    alone: Option<RwLockWriteGuard<'static, ()>>,
) -> MediaServer {
    let test = this_test();
    HOLDERS.lock().unwrap().push(test.clone());
    step("holds the media server");
    // The holders before it had limits of their own.
    crate::limit::restart_limit();
    MediaServer {
        _shared: shared,
        _alone: alone,
        test,
    }
}

fn this_test() -> String {
    std::thread::current().name().unwrap_or("a test").to_owned()
}

/// The longest time that one Docker command of a restore of the media server may take.
const RESTORE_LIMIT: Duration = Duration::from_secs(30);

/// Makes the LiveKit server of the test run run and answer again. A test that holds it
/// alone pauses or stops it; when that test ends in another way than its own last step, by
/// a failure or by its limit, the tests after it would find no media server.
fn give_the_media_server_back() {
    let Ok(container) = std::env::var("JUNTO_TEST_LIVEKIT_CONTAINER") else {
        return;
    };
    // Each command does nothing to a server that runs already.
    for action in ["unpause", "start"] {
        docker_within(&[action, &container], RESTORE_LIMIT);
    }
    let started = std::time::Instant::now();
    while !livekit_answers() {
        if started.elapsed() > PATIENCE {
            eprintln!("after {PATIENCE:?} the media server {container} does not answer");
            return;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// Runs a Docker command from a drop, which cannot wait for a future, and ends it at
/// `limit`.
fn docker_within(arguments: &[&str], limit: Duration) {
    let Ok(mut child) = std::process::Command::new("docker")
        .args(arguments)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    else {
        return;
    };
    let _adopted = crate::limit::adopt(child.id());
    let started = std::time::Instant::now();
    while matches!(child.try_wait(), Ok(None)) {
        if started.elapsed() > limit {
            let _ = child.kill();
            let _ = child.wait();
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Runs a Docker command, such as the pause of the media server by a test that holds it
/// alone, and fails when it fails.
#[allow(dead_code)]
pub async fn docker(arguments: &[&str]) {
    // A test that a limit cancels while the command runs ends the command too.
    let docker = tokio::process::Command::new("docker")
        .args(arguments)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let _adopted = docker.id().map(crate::limit::adopt);
    let done = docker.wait_with_output().await.unwrap();
    assert!(done.status.success(), "{done:?}");
}

/// True when the API of the media server answers a request at once.
pub fn livekit_answers() -> bool {
    std::env::var("JUNTO_TEST_LIVEKIT_API_URL").is_ok_and(|url| answers(&url))
}

/// How long one check of the media server may wait for each of its steps.
const ANSWER_WAIT: Duration = Duration::from_millis(500);

/// True when an HTTP server at `url` answers a request within the waits of a check. The name
/// is looked up on a thread of its own, because a lookup has no time limit of its own.
fn answers(url: &str) -> bool {
    use std::io::{Read, Write};
    use std::net::ToSocketAddrs;
    let authority = url
        .trim_start_matches("http://")
        .split('/')
        .next()
        .unwrap_or_default()
        .to_owned();
    let (found, finding) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = found.send(authority.to_socket_addrs().map(|mut all| all.next()));
    });
    let Ok(Ok(Some(address))) = finding.recv_timeout(ANSWER_WAIT) else {
        return false;
    };
    let Ok(mut stream) = std::net::TcpStream::connect_timeout(&address, ANSWER_WAIT) else {
        return false;
    };
    if stream.set_read_timeout(Some(ANSWER_WAIT)).is_err()
        || stream.set_write_timeout(Some(ANSWER_WAIT)).is_err()
        || stream.write_all(b"GET / HTTP/1.0\r\n\r\n").is_err()
    {
        return false;
    }
    let mut answer = [0u8; 12];
    stream.read_exact(&mut answer).is_ok() && answer.starts_with(b"HTTP/1.")
}

#[test]
fn a_check_of_a_media_server_that_does_not_answer_ends_within_its_waits() {
    // The listener accepts connections into its queue and never answers them.
    let silent = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let started = std::time::Instant::now();
    assert!(!answers(&format!(
        "http://{}",
        silent.local_addr().unwrap()
    )));
    // An address of a private network that no host of a test answers, and a name that the
    // system cannot look up: each step ends at its wait.
    assert!(!answers("http://10.255.255.1:7880"));
    assert!(!answers(
        "http://junto-test-name-that-does-not-exist.invalid:7880"
    ));
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "{:?}",
        started.elapsed()
    );
}

fn note_wait(how: &str) {
    let holders = HOLDERS.lock().unwrap().join(", ");
    step(format!(
        "waits for the media server, {how}; it is held by: {holders}"
    ));
}

/// Waits for the media server, which the test shares with the other tests that do not
/// restart it.
pub async fn media_server_shared() -> MediaServer {
    note_wait("shared");
    holding(Some(MEDIA_SERVER.read().await), None)
}

/// Waits until the test holds the media server alone, to restart it.
pub async fn media_server_alone() -> MediaServer {
    note_wait("alone");
    holding(None, Some(MEDIA_SERVER.write().await))
}

pub struct Person {
    pub device: Device,
    pub calls: Calls,
    pub host: SyntheticHost,
    pub permission: FixedPermission,
    pub capture: SyntheticCapture,
    pub camera: junto_media::camera::synthetic::SyntheticCamera,
    pub hz: f32,
}

/// A steady tone passes unchanged only without the processing of a voice.
pub fn plain_voice() -> VoiceConfig {
    VoiceConfig {
        processing: Processing::OFF,
        ..VoiceConfig::default()
    }
}

impl Person {
    pub fn new(hz: f32) -> Self {
        Self::with_permission(hz, FixedPermission::allowed())
    }

    pub fn with_permission(hz: f32, permission: FixedPermission) -> Self {
        log_into_test_output();
        let device = Device::new();
        let host = SyntheticHost::with_tone(hz);
        let capture = SyntheticCapture::new();
        let camera = junto_media::camera::synthetic::SyntheticCamera::new();
        let calls = Calls::new(
            device.client.clone(),
            MediaOptions {
                camera: Arc::new(camera.clone()),
                host: Arc::new(host.clone()),
                permission: Arc::new(permission.clone()),
                capture: Arc::new(capture.clone()),
                timing: CallTiming {
                    reconnect_first: Duration::from_millis(300),
                    reconnect_longest: Duration::from_secs(2),
                    ..CallTiming::default()
                },
            },
        );
        calls.set_voice(plain_voice());
        device.client.start_event_streams().unwrap();
        Self {
            device,
            calls,
            host,
            permission,
            capture,
            camera,
            hz,
        }
    }

    /// Adds the server to the device as its owner, and shows it.
    pub async fn claim(&self, server: &TestServer, name: &str) -> ServerHandle {
        let handle = self.device.add(&server.address()).await;
        handle.dispatch(claim(server, name)).await.unwrap();
        self.show(&handle).await;
        handle
    }

    /// Adds the server at one of its addresses to the device as a new member, and shows it.
    pub async fn join_at(&self, address: &str, code: &InviteCode, name: &str) -> ServerHandle {
        let handle = self.device.add(address).await;
        handle.dispatch(join(code, name)).await.unwrap();
        self.show(&handle).await;
        handle
    }

    pub async fn show(&self, handle: &ServerHandle) {
        self.device.client.select(Some(handle.id()));
        when(handle, "the stream delivers", |view| {
            view.status == StreamStatus::Live
        })
        .await;
    }

    /// Waits until the view of the calls fulfils a condition.
    pub async fn until(&self, what: &str, holds: impl FnMut(&CallsView) -> bool) -> CallsView {
        step(what);
        let mut watch = self.calls.watch();
        match tokio::time::timeout(PATIENCE, watch.wait_for(holds)).await {
            Ok(Ok(view)) => view.clone(),
            _ => panic!(
                "after {PATIENCE:?} this did not hold: {what}; the view is {:?}",
                self.calls.view()
            ),
        }
    }

    pub async fn enter(&self, target: CallTarget) {
        step("asks to join the call");
        self.calls.join(target).await.unwrap();
        self.until("the media connection of the call", |view| {
            view.call
                .as_ref()
                .is_some_and(|call| call.target == target && call.phase == Phase::Connected)
        })
        .await;
    }

    /// The level of a tone in the last half second of what this person would hear.
    pub fn hears(&self, hz: f32) -> f32 {
        let (played, rate) = self.host.played(Duration::from_millis(500));
        tone_level(&played, rate, hz)
    }

    /// The level of a tone in the last half second of the right channel.
    pub fn hears_right(&self, hz: f32) -> f32 {
        let (played, rate) = self.host.played_of(Duration::from_millis(500), 1);
        tone_level(&played, rate, hz)
    }

    pub async fn until_heard(&self, of: &Person) -> f32 {
        self.until_level(of.hz, "the tone arrives", |level| level > HEARD_DBFS)
            .await
    }

    /// Waits for the tone in sound that was all played after this call. A test uses it
    /// after a change that the sound from before the change must not answer.
    pub async fn until_heard_afresh(&self, of: &Person) -> f32 {
        self.host.clear_played();
        self.until_heard(of).await
    }

    pub async fn until_silent(&self, of: &Person) -> f32 {
        self.until_level(of.hz, "the tone has stopped", |level| level < ABSENT_DBFS)
            .await
    }

    /// Waits until the tone of another person arrives at a level that does not change any
    /// more, and returns that level.
    pub async fn until_steady(&self, of: &Person) -> f32 {
        step(format!("the tone of {} Hz is steady", of.hz));
        let started = tokio::time::Instant::now();
        let mut before = f32::MIN;
        loop {
            let level = self.hears(of.hz);
            if level > HEARD_DBFS && (level - before).abs() < 0.3 {
                return level;
            }
            assert!(
                started.elapsed() < PATIENCE,
                "after {PATIENCE:?} the tone of {} Hz is not steady: {level} dBFS",
                of.hz
            );
            before = level;
            tokio::time::sleep(Duration::from_millis(600)).await;
        }
    }

    pub async fn until_level(&self, hz: f32, what: &str, holds: impl Fn(f32) -> bool) -> f32 {
        step(format!("{what}: {hz} Hz"));
        let started = tokio::time::Instant::now();
        loop {
            let level = self.hears(hz);
            if holds(level) {
                return level;
            }
            assert!(
                started.elapsed() < PATIENCE,
                "after {PATIENCE:?} this did not hold: {what}; {hz} Hz is at {level} dBFS, and \
                 the view is {:?}",
                self.calls.view()
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
}

/// A line of the call library about the negotiation of a publisher.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NegotiationLine {
    /// A task of the fast publish mode starts.
    TaskStarted,
    /// A negotiation starts, in a task or after the wait of the debounced mode.
    Negotiating,
    /// An offer was sent, and its answer is awaited.
    OfferSent,
    /// A task of the fast publish mode ends.
    TaskEnded,
    /// A negotiation failed (an error).
    Failed,
    /// An offer was asked of a closed connection (a warning).
    OfferInClosedConnection,
}

/// The lines about the negotiation of a publisher that the call library logged in this test
/// program, with the time at which each arrived. A test reads the lines from the length that
/// the list had when it started, so it counts only its own; one that reads them holds the
/// media server alone in a test program of its own, so that no other test negotiates in the
/// process and no task of another test is cancelled with its lines half written.
static NEGOTIATION_LINES: std::sync::Mutex<Vec<(std::time::Instant, NegotiationLine)>> =
    std::sync::Mutex::new(Vec::new());

/// The lines about the negotiation of a publisher from position `from` of the list on.
#[allow(dead_code)]
pub fn negotiation_lines(from: usize) -> Vec<(std::time::Instant, NegotiationLine)> {
    let lines = NEGOTIATION_LINES.lock().unwrap();
    lines.get(from..).unwrap_or_default().to_vec()
}

/// The number of lines about the negotiation of a publisher so far: the start of a count.
#[allow(dead_code)]
pub fn negotiation_line_count() -> usize {
    NEGOTIATION_LINES.lock().unwrap().len()
}

/// Notes the lines of the call library about the negotiation of a publisher.
struct Negotiations;

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for Negotiations {
    fn on_event(&self, event: &tracing::Event<'_>, _: tracing_subscriber::layer::Context<'_, S>) {
        struct Message(String);
        impl tracing::field::Visit for Message {
            fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
                if field.name() == "message" {
                    self.0 = format!("{value:?}");
                }
            }
        }
        let mut message = Message(String::new());
        event.record(&mut message);
        let text = message.0.as_str();
        let line = if *event.metadata().level() == tracing::Level::ERROR
            && text.contains("failed to negotiate the publisher")
        {
            NegotiationLine::Failed
        } else if text.starts_with("negotiating the publisher") {
            NegotiationLine::Negotiating
        } else if text == "queue_negotiation: starting new negotiation" {
            NegotiationLine::TaskStarted
        } else if text == "negotiation completed" {
            NegotiationLine::TaskEnded
        } else if text.starts_with("offer sent, waiting for answer") {
            NegotiationLine::OfferSent
        } else if text.contains("peer connection is closed, cannot create offer") {
            NegotiationLine::OfferInClosedConnection
        } else {
            return;
        };
        NEGOTIATION_LINES
            .lock()
            .unwrap()
            .push((std::time::Instant::now(), line));
    }
}

/// Keep the engine's progress and shutdown logs with the test that owns its threads.
fn log_into_test_output() {
    use tracing_subscriber::Layer;
    use tracing_subscriber::filter::LevelFilter;
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let level = std::env::var("JUNTO_LOG_LEVEL")
            .ok()
            .and_then(|level| level.parse::<tracing::Level>().ok())
            .unwrap_or(tracing::Level::INFO);
        let _ = tracing_subscriber::registry()
            .with(
                tracing_subscriber::fmt::layer()
                    .with_test_writer()
                    .with_filter(LevelFilter::from_level(level)),
            )
            .with(
                Negotiations.with_filter(
                    tracing_subscriber::filter::Targets::new()
                        .with_target("livekit::rtc_engine::rtc_session", tracing::Level::DEBUG)
                        .with_target("livekit::rtc_engine::peer_transport", tracing::Level::WARN),
                ),
            )
            .try_init();
    });
}

pub async fn when(
    handle: &ServerHandle,
    what: &str,
    holds: impl FnMut(&LiveView) -> bool,
) -> LiveView {
    step(what);
    let mut watch = handle.live().watch();
    match tokio::time::timeout(PATIENCE, watch.wait_for(holds)).await {
        Ok(Some(view)) => view,
        _ => panic!(
            "after {PATIENCE:?} this did not hold: {what}; the status is {:?}",
            handle.live().view().status
        ),
    }
}

pub async fn model_when(
    handle: &ServerHandle,
    what: &str,
    mut holds: impl FnMut(&ServerModel) -> bool,
) -> Arc<ServerModel> {
    let view = when(handle, what, |view| {
        view.model.as_deref().is_some_and(&mut holds)
    })
    .await;
    view.model.unwrap()
}

pub async fn invite(owner: &ServerHandle, uses: u32) -> InviteCode {
    owner
        .api()
        .create_invite(&CreateInvite {
            max_uses: Some(uses),
            ..CreateInvite::default()
        })
        .await
        .unwrap()
        .code
}

/// A server with one voice channel, and its place as the target of a join.
pub struct Room {
    pub server: TestServer,
    pub workspace_id: WorkspaceId,
    pub voice: ChannelId,
    pub target: CallTarget,
}

impl Room {
    /// Starts a server, lets `owner` claim it and creates the voice channel.
    pub async fn new(name: &str, owner: &Person, owner_name: &str) -> (Self, ServerHandle) {
        Self::with_server(TestServer::start_with_media(name).await, owner, owner_name).await
    }

    /// As [`Room::new`], with a server whose clients reach the media server at `address`:
    /// the address of a proxy.
    pub async fn with_media_at(
        name: &str,
        owner: &Person,
        owner_name: &str,
        address: &str,
    ) -> (Self, ServerHandle) {
        let server = TestServer::start_with_media_at(name, address).await;
        Self::with_server(server, owner, owner_name).await
    }

    async fn with_server(
        server: TestServer,
        owner: &Person,
        owner_name: &str,
    ) -> (Self, ServerHandle) {
        let handle = owner.claim(&server, owner_name).await;
        let workspace_id = handle.live().view().model.unwrap().workspaces[0]
            .workspace
            .id;
        let voice = handle
            .api()
            .create_channel(
                workspace_id,
                &CreateChannel {
                    name: "Daily".to_owned(),
                    kind: ChannelKind::Voice,
                    category_id: None,
                    position: None,
                },
            )
            .await
            .unwrap()
            .id;
        let place = CallPlace::Channel {
            workspace_id,
            channel_id: voice,
        };
        let target = CallTarget {
            server_id: handle.id(),
            place,
        };
        (
            Self {
                server,
                workspace_id,
                voice,
                target,
            },
            handle,
        )
    }

    /// A member of the server on the device of `person`.
    pub async fn member(&self, owner: &ServerHandle, person: &Person, name: &str) -> ServerHandle {
        self.member_at(owner, person, name, &self.server.address())
            .await
    }

    /// A member whose device reaches the server at `address`: the address of a proxy.
    pub async fn member_at(
        &self,
        owner: &ServerHandle,
        person: &Person,
        name: &str,
        address: &str,
    ) -> ServerHandle {
        let code = invite(owner, 1).await;
        let handle = person.join_at(address, &code, name).await;
        model_when(&handle, "the voice channel", |model| {
            model.channel(self.voice).is_some()
        })
        .await;
        handle
    }

    /// Links the device of `second` to the account that `first` is logged in with.
    pub async fn second_device(&self, first: &ServerHandle, second: &Person) -> ServerHandle {
        let handle = second.device.add(&self.server.address()).await;
        handle
            .dispatch(AccessAction::RequestLink {
                device_name: "Second".to_owned(),
            })
            .await
            .unwrap();
        let AccessState::WaitingForLink { code, .. } = handle.access().state else {
            panic!("not waiting for a link");
        };
        let approval = first.approve_link();
        approval
            .dispatch(ApproveLinkAction::LookUp { code })
            .await
            .unwrap();
        approval.dispatch(ApproveLinkAction::Approve).await.unwrap();
        handle.dispatch(AccessAction::Check).await.unwrap();
        assert_eq!(logged_in(&handle), logged_in(first));
        second.show(&handle).await;
        model_when(&handle, "the voice channel", |model| {
            model.channel(self.voice).is_some()
        })
        .await;
        handle
    }

    /// A second voice channel of the server, and its place as the target of a join.
    pub async fn second_voice(&self, owner: &ServerHandle, name: &str) -> CallTarget {
        let channel_id = owner
            .api()
            .create_channel(
                self.workspace_id,
                &CreateChannel {
                    name: name.to_owned(),
                    kind: ChannelKind::Voice,
                    category_id: None,
                    position: None,
                },
            )
            .await
            .unwrap()
            .id;
        CallTarget {
            server_id: self.target.server_id,
            place: CallPlace::Channel {
                workspace_id: self.workspace_id,
                channel_id,
            },
        }
    }

    /// The path of the request for a new token of the seat `identity` in a voice channel
    /// (ADR 0520).
    pub fn renew_path(target: &CallTarget, identity: &str) -> String {
        let named: String = url::form_urlencoded::byte_serialize(identity.as_bytes()).collect();
        format!("{}?renews={named}", Self::join_path(target))
    }

    /// The path of the request for a place in a voice channel.
    pub fn join_path(target: &CallTarget) -> String {
        match target.place {
            CallPlace::Channel { channel_id, .. } => {
                format!("/api/v1/channels/{channel_id}/call/join")
            }
            CallPlace::Direct { .. } => panic!("a voice channel"),
        }
    }
}

/// Waits until `holds`, for a state that no watch tells.
pub async fn until(what: &str, holds: impl Fn() -> bool) {
    step(what);
    let started = tokio::time::Instant::now();
    while !holds() {
        assert!(
            started.elapsed() < PATIENCE,
            "after {PATIENCE:?} this did not hold: {what}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

pub fn account(handle: &ServerHandle) -> AccountId {
    logged_in(handle)
}

#[tokio::test]
async fn a_test_that_waits_for_the_media_server_notes_the_tests_that_hold_it() {
    let held = media_server_shared().await;
    let holder = this_test();
    let noted = std::thread::Builder::new()
        .name("a waiting test".to_owned())
        .spawn(|| {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            let waited = runtime.block_on(async {
                tokio::time::timeout(Duration::from_millis(100), media_server_alone()).await
            });
            assert!(waited.is_err(), "the media server is held");
            crate::limit::last_steps()
        })
        .unwrap()
        .join()
        .unwrap();
    assert!(
        noted.contains(&format!(
            "waits for the media server, alone; it is held by: {holder}"
        )),
        "{noted}"
    );
    drop(held);
    assert!(!HOLDERS.lock().unwrap().contains(&holder));
}
