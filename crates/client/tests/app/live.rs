//! A call in the window, with a real media server (ADR 0902). The microphone of each side
//! is a tone and its output a sink that measures: no test opens a real device. The tests
//! are ignored in the plain run and run with:
//!
//! ```sh
//! tools/with-test-postgres.sh tools/with-test-livekit.sh tools/serial.sh \
//!     cargo test -p junto-client --test app live:: -- --ignored
//! ```

use std::time::Duration;

use gpui::{Modifiers, TestAppContext};
use junto_client::shell::call::CallTab;
use junto_client::state::view::{
    ChannelRow, Microphone, MicrophoneButton, MicrophoneLine, Occupant, OwnShare, Presence,
    Relation, ShareStage, SoundWarning,
};
use junto_client_core::protocol::WorkspacePermission;
use junto_client_core::protocol::call::{BlockReason, CallPlace, ShareAudio};
use junto_client_core::protocol::ids::ChannelId;
use junto_client_core::protocol::roles::UpdateWorkspaceRole;
use junto_client_core::protocol::structure::ChannelKind;
use junto_media::calls::{CallTarget, Phase};
use junto_media::engine::ShareId;
use junto_media::share::synthetic::SyntheticCapture;
use junto_media::share::{
    CaptureError, CaptureRequest, PictureSource, Resolution, ScreenAccess, ShareKind, ShareQuality,
    SoundChoice,
};

use crate::calls::{voice_channel, voice_file};
use crate::harness::{
    ABSENT_DBFS, ADMIN_TONE_HZ, Admin, Caller, HEARD_DBFS, PATIENCE, WINDOW_TONE_HZ, Window, World,
    heard, member,
};

mod cameras;
mod detached;
mod direct;
mod lines;
#[cfg(all(target_os = "macos", feature = "native-device-tests"))]
mod native_share;
mod rules;
mod sound_help;
#[cfg(target_os = "windows")]
mod windows_share;

/// The voice channel of the tests. A new server has a voice channel of its own beside it.
const ROOM: &str = "Daily";

fn room(window: &mut Window) -> ChannelRow {
    window
        .channels()
        .into_iter()
        .find(|row| row.kind == ChannelKind::Voice && row.name == ROOM)
        .unwrap()
}

/// Which side of the call hears: the window hears the tone of the administrator, and the
/// administrator the tone of the window.
#[derive(Debug, Clone, Copy)]
enum Hears {
    Window,
    Administrator,
}

/// What a sink was given in the last 600 ms: one character for each 10 ms, the oldest
/// first, a number sign for sound and a point for silence.
fn sink_line(sound: &junto_media::audio::synthetic::SyntheticHost) -> String {
    let (played, _) = sound.played(Duration::from_millis(600));
    played
        .as_chunks::<480>()
        .0
        .iter()
        .map(|part| {
            if junto_media::audio::synthetic::level(part) > -40.0 {
                '#'
            } else {
                '.'
            }
        })
        .collect()
}

/// What both engines know of the sound at this moment, for an assertion that failed.
fn sound_report(world: &World, window: &mut Window, caller: &Caller) -> String {
    let sinks = (sink_line(&window.sound), sink_line(&caller.sound));
    let calls = window
        .state
        .read_with(&window.cx, |state, _| state.calls().clone());
    let line = |check: Option<_>| match check {
        Some(check) => format!("{check}"),
        None => "  no call".to_owned(),
    };
    let checks = (
        line(world.run(calls.sound_check())),
        line(world.run(caller.calls.sound_check())),
    );
    format!(
        "each character is 10 ms of the last 600 ms, the oldest first\n\
         the sink of the window:        {}\nthe window:\n{}\n\
         the sink of the administrator: {}\nthe administrator:\n{}",
        sinks.0, checks.0, sinks.1, checks.1
    )
}

