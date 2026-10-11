//! People in a voice channel, with a real media server and a real participant (ADR 0902).
//! The test is ignored in the plain run and runs with:
//!
//! ```sh
//! tools/with-test-postgres.sh tools/with-test-livekit.sh tools/serial.sh \
//!     cargo test -p junto-client --test app live:: -- --ignored
//! ```
//!
//! The participant is the tool of `spikes/test-participant`, as in the tests of the client
//! core, or the binary that `JUNTO_TEST_PARTICIPANT_BIN` names.

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};

use gpui::TestAppContext;
use junto_client::state::view::{Microphone, Occupant};
use junto_client_core::protocol::call::CallJoin;
use junto_client_core::protocol::structure::ChannelKind;

use crate::harness::{Window, World, member};

fn participant_tool() -> PathBuf {
    let path = std::env::var_os("JUNTO_TEST_PARTICIPANT_BIN").map_or_else(
        || {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../spikes/test-participant/target/release/test-participant")
        },
        PathBuf::from,
    );
    assert!(
        path.exists(),
        "{} is missing. Build it once: (cd spikes/test-participant && ../../tools/serial.sh \
         cargo build --release), or set JUNTO_TEST_PARTICIPANT_BIN.",
        path.display()
    );
    path
}

/// A process that joins the room of a call with the token of a join and publishes sound.
struct Participant(Child);

impl Participant {
    fn start(join: &CallJoin) -> Self {
        let child = Command::new(participant_tool())
            .arg("publish")
            .args(["--url", &join.media_url, "--token", join.token.expose()])
            .args(["--leave-after", "300"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        Self(child)
    }
}

impl Drop for Participant {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn occupants(window: &mut Window) -> Vec<Occupant> {
    window
        .channels()
        .into_iter()
        .find(|row| row.kind == ChannelKind::Voice)
        .and_then(|row| row.voice)
        .map(|voice| voice.occupants)
        .unwrap_or_default()
}

#[gpui::test]
#[ignore = "needs a LiveKit server: run through tools/with-test-livekit.sh"]
fn a_person_who_enters_and_leaves_a_voice_channel_is_shown_with_the_marks(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server_with_media("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    let voice = window
        .channels()
        .into_iter()
        .find(|row| row.kind == ChannelKind::Voice)
        .unwrap();
    assert!(occupants(&mut window).is_empty());
    assert!(!voice.voice.as_ref().unwrap().in_use);

    // The administrator enters the call. The window lists the person under the voice
    // channel, with the name from the members of the workspace.
    let api = admin.handle.api();
    let join = world.run(api.join_channel_call(voice.id)).unwrap();
    let participant = Participant::start(&join);
    window.until("the person is listed", |window| {
        occupants(window)
            .first()
            .is_some_and(|occupant| occupant.name.as_deref() == Some("Ada"))
    });
    let shown = occupants(&mut window);
    assert_eq!(shown.len(), 1);
    assert!(!shown[0].is_self);
    assert!(matches!(shown[0].microphone, Microphone::Open { .. }));
    let state = window
        .channels()
        .into_iter()
        .find(|row| row.id == voice.id)
        .and_then(|row| row.voice)
        .unwrap();
    assert!(state.in_use && !state.locked && !state.recording);

    // A closed microphone, a raised hand and a lock reach the list as marks.
    world.run(api.report_call_state(true, false)).unwrap();
    world.run(api.raise_hand()).unwrap();
    world.run(api.lock_call(voice.id)).unwrap();
    window.until("the marks are shown", |window| {
        let state = window
            .channels()
            .into_iter()
            .find(|row| row.id == voice.id)
            .and_then(|row| row.voice);
        state.is_some_and(|state| {
            state.locked
                && state.occupants.first().is_some_and(|occupant| {
                    occupant.microphone == Microphone::Closed && occupant.hand_raised
                })
        })
    });

    // The person leaves, and the list is empty again.
    world.run(api.leave_call()).unwrap();
    drop(participant);
    window.until("the person is gone", |window| occupants(window).is_empty());
}
