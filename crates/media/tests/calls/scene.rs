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
use tokio::sync::RwLock;

use crate::support::{Device, TestServer, claim, join, logged_in};

/// How long a step of a test may take. It bounds one step, never a whole server.
pub const PATIENCE: Duration = Duration::from_secs(60);

/// A tone that arrives has about the level at which it was sent: -12 dBFS.
pub const HEARD_DBFS: f32 = -20.0;
/// A tone that does not arrive is below this.
pub const ABSENT_DBFS: f32 = -55.0;

/// The one LiveKit server of the test run. A test that restarts it holds it alone.
pub static MEDIA_SERVER: RwLock<()> = RwLock::const_new(());

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

/// Keep the engine's progress and shutdown logs with the test that owns its threads.
fn log_into_test_output() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let level = std::env::var("JUNTO_LOG_LEVEL")
            .ok()
            .and_then(|level| level.parse::<tracing::Level>().ok())
            .unwrap_or(tracing::Level::INFO);
        let _ = tracing_subscriber::fmt()
            .with_test_writer()
            .with_max_level(level)
            .try_init();
    });
}

pub async fn when(
    handle: &ServerHandle,
    what: &str,
    holds: impl FnMut(&LiveView) -> bool,
) -> LiveView {
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
        let server = TestServer::start_with_media(name).await;
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