/// Waits until one side hears the other, and fails with what both engines know when the
/// tone does not come. A single reading would also fail on a moment in which the threads
/// of the test did not run: the half second that it measures then holds that gap.
#[track_caller]
fn until_hears(world: &World, window: &mut Window, caller: &Caller, side: Hears, after: &str) {
    let deadline = std::time::Instant::now() + PATIENCE;
    loop {
        window.cx.run_until_parked();
        let level = match side {
            Hears::Window => heard(&window.sound, ADMIN_TONE_HZ),
            Hears::Administrator => heard(&caller.sound, WINDOW_TONE_HZ),
        };
        if level > HEARD_DBFS {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "after {PATIENCE:?} {side:?} hears the other tone at {level} dBFS {after}\n{}",
            sound_report(world, window, caller)
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn occupants(window: &mut Window) -> Vec<Occupant> {
    room(window)
        .voice
        .map(|voice| voice.occupants)
        .unwrap_or_default()
}

fn own(window: &mut Window) -> Option<Occupant> {
    occupants(window)
        .into_iter()
        .find(|occupant| occupant.is_self)
}

/// A steady tone passes unchanged only without the processing of a voice: the person turns
/// the three parts off in the settings of the application.
fn turn_processing_off(window: &mut Window) {
    window.click("app-settings");
    for part in ["echo-cancellation", "noise-suppression", "automatic-gain"] {
        window.click(part);
    }
    window.until("the processing is off", |window| {
        let voice = window
            .state
            .read_with(&window.cx, |state, _| state.voice().clone());
        !voice.echo_cancellation && !voice.noise_suppression && !voice.automatic_gain
    });
    window.press("escape");
}

/// The administrator enters a call. The server lists a person when its admission check
/// has seen the device in the room, and the person acts as one inside from then on
/// (ADR 0517): the function returns then.
fn enter(world: &World, caller: &Caller, target: CallTarget) {
    world.run(caller.calls.join(target)).unwrap();
    let listed = || {
        caller.handle.live().view().model.is_some_and(|model| {
            model
                .call_at(&target.place)
                .is_some_and(|call| call.participant(model.account.id).is_some())
        })
    };
    let deadline = std::time::Instant::now() + PATIENCE;
    while !listed() {
        assert!(
            std::time::Instant::now() < deadline,
            "after {PATIENCE:?} the server does not list the administrator in the call"
        );
        world.run(async { tokio::time::sleep(Duration::from_millis(20)).await });
    }
}

/// A second voice channel of the server, and its place as the target of a join.
fn second_room(world: &World, admin: &Admin, window: &mut Window, name: &str) -> CallTarget {
    voice_channel(world, admin, window, name);
    let workspace_id = window.view().workspace().unwrap().id;
    let row = window
        .channels()
        .into_iter()
        .find(|row| row.kind == ChannelKind::Voice && row.name == name)
        .unwrap();
    CallTarget {
        server_id: admin.handle.id(),
        place: CallPlace::Channel {
            workspace_id,
            channel_id: row.id,
        },
    }
}

fn channel_of(target: &CallTarget) -> ChannelId {
    let CallPlace::Channel { channel_id, .. } = target.place else {
        unreachable!("a voice channel");
    };
    channel_id
}

/// The administrator moves the person of the window from one voice channel to another,
/// and the device of the person enters the call of the destination. The window is not
/// drawn meanwhile: it shows the first call, and its controls are the ones that were
/// drawn for the first call.
fn moved_without_a_drawing(
    world: &World,
    admin: &Admin,
    window: &Window,
    from: &CallTarget,
    to: &CallTarget,
) {
    let (calls, member) = window.state.read_with(&window.cx, |state, _| {
        (
            state.calls().clone(),
            state.view().person.account_id.unwrap(),
        )
    });
    world
        .run(
            admin
                .handle
                .api()
                .move_participant(channel_of(from), member, channel_of(to)),
        )
        .unwrap();
    let deadline = std::time::Instant::now() + PATIENCE;
    while !calls
        .view()
        .call
        .is_some_and(|call| call.target.place == to.place && call.phase == Phase::Connected)
    {
        assert!(
            std::time::Instant::now() < deadline,
            "after {PATIENCE:?} the device is not in the call of the destination: {:?}",
            calls.view()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Reads the calls of the window for a time in which a camera or a share would have
/// started, and requires that `holds` the whole time.
#[track_caller]
fn stays(window: &mut Window, what: &str, holds: impl Fn(&junto_media::calls::CallsView) -> bool) {
    let calls = window
        .state
        .read_with(&window.cx, |state, _| state.calls().clone());
    let end = std::time::Instant::now() + Duration::from_secs(2);
    while std::time::Instant::now() < end {
        window.cx.run_until_parked();
        let view = calls.view();
        assert!(
            holds(&view),
            "this did not hold: {what}; the calls: {view:?}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[gpui::test]
#[ignore = "needs a LiveKit server: run through tools/with-test-livekit.sh"]
fn a_person_joins_a_call_in_the_window_speaks_hears_mutes_and_leaves(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server_with_media("Acme Studio");
    let admin = world.owner(&server);
    let caller = admin.caller(&world);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    voice_channel(&world, &admin, &mut window, ROOM);
    turn_processing_off(&mut window);
    let (server_id, workspace_id) = {
        let view = window.view();
        (
            view.selected.as_ref().unwrap().id,
            view.workspace().unwrap().id,
        )
    };
    let voice = room(&mut window);
    let target = CallTarget {
        server_id: admin.handle.id(),
        place: CallPlace::Channel {
            workspace_id,
            channel_id: voice.id,
        },
    };
    assert_eq!(server_id, target.server_id);

    // The administrator is in the call. The window lists the person with the mark of the
    // server, and offers the call.
    enter(&world, &caller, target);
    window.until("the person is listed", |window| {
        occupants(window)
            .first()
            .is_some_and(|occupant| occupant.name.as_deref() == Some("Ada"))
    });
    window.click_channel(ROOM);
    assert!(window.shows("join-call") && !window.shows("call-strip"));
    assert_eq!(occupants(&mut window)[0].level, None);

    // The person joins: the stage, the strip of the call and the panel appear.
    window.click("join-call");
    window.until("the call is connected", |window| {
        window
            .view()
            .call
            .is_some_and(|call| call.phase == Phase::Connected)
    });
    assert!(window.shows("call-stage") && window.shows("call-strip"));
    assert!(window.shows("call-panel") && window.shows("leave-call"));
    assert!(!window.shows("join-call") && !window.shows("members-button"));
    let view = window.view();
    let strip = view.call.clone().unwrap();
    assert_eq!(strip.name.as_deref(), Some(ROOM));
    assert!(strip.shown);
    assert_eq!(
        view.workspace()
            .unwrap()
            .channel
            .as_ref()
            .unwrap()
            .voice
            .as_ref()
            .unwrap()
            .relation,
        Relation::Inside
    );

    // Each side hears the tone of the other, and the window shows the measured levels.
    window.until("the window hears the administrator", |window| {
        heard(&window.sound, ADMIN_TONE_HZ) > HEARD_DBFS
    });
    window.until("the administrator hears the window", |_| {
        heard(&caller.sound, WINDOW_TONE_HZ) > HEARD_DBFS
    });
    window.until("the levels are shown", |window| {
        let people = occupants(window);
        people.len() == 2
            && people.iter().all(|occupant| {
                occupant.presence == Presence::Here
                    && occupant.level.is_some_and(|level| level >= 3)
                    && occupant.microphone == Microphone::Open { speaking: true }
            })
    });
    let ada = occupants(&mut window)
        .into_iter()
        .find(|occupant| !occupant.is_self)
        .unwrap();
    window.until("the call time runs", |window| {
        // The clock of the window is the clock of the test platform.
        window.cx.executor().advance_clock(Duration::from_secs(1));
        window
            .view()
            .call
            .is_some_and(|call| call.elapsed.is_some_and(|elapsed| elapsed.as_secs() >= 1))
    });
    assert!(window.shows("call-time"));

    // The microphone button closes the microphone: the other side hears nothing, and the
    // lists show the closed microphone.
    window.click("microphone");
    window.until("the administrator hears nothing", |_| {
        heard(&caller.sound, WINDOW_TONE_HZ) < ABSENT_DBFS
    });
    window.until("the closed microphone is shown", |window| {
        window.view().controls.microphone == MicrophoneButton::Muted
            && own(window).is_some_and(|own| own.microphone == Microphone::Closed)
    });
    until_hears(
        &world,
        &mut window,
        &caller,
        Hears::Window,
        "after the microphone of the window was closed",
    );
    window.click("microphone");
    window.until("the administrator hears the window again", |_| {
        heard(&caller.sound, WINDOW_TONE_HZ) > HEARD_DBFS
    });

    // The sound button turns the sound of the call off for this person only.
    window.click("sound-output");
    window.until("the window plays nothing", |window| {
        heard(&window.sound, ADMIN_TONE_HZ) < ABSENT_DBFS
    });
    until_hears(
        &world,
        &mut window,
        &caller,
        Hears::Administrator,
        "after the sound of the window was turned off",
    );
    window.click("sound-output");
    window.until("the window plays the call again", |window| {
        heard(&window.sound, ADMIN_TONE_HZ) > HEARD_DBFS
    });

    // The members of the call, and the volume of one person: the arrow keys change it.
    window.click("call-tab-members");
    assert_eq!(
        window.in_shell(|shell, _, _| shell.call_tab()),
        CallTab::Members
    );
    let row: &'static str = Box::leak(format!("call-member:{}", ada.account_id).into_boxed_str());
    let slider: &'static str = Box::leak(format!("volume:{}", ada.account_id).into_boxed_str());
    assert!(window.shows(row) && window.shows(slider));
    let full = heard(&window.sound, ADMIN_TONE_HZ);
    // A click on the track sets the volume of that place, and takes the keyboard focus.
    window.click(slider);
    window.press(&["left"; 20].join(" "));
    window.until("the volume is none", |window| {
        occupants(window)
            .iter()
            .any(|occupant| !occupant.is_self && occupant.volume == 0)
    });
    window.until("the administrator is not played", |window| {
        heard(&window.sound, ADMIN_TONE_HZ) < ABSENT_DBFS
    });
    window.press("right right right right right");
    window.until("the volume is half", |window| {
        occupants(window)
            .iter()
            .any(|occupant| !occupant.is_self && occupant.volume == 50)
    });
    window.until("the administrator is quieter", |window| {
        (full - heard(&window.sound, ADMIN_TONE_HZ) - 6.0).abs() < 2.0
    });

    // The sound settings of the call: the devices of the call and the input level.
    window.click("call-tab-settings");
    window.until("the settings show the microphone", |window| {
        window
            .view()
            .lists()
            .and_then(|lists| lists.call.clone())
            .is_some_and(|panel| {
                panel.audio.microphone == MicrophoneLine::Running
                    && panel.audio.input_level > 5
                    && panel.audio.inputs.len() == 2
            })
    });
    assert!(window.shows("input-device") && window.shows("input-level"));
    window.click("input-device");
    assert!(window.shows("device-menu") && window.shows("device-choice-1"));
    window.press("escape");
    assert!(!window.shows("device-menu"));

    // The call goes on while another channel is shown, and the strip leads back to it.
    window.click_channel("general");
    assert!(window.shows("call-strip") && !window.shows("call-stage"));
    until_hears(
        &world,
        &mut window,
        &caller,
        Hears::Administrator,
        "after another channel was shown",
    );
    window.click("show-call");
    window.until("the call is shown again", |window| {
        window.shows("call-stage")
    });

    // Leave: the strip and the stage go, and the other side hears nothing.
    window.click("leave-call");
    window.until("the call has ended", |window| {
        window.view().call.is_none() && !window.shows("call-strip")
    });
    assert!(window.shows("join-call"));
    assert_eq!(window.view().call_notice, None);
    window.until("the administrator hears nothing", |_| {
        heard(&caller.sound, WINDOW_TONE_HZ) < ABSENT_DBFS
    });
    window.until("the window is listed no more", |window| {
        own(window).is_none() && occupants(window).len() == 1
    });
    world.run(caller.calls.leave());
}

#[gpui::test]
#[ignore = "needs a LiveKit server: run through tools/with-test-livekit.sh"]
fn the_chat_of_a_call_is_read_when_its_tab_is_shown(cx: &mut TestAppContext) {
    use junto_client_core::model::PlaceId;
    use junto_client_core::protocol::message::{Block, Document, Inline, SendMessage};

    let world = World::new();
    let server = world.server_with_media("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    voice_channel(&world, &admin, &mut window, ROOM);
    turn_processing_off(&mut window);
    window.click_channel(ROOM);
    window.click("join-call");
    window.until("the call is connected", |window| {
        window
            .view()
            .call
            .is_some_and(|call| call.phase == Phase::Connected)
    });
    // The person looks at the window: what it shows at the end of a list is read.
    window.cx.update(|window, _| window.activate_window());
    window.until("the chat is drawn and read", |window| {
        window.shows("feed") && !room(window).unread.any()
    });

    // The panel shows the members. A message that arrives now is not shown.
    window.click("call-tab-members");
    assert!(!window.shows("feed"));
    let message = SendMessage {
        document: Document {
            blocks: vec![Block::Paragraph {
                content: vec![Inline::Text {
                    text: "during the call".to_owned(),
                    marks: Vec::new(),
                }],
            }],
        },
        reply_to: None,
        attachments: Vec::new(),
    };
    let place = PlaceId::Channel(room(&mut window).id);
    world
        .run(admin.handle.api().send_message(place, &message))
        .unwrap();
    window.until("the message is counted", |window| {
        room(window).unread.messages == 1
    });
    // A list that is shown sends its read position within a second. This one is not
    // shown, and the message stays unread.
    let later = std::time::Instant::now() + Duration::from_secs(3);
    while std::time::Instant::now() < later {
        assert_eq!(
            room(&mut window).unread.messages,
            1,
            "a message that was not shown was read"
        );
        std::thread::sleep(Duration::from_millis(50));
    }

    // The chat tab shows the message: it is read.
    window.click("call-tab-chat");
    window.until("the message is read", |window| {
        window.shows("feed") && !room(window).unread.any()
    });
}

#[gpui::test]
#[ignore = "needs a LiveKit server: run through tools/with-test-livekit.sh"]
fn push_to_talk_and_a_block_of_a_moderator_in_the_window(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server_with_media("Acme Studio");
    let admin = world.owner(&server);
    let caller = admin.caller(&world);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    voice_channel(&world, &admin, &mut window, ROOM);
    turn_processing_off(&mut window);
    let workspace_id = window.view().workspace().unwrap().id;
    let voice = room(&mut window);
    enter(
        &world,
        &caller,
        CallTarget {
            server_id: admin.handle.id(),
            place: CallPlace::Channel {
                workspace_id,
                channel_id: voice.id,
            },
        },
    );

    // Push-to-talk with the Control key, chosen in the settings.
    window.click("app-settings");
    window.click("input-mode-1");
    window.until("the key is offered", |window| window.shows("talk-key-1"));
    window.click("talk-key-1");
    window.press("escape");
    window.click_channel(ROOM);
    window.click("join-call");
    window.until("the call is connected", |window| {
        window
            .view()
            .call
            .is_some_and(|call| call.phase == Phase::Connected)
    });
    window.until("the window hears the administrator", |window| {
        heard(&window.sound, ADMIN_TONE_HZ) > HEARD_DBFS
    });
    assert!(heard(&caller.sound, WINDOW_TONE_HZ) < ABSENT_DBFS);

    let held = Modifiers {
        control: true,
        ..Modifiers::none()
    };
    window.cx.simulate_modifiers_change(held);
    window.until("the key sends the microphone", |_| {
        heard(&caller.sound, WINDOW_TONE_HZ) > HEARD_DBFS
    });
    // The other modifier key is not the key.
    window.cx.simulate_modifiers_change(Modifiers {
        alt: true,
        ..Modifiers::none()
    });
    window.until("the release ends it", |_| {
        heard(&caller.sound, WINDOW_TONE_HZ) < ABSENT_DBFS
    });
    window.cx.simulate_modifiers_change(held);
    window.until("the key sends again", |_| {
        heard(&caller.sound, WINDOW_TONE_HZ) > HEARD_DBFS
    });

    // A moderator closes the microphone: the button is held, a line says who did it, and
    // the key sends nothing.
    let own_id = own(&mut window).unwrap().account_id;
    world
        .run(admin.handle.api().block_microphone(voice.id, own_id))
        .unwrap();
    window.until("the block is shown", |window| {
        window.view().controls.microphone == MicrophoneButton::Held
    });
    assert_eq!(
        window.view().controls.warning,
        Some(SoundWarning::MicrophoneHeld {
            reason: BlockReason::Moderator,
            by: Some("Ada".to_owned()),
        })
    );
    assert!(window.shows("sound-warning"));
    assert_eq!(
        own(&mut window).unwrap().microphone,
        Microphone::Held(BlockReason::Moderator)
    );
    window.until("the administrator hears nothing", |_| {
        heard(&caller.sound, WINDOW_TONE_HZ) < ABSENT_DBFS
    });
    // A click on the held button changes nothing.
    window.click("microphone");
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(window.view().controls.microphone, MicrophoneButton::Held);

    world
        .run(admin.handle.api().unblock_microphone(voice.id, own_id))
        .unwrap();
    window.until("the block is gone", |window| {
        window.view().controls.microphone == MicrophoneButton::Open
            && !window.shows("sound-warning")
    });
    window.until("the key sends again", |_| {
        heard(&caller.sound, WINDOW_TONE_HZ) > HEARD_DBFS
    });
    window.click("leave-call");
    window.until("the call has ended", |window| window.view().call.is_none());
    world.run(caller.calls.leave());
}

/// A tone of a share that arrives is above this: the call library sends the sound of a
/// share with one channel, so each of its two tones arrives at half its level.
const SHARE_HEARD_DBFS: f32 = -26.0;
/// The left tones of all system sound and of the application "Player" of the capture of a
/// test.
const SYSTEM_TONE_HZ: f32 = 700.0;
const PLAYER_TONE_HZ: f32 = 1_100.0;

fn own_share(window: &mut Window) -> OwnShare {
    window.view().call.map_or(OwnShare::Off, |call| call.share)
}

fn stage(window: &mut Window) -> Option<ShareStage> {
    window
        .view()
        .lists()
        .and_then(|lists| lists.call.clone())
        .and_then(|panel| panel.stage)
}

#[gpui::test]
#[ignore = "needs a LiveKit server: run through tools/with-test-livekit.sh"]
fn a_person_shares_a_screen_in_the_window_watches_a_share_and_asks_for_a_recording(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server_with_media("Acme Studio");
    let admin = world.owner(&server);
    let caller = admin.caller(&world);
    let (directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    voice_channel(&world, &admin, &mut window, ROOM);
    let workspace_id = window.view().workspace().unwrap().id;
    let voice = room(&mut window);
    let target = CallTarget {
        server_id: admin.handle.id(),
        place: CallPlace::Channel {
            workspace_id,
            channel_id: voice.id,
        },
    };
    enter(&world, &caller, target);
    window.click_channel(ROOM);
    window.click("join-call");
    window.until("the call is connected", |window| {
        window
            .view()
            .call
            .is_some_and(|call| call.phase == Phase::Connected)
    });
    assert_eq!(own_share(&mut window), OwnShare::Off);
    assert!(window.view().call.unwrap().can_share);

    // The system does not allow the capture yet: the dialog tells how the person allows
    // it, and reads nothing. The button makes the system ask and opens its settings.
    window.capture.set_access(ScreenAccess::NotAllowed);
    window.click("call-share");
    assert!(window.shows("share-picker") && window.shows("screen-access"));
    assert!(!window.shows("share-sources") && !window.shows("start-share"));
    assert_eq!(window.capture.asked(), 0);
    window.click("open-system-settings");
    assert_eq!(
        (window.capture.asked(), window.capture.settings_opened()),
        (1, 1)
    );
    window.capture.set_access(ScreenAccess::Allowed);
    window.click("check-screen-access");
    window.until("the screens and windows are listed", |window| {
        window.shows("share-sources")
    });

    // Nothing is chosen for the person: without a picture and a sound, no share starts.
    window.click("start-share");
    assert!(window.in_shell(|shell, _, _| shell.share_picker_is_open()));
    window.click("share-display:1");
    window.click("start-share");
    assert!(window.in_shell(|shell, _, _| shell.share_picker_is_open()));
    assert!(window.capture.started().is_empty());
    // The screen with all system sound.
    window.click("share-sound-1");
    window.click("start-share");
    assert!(!window.in_shell(|shell, _, _| shell.share_picker_is_open()));
    window.until("the share of the window runs", |window| {
        own_share(window) == OwnShare::Running
    });
    assert_eq!(
        window.capture.started(),
        [CaptureRequest {
            picture: PictureSource::Display(1),
            sound: SoundChoice::System,
            quality: ShareQuality::default(),
        }]
    );
    window.until("the own share is shown", |window| {
        window.shows("share-header") && window.shows("share-picture")
    });
    let shown = stage(&mut window).unwrap();
    assert_eq!(shown.shares.len(), 1);
    assert_eq!(
        (
            shown.shares[0].id,
            shown.shares[0].sound,
            shown.shares[0].watched
        ),
        (ShareId::Own, Some(ShareAudio::System), true)
    );
    assert!(!window.shows("shares-bar"));
    // The administrator gets the picture and the sound of the share.
    window.until("the administrator watches the share", |_| {
        caller.calls.main_picture().borrow().is_some()
    });
    window.until("the administrator hears the share", |_| {
        heard(&caller.sound, SYSTEM_TONE_HZ) > SHARE_HEARD_DBFS
    });

    // The administrator shares a window with the sound of its application. The window
    // shows the bar of the two shares, watches the share of the administrator and plays
    // its sound.
    caller.calls.start_share(CaptureRequest {
        picture: PictureSource::Window(11),
        sound: SoundChoice::Application(SyntheticCapture::player()),
        quality: ShareQuality::default(),
    });
    window.until("two shares are shown", |window| {
        window.shows("shares-bar")
            && stage(window).is_some_and(|stage| {
                stage.shares.len() == 2
                    && stage.shares[0].watched
                    && stage.shares[0].sound == Some(ShareAudio::Application)
                    && stage.shares[0].kind == Some(ShareKind::Window)
            })
    });
    window.until(
        "the window plays the share of the administrator",
        |window| heard(&window.sound, PLAYER_TONE_HZ) > SHARE_HEARD_DBFS,
    );
    // The person picks the own share: the sound of the other share stops.
    window.click("share-card:own");
    window.until("the own share is watched", |window| {
        stage(window).is_some_and(|stage| stage.shares[1].watched && !stage.shares[0].watched)
    });
    window.until("the sound of the other share has stopped", |window| {
        heard(&window.sound, PLAYER_TONE_HZ) < ABSENT_DBFS
    });

    // The settings of a share and of the view are stored, and the strip hides the people.
    window.click("call-tab-settings");
    window.click("share-resolution-0");
    window.click("share-frame-rate-0");
    window.click("hide-without-video");
    // With no cameras, hiding keeps the avatar grid usable. Once a camera starts,
    // only people without a picture move into the hidden summary.
    assert!(!window.shows("people-hidden"));
    window.click("call-camera");
    window.until(
        "the people without video are hidden beside the camera",
        |window| window.shows("people-hidden"),
    );
    window.until("the settings are stored", |_| {
        std::fs::read_to_string(voice_file(directory.path())).is_ok_and(|stored| {
            stored.contains("\"share_resolution\": \"p720\"")
                && stored.contains("\"share_frame_rate\": 15")
                && stored.contains("\"hide_people_without_video\": true")
        })
    });

    // The button of the strip ends the share.
    window.click("call-share");
    window.until("the share of the window has ended", |window| {
        own_share(window) == OwnShare::Off && window.capture.running() == 0
    });
    window.until("one share is left", |window| {
        stage(window).is_some_and(|stage| stage.shares.len() == 1) && !window.shows("shares-bar")
    });
    // The next share uses the stored quality.
    window.click("call-share");
    window.until("the screens and windows are listed", |window| {
        window.shows("share-sources")
    });
    window.click("share-window:11");
    window.click("share-sound-0");
    window.click("start-share");
    window.until("the window is shared", |window| {
        own_share(window) == OwnShare::Running
    });
    let second = window.capture.started().pop().unwrap();
    assert_eq!(
        (second.picture, second.sound),
        (PictureSource::Window(11), SoundChoice::None)
    );
    assert_eq!(
        (second.quality.resolution, second.quality.frame_rate),
        (Resolution::P720, 15)
    );
    // The window that is shared is closed: the share ends, and the person is told.
    window.capture.end_all(&CaptureError::SourceGone);
    window.until("the end of the share is told", |window| {
        window.shows("share-notice") && own_share(window) == OwnShare::Off
    });
    window.click("dismiss-share-notice");
    assert!(!window.shows("share-notice"));

    // The recording: the roles of the person do not allow it, until an administrator
    // gives "record calls" to every member. This server has no recorder, and the refusal
    // of the request is told.
    assert!(window.shows("no-record-permission"));
    let roles = world
        .run(admin.handle.api().workspace_roles(workspace_id))
        .unwrap();
    let everyone = roles.iter().find(|role| role.is_everyone).unwrap();
    let mut permissions = everyone.permissions.clone();
    permissions.push(WorkspacePermission::RecordCalls);
    world
        .run(admin.handle.api().update_workspace_role(
            workspace_id,
            everyone.id,
            &UpdateWorkspaceRole {
                name: None,
                permissions: Some(permissions),
            },
        ))
        .unwrap();
    window.until("the person may record", |window| {
        !window.shows("no-record-permission")
    });
    // "Record" opens the languages of the speech; a choice sends the request (ADR 0616).
    window.click("start-recording");
    window.click("record-language:auto");
    window.until("the refusal is told", |window| {
        window.shows("recording-error")
    });
    assert!(!window.shows("recording-indicator"));
    window.click("dismiss-recording-error");
    assert!(!window.shows("recording-error") && window.shows("start-recording"));
}
