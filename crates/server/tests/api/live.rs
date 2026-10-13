//! The second tier of the tests of calls: against a real LiveKit server, with real
//! participants (ADR 0902). The tests are ignored in the plain run and run with:
//!
//! ```sh
//! (cd spikes/test-participant && ../../tools/serial.sh cargo build --release)
//! tools/with-test-postgres.sh tools/with-test-livekit.sh tools/serial.sh \
//!     cargo test -p junto-server --test api live:: -- --ignored
//! ```

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration as Wait, Instant};

use junto_protocol::call::{BlockReason, CallJoin, LeaveReason};
use junto_protocol::error::ErrorCode;
use junto_protocol::events::Event;
use junto_protocol::secret::CallToken;
use junto_server::config::{Config, MediaSecret, MediaSettings};
use junto_server::media::{Participant, Publishing, TrackSource};
use reqwest::StatusCode;
use serde_json::Value;
use time::Duration;

use crate::calls_support::{Scene, names};
use crate::support::TestServer;

const TOOL: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../spikes/test-participant/target/release/test-participant"
);
const PATIENCE: Wait = Wait::from_secs(30);
const REASON: &str = "needs a LiveKit server: run through tools/with-test-livekit.sh";

fn setting(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| {
        panic!(
            "{name} is not set. Run the tests of this tier through tools/with-test-livekit.sh, \
             as crates/server/README.md describes."
        )
    })
}

fn media(config: &mut Config) {
    config.media = Some(MediaSettings {
        api_url: setting("JUNTO_TEST_LIVEKIT_API_URL"),
        client_url: setting("JUNTO_TEST_LIVEKIT_URL"),
        api_key: setting("JUNTO_TEST_LIVEKIT_API_KEY"),
        api_secret: MediaSecret::new(setting("JUNTO_TEST_LIVEKIT_API_SECRET")),
        request_timeout: Duration::seconds(2),
    });
    config.limits.calls.check_interval = Duration::milliseconds(500);
    config.limits.calls.absence_grace = Duration::seconds(3);
}

async fn scene() -> Scene {
    Scene::on(TestServer::start_live(media, None).await).await
}

async fn until(what: &str, mut holds: impl AsyncFnMut() -> bool) {
    let started = Instant::now();
    while !holds().await {
        assert!(
            started.elapsed() < PATIENCE,
            "after {PATIENCE:?} this did not hold: {what}"
        );
        tokio::time::sleep(Wait::from_millis(100)).await;
    }
}

/// A process of the test participant of `spikes/test-participant`.
struct Client {
    child: Child,
    lines: Arc<Mutex<Vec<Value>>>,
}

impl Client {
    fn start(command: &str, url: &str, token: &CallToken, arguments: &[&str]) -> Self {
        let tool = std::env::var("JUNTO_TEST_PARTICIPANT").unwrap_or_else(|_| TOOL.to_owned());
        assert!(
            std::path::Path::new(&tool).exists(),
            "{tool} is missing. Build it once: (cd spikes/test-participant && \
             ../../tools/serial.sh cargo build --release)"
        );
        let mut child = Command::new(&tool)
            .arg(command)
            .args(["--url", url, "--token", token.expose()])
            .args(arguments)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let output = child.stdout.take().unwrap();
        let lines = Arc::new(Mutex::new(Vec::new()));
        let kept = lines.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(output).lines().map_while(Result::ok) {
                if let Ok(line) = serde_json::from_str::<Value>(&line) {
                    kept.lock().unwrap().push(line);
                }
            }
        });
        Self { child, lines }
    }

    fn publisher(join: &CallJoin, arguments: &[&str]) -> Self {
        Self::start("publish", &join.media_url, &join.token, arguments)
    }

    fn lines(&self) -> Vec<Value> {
        self.lines.lock().unwrap().clone()
    }

    fn saw(&self, event: &str) -> bool {
        self.lines().iter().any(|line| line["event"] == event)
    }

    /// Waits until the process printed `event`.
    async fn until(&self, event: &str) {
        let started = Instant::now();
        while !self.saw(event) {
            assert!(
                started.elapsed() < PATIENCE,
                "the participant did not report {event:?}; it reported {:?}",
                self.lines()
            );
            tokio::time::sleep(Wait::from_millis(100)).await;
        }
    }

    /// Waits until the process has ended.
    async fn ended(&mut self) -> ExitStatus {
        let started = Instant::now();
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                return status;
            }
            assert!(
                started.elapsed() < PATIENCE,
                "the participant did not end; it reported {:?}",
                self.lines()
            );
            tokio::time::sleep(Wait::from_millis(100)).await;
        }
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

