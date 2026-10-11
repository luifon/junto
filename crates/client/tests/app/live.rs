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
    ABSENT_DBFS, ADMIN_TONE_HZ, Caller, HEARD_DBFS, WINDOW_TONE_HZ, Window, World, heard, member,
};

mod rules;

/// The voice channel of the tests. A new server has a voice channel of its own beside it.
const ROOM: &str = "Daily";

fn room(window: &mut Window) -> ChannelRow {
    window
        .channels()
        .into_iter()
        .find(|row| row.kind == ChannelKind::Voice && row.name == ROOM)
        .unwrap()
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

fn enter(world: &World, caller: &Caller, target: CallTarget) {
    world.run(caller.calls.join(target)).unwrap();
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
    assert!(heard(&window.sound, ADMIN_TONE_HZ) > HEARD_DBFS);
    window.click("microphone");
    window.until("the administrator hears the window again", |_| {
        heard(&caller.sound, WINDOW_TONE_HZ) > HEARD_DBFS
    });

    // The sound button turns the sound of the call off for this person only.
    window.click("sound-output");
    window.until("the window plays nothing", |window| {
        heard(&window.sound, ADMIN_TONE_HZ) < ABSENT_DBFS
    });
    assert!(heard(&caller.sound, WINDOW_TONE_HZ) > HEARD_DBFS);
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
            .workspace()
            .and_then(|workspace| workspace.call.clone())
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
    assert!(heard(&caller.sound, WINDOW_TONE_HZ) > HEARD_DBFS);
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
        .workspace()
        .and_then(|workspace| workspace.call.clone())
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
    window.until("the people without video are hidden", |window| {
        window.shows("people-hidden")
    });
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
    window.click("start-recording");
    window.until("the refusal is told", |window| {
        window.shows("recording-error")
    });
    assert!(!window.shows("recording-indicator"));
    window.click("dismiss-recording-error");
    assert!(!window.shows("recording-error") && window.shows("start-recording"));
}