async fn listed(server: &TestServer, room: &str) -> Vec<Participant> {
    let media = server.state.calls.media().unwrap();
    media.list_participants(room).await.unwrap()
}

fn room_of(join: &CallJoin) -> String {
    let payload = join.token.expose().split('.').nth(1).unwrap();
    let decoded =
        base64::Engine::decode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, payload).unwrap();
    let claims: Value = serde_json::from_slice(&decoded).unwrap();
    claims["video"]["room"].as_str().unwrap().to_owned()
}

#[tokio::test]
#[ignore = "needs a LiveKit server: run through tools/with-test-livekit.sh"]
async fn livekit_accepts_the_join_token_and_its_participant_list_drives_who_is_in_the_call() {
    let _ = REASON;
    let scene = scene().await;
    let (server, ann, ben) = (&scene.server, &scene.ann, &scene.ben);
    let voice = scene.voice.id;
    until("the first check has reached LiveKit", async || {
        server.state.calls.reachable() == Some(true)
    })
    .await;
    let mut to_ben = server.full_stream(ben).await;

    let join = server.join_call(ann, voice).await;
    let room = room_of(&join);
    assert!(server.occupants(ben, voice).await.is_empty());

    let token = join.token.expose();
    let signature = token.rfind('.').unwrap() + 1;
    let first = if token[signature..].starts_with('A') {
        "B"
    } else {
        "A"
    };
    let forged = format!("{}{first}{}", &token[..signature], &token[signature + 1..]);
    let mut refused = Client::start(
        "publish",
        &join.media_url,
        &CallToken::new(forged),
        &["--leave-after", "5"],
    );
    assert!(!refused.ended().await.success());
    assert!(!refused.saw("joined"), "{:?}", refused.lines());

    let client = Client::publisher(&join, &["--mic-hz", "440"]);
    client.until("joined").await;
    client.until("published").await;
    until("the person is in the call", async || {
        server.occupants(ben, voice).await == [ann.account_id]
    })
    .await;
    let seen = server.call_events(ben, &mut to_ben).await;
    assert_eq!(names(&seen), ["call_started", "call_participant_joined"]);
    let Event::CallParticipantJoined { participant, .. } = &seen[1] else {
        panic!("{seen:?}");
    };
    assert_eq!(participant.identity, join.identity);

    let participants = listed(server, &room).await;
    assert_eq!(participants.len(), 1);
    assert_eq!(participants[0].identity, join.identity);
    assert_eq!(participants[0].publishing, Some(Publishing::open()));
    assert!(participants[0].publishes(TrackSource::Microphone));
    let microphone = participants[0]
        .tracks
        .iter()
        .find(|track| track.source == TrackSource::Microphone)
        .unwrap();
    assert!(microphone.id.starts_with("TR_"), "{:?}", microphone.id);

    drop(client);
    until("the person who lost the connection has left", async || {
        !server.channel_call(ben, voice).await.in_use
    })
    .await;
    let after = server.call_events(ben, &mut to_ben).await;
    assert!(matches!(
        after.as_slice(),
        [
            Event::CallParticipantLeft {
                reason: LeaveReason::ConnectionLost,
                ..
            },
            Event::CallEnded { .. }
        ]
    ));
}

#[tokio::test]
#[ignore = "needs a LiveKit server: run through tools/with-test-livekit.sh"]
async fn a_removed_person_is_thrown_out_by_the_repeated_check_each_time_they_return() {
    let scene = scene().await;
    let (server, owner, ann, ben) = (&scene.server, &scene.owner, &scene.ann, &scene.ben);
    let voice = scene.voice.id;
    until("the first check has reached LiveKit", async || {
        server.state.calls.reachable() == Some(true)
    })
    .await;

    let anchor_join = server.join_call(ben, voice).await;
    let anchor = Client::publisher(&anchor_join, &["--no-mic"]);
    anchor.until("joined").await;
    let join = server.join_call(ann, voice).await;
    let room = room_of(&join);
    let client = Client::publisher(&join, &["--mic-hz", "440"]);
    client.until("joined").await;
    until("both are in the call", async || {
        server.occupants(owner, voice).await.len() == 2
    })
    .await;

    server
        .delete(
            owner,
            &format!(
                "/api/v1/channels/{voice}/call/participants/{}",
                ann.account_id
            ),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    client.until("disconnected").await;
    assert_eq!(server.occupants(owner, voice).await, [ben.account_id]);

    for attempt in 1..=2 {
        let back = Client::publisher(&join, &["--mic-hz", "440"]);
        back.until("joined").await;
        back.until("disconnected").await;
        assert_eq!(
            server.occupants(owner, voice).await,
            [ben.account_id],
            "return {attempt}"
        );
    }
    until("only the allowed person is left in the room", async || {
        let identities: Vec<String> = listed(server, &room)
            .await
            .into_iter()
            .map(|participant| participant.identity)
            .collect();
        identities == std::slice::from_ref(&anchor_join.identity)
    })
    .await;
    assert!(!anchor.saw("disconnected"));
    assert_eq!(server.own_call(ann).await.call, None);
}

#[tokio::test]
#[ignore = "needs a LiveKit server: run through tools/with-test-livekit.sh"]
async fn a_microphone_block_stops_the_participant_from_publishing() {
    let scene = scene().await;
    let (server, owner, ann, ben) = (&scene.server, &scene.owner, &scene.ann, &scene.ben);
    let voice = scene.voice.id;
    until("the first check has reached LiveKit", async || {
        server.state.calls.reachable() == Some(true)
    })
    .await;

    let join = server.join_call(ann, voice).await;
    let room = room_of(&join);
    let speaker = Client::publisher(&join, &["--mic-hz", "440", "--camera"]);
    speaker.until("joined").await;
    let expected = format!("{}/microphone=440", join.identity);
    let listen = |listener: CallJoin| {
        Client::start(
            "subscribe",
            &listener.media_url,
            &listener.token,
            &["--duration", "6", "--expect", &expected],
        )
    };
    until("the speaker publishes the microphone", async || {
        listed(server, &room)
            .await
            .iter()
            .any(|participant| participant.publishes(TrackSource::Microphone))
    })
    .await;
    let mut heard = listen(server.join_call(ben, voice).await);
    assert!(heard.ended().await.success(), "{:?}", heard.lines());

    let block = format!(
        "/api/v1/channels/{voice}/call/participants/{}/microphone-block",
        ann.account_id
    );
    server
        .put_empty(owner, &block)
        .await
        .empty(StatusCode::NO_CONTENT);
    until(
        "LiveKit lists no microphone track of the blocked person",
        async || {
            let participants = listed(server, &room).await;
            participants
                .iter()
                .find(|participant| participant.identity == join.identity)
                .is_some_and(|participant| {
                    participant.publishing == Some(Publishing::pictures_only())
                        && !participant.publishes(TrackSource::Microphone)
                        && participant.publishes(TrackSource::Camera)
                })
        },
    )
    .await;
    let mut silent = listen(server.join_call(ben, voice).await);
    assert!(!silent.ended().await.success(), "{:?}", silent.lines());
    let summary = silent.lines().pop().unwrap();
    assert!(
        !summary["failures"].as_array().unwrap().is_empty(),
        "{summary}"
    );

    let shown = server
        .channel_call(owner, voice)
        .await
        .call
        .unwrap()
        .participants
        .into_iter()
        .find(|participant| participant.account_id == ann.account_id)
        .unwrap();
    assert_eq!(
        shown.microphone_block.unwrap().reason,
        BlockReason::Moderator
    );
    assert!(!speaker.saw("disconnected"));

    server.leave_call(ann).await;
    speaker.until("disconnected").await;
    let again = server.join_call(ann, voice).await;
    let blocked = Client::publisher(&again, &["--no-mic", "--camera"]);
    blocked.until("joined").await;
    until(
        "the new connection of the blocked person has no sound source",
        async || {
            listed(server, &room)
                .await
                .iter()
                .find(|participant| participant.identity == again.identity)
                .is_some_and(|participant| {
                    participant.publishing == Some(Publishing::pictures_only())
                })
        },
    )
    .await;
    server
        .call_join_reply(owner, voice)
        .await
        .ok::<Value>(StatusCode::OK);
    let _ = ErrorCode::NotInCall;
}

#[tokio::test]
#[ignore = "needs a LiveKit server: run through tools/with-test-livekit.sh"]
async fn a_notification_of_livekit_is_verified_and_starts_a_check() {
    let listen = setting("JUNTO_TEST_WEBHOOK_ADDRESS");
    let configure = |config: &mut Config| {
        media(config);
        config.limits.calls.check_interval = Duration::hours(1);
    };
    let scene = Scene::on(TestServer::start_live(configure, Some(listen)).await).await;
    let (server, ann, ben) = (&scene.server, &scene.ann, &scene.ben);
    let voice = scene.voice.id;
    until("the first check has reached LiveKit", async || {
        server.state.calls.reachable() == Some(true)
    })
    .await;

    let join = server.join_call(ann, voice).await;
    tokio::time::sleep(Wait::from_secs(1)).await;
    assert!(server.occupants(ben, voice).await.is_empty());
    let client = Client::publisher(&join, &["--no-mic"]);
    client.until("joined").await;
    until(
        "a notification of LiveKit made the check see the person",
        async || server.occupants(ben, voice).await == [ann.account_id],
    )
    .await;
}
