//! The controls of calls that need no media server: the person's own microphone and sound
//! settings, the sound settings of the application, and a join that the server refuses.

use std::time::Duration;

use gpui::TestAppContext;
use junto_client::state::view::{CallNotice, MicrophoneButton, Relation};
use junto_client_core::flow::FlowError;
use junto_client_core::protocol::call::CallPlace;
use junto_client_core::protocol::error::ErrorCode;
use junto_client_core::protocol::ids::CallInvitationId;
use junto_client_core::protocol::structure::{ChannelKind, CreateChannel};
use junto_client_core::voice::{InputMode, TalkKey, VoiceSettings};

use junto_media::calls::{CallTarget, JoinError};

use crate::harness::{Admin, Window, World, member, open};

pub fn voice_channel(world: &World, admin: &Admin, window: &mut Window, name: &str) {
    let workspace_id = window.view().workspace().unwrap().id;
    world
        .run(admin.handle.api().create_channel(
            workspace_id,
            &CreateChannel {
                name: name.to_owned(),
                kind: ChannelKind::Voice,
                category_id: None,
                position: None,
            },
        ))
        .unwrap();
    let name = name.to_owned();
    window.until("the voice channel is listed", move |window| {
        window.shows_channel(&name)
    });
}

fn relation(window: &mut Window, name: &str) -> Relation {
    window
        .channels()
        .into_iter()
        .find(|row| row.name == name)
        .and_then(|row| row.voice)
        .map(|voice| voice.relation)
        .unwrap()
}

/// The window polls the futures of its calls on the executor of gpui, which has no Tokio
/// runtime (ADR 0733). The test devices of the application fail outside a runtime, as the
/// devices of the systems do. The server has no media server: each join takes its turn on
/// the runtime of the client, asks the server, and its future waits for the refusal.
#[gpui::test]
fn the_futures_of_the_calls_end_on_the_executor_of_the_window(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    voice_channel(&world, &admin, &mut window, "Daily");
    let workspace_id = window.view().workspace().unwrap().id;
    let channel_id = window
        .channels()
        .into_iter()
        .find(|row| row.name == "Daily")
        .unwrap()
        .id;
    let calls = window
        .state
        .read_with(&window.cx, |state, _| state.calls().clone());
    let target = CallTarget {
        server_id: admin.handle.id(),
        place: CallPlace::Channel {
            workspace_id,
            channel_id,
        },
    };
    let ended = std::rc::Rc::new(std::cell::Cell::new(false));
    let done = ended.clone();
    window
        .cx
        .foreground_executor()
        .spawn(async move {
            assert!(tokio::runtime::Handle::try_current().is_err());
            let sources = calls.share_sources().await.unwrap();
            assert!(!sources.displays.is_empty());
            let joined = calls.join(target).await;
            assert!(matches!(joined, Err(JoinError::Refused(_))), "{joined:?}");
            let joined = calls
                .join_invited(target, CallInvitationId::generate())
                .await;
            assert!(matches!(joined, Err(JoinError::Refused(_))), "{joined:?}");
            calls.leave().await;
            done.set(true);
        })
        .detach();
    window.until("every future of the calls has ended", |_| ended.get());
}

#[gpui::test]
fn a_join_that_the_server_refuses_is_told_and_leaves_the_person_outside(cx: &mut TestAppContext) {
    let world = World::new();
    // The server has no media server: it gives no place in a call.
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    voice_channel(&world, &admin, &mut window, "Daily");

    // A click on a voice channel shows it and joins nothing.
    window.click_channel("Daily");
    assert!(window.shows("join-call") && window.shows("feed") && window.shows("composer"));
    assert_eq!(relation(&mut window, "Daily"), Relation::Outside);
    assert!(!window.shows("call-strip") && !window.shows("call-stage"));

    window.click("join-call");
    window.until("the refusal is told", |window| window.shows("call-notice"));
    let view = window.view();
    assert!(
        matches!(
            &view.call_notice,
            Some(CallNotice::JoinFailed { channel, error: FlowError::Refused { code, .. } })
                if channel.as_deref() == Some("Daily") && *code == ErrorCode::MediaUnavailable
        ),
        "{:?}",
        view.call_notice
    );
    assert!(view.call.is_none());
    assert_eq!(relation(&mut window, "Daily"), Relation::Outside);
    assert!(window.shows("join-call") && !window.shows("call-strip"));
    // No microphone was opened for a call that did not begin.
    assert!(window.sound.opened_inputs().is_empty());
    assert!(window.sound.opened_outputs().is_empty());

    // The notice stays while another channel is shown, and goes with its button.
    window.click_channel("general");
    assert!(window.shows("call-notice"));
    window.click("dismiss-call-notice");
    window.until("the notice is gone", |window| !window.shows("call-notice"));
    assert_eq!(window.view().call_notice, None);
}

#[gpui::test]
fn the_microphone_and_the_sound_button_hold_the_setting_of_the_person_outside_a_call(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    let controls = window.view().controls;
    assert_eq!(controls.microphone, MicrophoneButton::Open);
    assert!(!controls.sound_off && controls.warning.is_none());

    window.click("microphone");
    window.until("the microphone is closed", |window| {
        window.view().controls.microphone == MicrophoneButton::Muted
    });
    window.click("sound-output");
    window.until("the sound is off", |window| {
        window.view().controls.sound_off
    });
    // The keyboard reaches both buttons.
    window.click("microphone");
    window.until("the microphone is open", |window| {
        window.view().controls.microphone == MicrophoneButton::Open
    });
    assert!(window.view().controls.sound_off);
    assert!(window.sound.opened_inputs().is_empty());
}

pub fn voice_file(directory: &std::path::Path) -> std::path::PathBuf {
    directory.join("junto").join("voice.json")
}

#[gpui::test]
fn the_sound_settings_of_the_application_are_kept_over_a_restart(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    let voice = |window: &mut Window| -> VoiceSettings {
        window
            .state
            .read_with(&window.cx, |state, _| state.voice().clone())
    };
    assert_eq!(voice(&mut window), VoiceSettings::default());
    assert!(!voice_file(directory.path()).exists());

    window.click("app-settings");
    assert!(window.shows("settings-dialog"));
    // Voice activation has no key to choose.
    assert!(window.shows("input-mode-0") && !window.shows("talk-key-0"));
    window.click("input-mode-1");
    window.until("the key of push-to-talk is offered", |window| {
        window.shows("talk-key-1")
    });
    window.click("talk-key-1");
    window.click("noise-suppression");
    window.click("echo-cancellation");
    window.click("echo-cancellation");
    window.until("the settings are used", |window| {
        let voice = voice(window);
        voice.input_mode == InputMode::PushToTalk
            && voice.talk_key == TalkKey::Control
            && !voice.noise_suppression
            && voice.echo_cancellation
            && voice.automatic_gain
    });
    assert!(!window.shows("voice-settings-error"));
    window.press("escape");
    let chosen = voice(&mut window);
    window.close();

    // The file holds what was chosen, and a new start reads it.
    let text = std::fs::read_to_string(voice_file(directory.path())).unwrap();
    assert!(text.contains("\"input_mode\": \"push_to_talk\""), "{text}");
    assert!(text.contains("\"talk_key\": \"control\""), "{text}");
    let mut window = open(cx, directory.path());
    assert_eq!(voice(&mut window), chosen);
    window.click("app-settings");
    assert!(window.shows("talk-key-1"));
}

#[gpui::test]
fn drawing_camera_choices_does_not_enumerate_hardware(cx: &mut gpui::TestAppContext) {
    let directory = tempfile::tempdir().unwrap();
    let window = crate::harness::open(cx, &directory.path().join("client"));
    let before = window.camera.device_reads();
    for _ in 0..4 {
        window.state.read_with(&window.cx, |state, _| {
            let _ = state.camera_choices();
        });
    }
    assert_eq!(window.camera.device_reads(), before);
}

#[gpui::test]
fn each_unsuccessful_move_outcome_is_shown_and_dismissible(cx: &mut gpui::TestAppContext) {
    use junto_client_core::protocol::{
        call::MoveOutcome,
        events::Event,
        ids::{CallMoveId, ServerId},
    };
    let directory = tempfile::tempdir().unwrap();
    let mut window = crate::harness::open(cx, &directory.path().join("client"));
    for (outcome, expected) in [
        (MoveOutcome::Completed, None),
        (
            MoveOutcome::TimedOut,
            Some("The move timed out. The person remains in the original call."),
        ),
        (
            MoveOutcome::Refused,
            Some(
                "The device could not join the destination. The person remains in the original call.",
            ),
        ),
        (
            MoveOutcome::Abandoned,
            Some("The move was cancelled because the call or its permissions changed."),
        ),
    ] {
        let notice = junto_client_core::live::Notice::Event {
            id: 1,
            at: time::OffsetDateTime::now_utc(),
            event: std::sync::Arc::new(Event::CallMoveResolved {
                move_id: CallMoveId::generate(),
                outcome,
            }),
        };
        window.state.update(&mut window.cx, |state, cx| {
            state.notice_fixture(ServerId::generate(), &notice, cx)
        });
        assert_eq!(
            window
                .state
                .read_with(&window.cx, |state, _| state.pending().call_told.clone())
                .as_deref(),
            expected
        );
        assert!(
            window
                .state
                .read_with(&window.cx, |state, _| state.calls().view().call.is_none())
        );
        window
            .state
            .update(&mut window.cx, |state, cx| state.dismiss_call_told(cx));
        assert!(
            window
                .state
                .read_with(&window.cx, |state, _| state.pending().call_told.is_none())
        );
    }
}

/// A connected call of this device in the voice channel with this name, as the media
/// engine states it, without a media server.
fn connected_call(window: &mut Window, channel: &str) -> junto_media::calls::ActiveCall {
    use junto_media::calls::{ActiveCall, Phase};
    use junto_media::engine::{MicrophoneState, OutputState, ShareState};

    let view = window.view();
    let row = (window.channels().into_iter())
        .find(|row| row.name == channel)
        .unwrap();
    ActiveCall {
        camera: junto_media::camera::CameraState::Off,
        cameras: Vec::new(),
        target: CallTarget {
            server_id: view.selected.as_ref().unwrap().id,
            place: CallPlace::Channel {
                workspace_id: view.workspace().unwrap().id,
                channel_id: row.id,
            },
        },
        identity: "seat".to_owned(),
        phase: Phase::Connected,
        interrupted_at: None,
        microphone: MicrophoneState::Running,
        access: junto_media::permission::MicrophoneAccess::Allowed,
        input_device: None,
        output: OutputState::Running,
        output_device: None,
        microphone_block: None,
        sound_block: None,
        share: ShareState::Off,
        shares: Vec::new(),
        watched: None,
    }
}

/// The window draws this call of this device, without a media server.
pub(crate) fn show_call(window: &mut Window, call: junto_media::calls::ActiveCall) {
    use junto_client::state::view::MediaSnapshot;
    use junto_media::calls::CallsView;

    let media = MediaSnapshot {
        calls: CallsView {
            call: Some(call),
            ..CallsView::default()
        },
        ..MediaSnapshot::default()
    };
    let state = window.state.clone();
    window.cx.update(|_, cx| {
        state.update(cx, |state, cx| state.show_media_fixture(media, cx));
    });
    window.settle();
}

/// ADR 0793: received pictures cause at most one drawing per frame of the display, whatever
/// their number and their source: the watched share, the small pictures of the shares and
/// the cameras. The follower converts the newest pictures on a thread of the runtime before
/// it asks for a frame. After a burst of pictures a frame draws the pictures whose conversion
/// started first, which can already be the newest ones, and the next frame the newest ones:
/// one or two drawings. The test counts every drawing of the window. A change of the state of
/// the application, such as a late event of the server, can also draw this window, once for
/// each change at most: in each step the drawings that no picture asked for may not be more
/// than the changes of the state counted in that step.
#[gpui::test]
fn received_pictures_cause_one_drawing_per_frame_of_the_display(cx: &mut TestAppContext) {
    use junto_client_core::protocol::ids::AccountId;
    use junto_media::engine::{MainPicture, ShareId};
    use junto_media::share::picture::VideoPicture;
    use junto_media::share::synthetic::two_colours;
    use std::cell::Cell;
    use std::collections::HashMap;
    use std::rc::Rc;
    use std::sync::Arc;

    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    voice_channel(&world, &admin, &mut window, "Daily");
    window.click_channel("Daily");
    let call = connected_call(&mut window, "Daily");
    show_call(&mut window, call);
    let calls = window
        .state
        .read_with(&window.cx, |state, _| state.calls().clone());
    // A picture `width` pixels wide.
    let frame = |width: u32| {
        let pixels = two_colours(width, 2, (255, 255, 255), (255, 255, 255));
        Arc::new(VideoPicture::new(&pixels, false, (width, 2)).unwrap())
    };
    let (share, camera) = (
        ShareId::Of(AccountId::generate()),
        ShareId::Of(AccountId::generate()),
    );
    // The three sources, and all of them at once.
    let main = |width: u32| {
        calls.show_main_picture_for_test(Some(MainPicture {
            share: ShareId::Own,
            frame: frame(width),
        }));
    };
    let small = |width: u32| {
        calls.show_small_pictures_for_test(HashMap::from([(share, frame(width))]));
    };
    let cameras = |width: u32| {
        calls.show_camera_pictures_for_test(HashMap::from([(camera, frame(width))]));
    };
    let every = |width: u32| {
        main(width);
        small(width);
        cameras(width);
    };
    // The width of the picture that the window holds of each source.
    let held = |window: &Window| {
        window.shell.read_with(&window.cx, |shell, _| {
            (
                shell.draws().2,
                shell.small_widths().get(&share).copied(),
                shell.camera_widths().get(&camera).copied(),
            )
        })
    };
    // (every drawing of the window, the drawings that pictures asked for)
    let draws = |window: &Window| {
        let (all, asked, _) = window.shell.read_with(&window.cx, |shell, _| shell.draws());
        (all, asked)
    };
    let changes = Rc::new(Cell::new(0usize));
    let counted = changes.clone();
    let state = window.state.clone();
    let _changes = window
        .cx
        .update(|_, cx| cx.observe(&state, move |_, _| counted.set(counted.get() + 1)));
    // The drawings of a step that no picture asked for, from the counts before and after it,
    // are at most the changes of the state in that step.
    let unasked_within_changes = |step: &str,
                                  (all, asked): (usize, usize),
                                  (all_now, asked_now): (usize, usize),
                                  changed| {
        let unasked = (all_now - all) - (asked_now - asked);
        let state_changes = changes.get() - changed;
        assert!(
            unasked <= state_changes,
            "{step}: {unasked} drawings that no picture asked for, with {state_changes} \
             changes of the state of the application"
        );
    };
    // The window can hold work for a frame from what it loaded before: it is given frames
    // until it holds none.
    window.until("the window has no work for a frame", |window| {
        window.frame() == 0
    });

    let sources: [(&str, &dyn Fn(u32)); 4] = [
        ("the watched share", &main),
        ("the small pictures", &small),
        ("the cameras", &cameras),
        ("all three", &every),
    ];
    let mut width = 0;
    for (source, show) in sources {
        // Fifty pictures arrive, and the application does its work after each one. No frame
        // of the display passes: nothing is drawn.
        let (all, asked) = draws(&window);
        let changed = changes.get();
        for _ in 0..50 {
            width += 1;
            show(width);
            window.park();
        }
        assert_eq!(
            draws(&window).1,
            asked,
            "{source}: the pictures asked for a drawing without a frame"
        );
        unasked_within_changes(source, (all, asked), draws(&window), changed);
        // The pictures whose conversion started first, and then the newest ones.
        window.until_pictures_taken();
        let (all_after, asked_after) = draws(&window);
        assert!(
            (1..=2).contains(&(asked_after - asked)),
            "{source}: {} drawings for a burst of fifty pictures",
            asked_after - asked
        );
        unasked_within_changes(source, (all, asked), (all_after, asked_after), changed);
        let newest = Some(width);
        match source {
            "the watched share" => assert_eq!(held(&window).0, newest),
            "the small pictures" => assert_eq!(held(&window).1, newest),
            "the cameras" => assert_eq!(held(&window).2, newest),
            _ => assert_eq!(held(&window), (newest, newest, newest)),
        }
        // Without a newer picture a frame draws nothing.
        let changed = changes.get();
        window.frame();
        window.park();
        assert_eq!(
            draws(&window).1,
            asked_after,
            "{source}: a frame without a picture"
        );
        unasked_within_changes(source, (all_after, asked_after), draws(&window), changed);
        // Five pictures between two waits for frames: at most two drawings each time, all
        // of them asked for by the pictures, and the last picture is held.
        for round in 0..2 {
            let (all, asked) = draws(&window);
            let changed = changes.get();
            for _ in 0..5 {
                width += 1;
                show(width);
                window.park();
            }
            window.until_pictures_taken();
            let (all_now, asked_now) = draws(&window);
            assert!(
                (1..=2).contains(&(asked_now - asked)),
                "{source}, round {round}: {} drawings for five pictures",
                asked_now - asked
            );
            unasked_within_changes(
                &format!("{source}, round {round}"),
                (all, asked),
                (all_now, asked_now),
                changed,
            );
        }
    }
}

/// ADR 0774: a picture of a call that a newer one replaced is removed from the GPU images of
/// every window and is freed. Many pictures of the watched share, of the small pictures of
/// the shares and of the cameras are drawn in the window and then in the separate call
/// window; each window then holds GPU images only for the pictures that it draws now,
/// whatever their number. The pictures that were replaced are freed, and on macOS so are
/// their pixel buffers of Core Video, which the atlas does not show.
#[gpui::test]
fn a_window_holds_gpu_images_only_for_the_pictures_that_it_draws_now(cx: &mut TestAppContext) {
    use std::collections::HashMap;
    use std::sync::Arc;

    use gpui::{AppContext as _, RenderImage, VisualTestContext};
    use gpui_base::Root;
    use junto_client::shell::Shell;
    use junto_client_core::protocol::call::ShareAudio;
    use junto_client_core::protocol::ids::AccountId;
    use junto_media::camera::CameraState;
    use junto_media::engine::{MainPicture, SeenShare, ShareId, ShareInfo, ShareState};
    use junto_media::share::ShareKind;
    use junto_media::share::picture::VideoPicture;
    use junto_media::share::synthetic::two_colours;

    const PICTURES: u8 = 40;
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    voice_channel(&world, &admin, &mut window, "Daily");
    window.click_channel("Daily");
    let other: AccountId = "0198f1c2-7d3a-7b1e-9c55-0a1b2c3d4e5f".parse().unwrap();
    // The person shares and watches the share of another person; both cameras run.
    let mut call = connected_call(&mut window, "Daily");
    call.camera = CameraState::Running;
    call.cameras = vec![other];
    call.share = ShareState::Running(ShareInfo {
        kind: ShareKind::Window,
        sound: ShareAudio::None,
        sound_held: false,
        size: (64, 36),
        frame_rate: 30,
    });
    call.shares = vec![SeenShare {
        account_id: other,
        kind: Some(ShareKind::Screen),
        frame_rate: Some(30),
        size: Some((64, 36)),
    }];
    call.watched = Some(ShareId::Of(other));
    show_call(&mut window, call);
    let calls = window
        .state
        .read_with(&window.cx, |state, _| state.calls().clone());

    // Each source gives a new picture, the window gets a frame of the display, and the test
    // keeps every image that a window held for a picture, a weak reference of each picture,
    // and on macOS their pixel buffers.
    let mut seen: Vec<Arc<RenderImage>> = Vec::new();
    let given: std::cell::RefCell<Vec<Vec<std::sync::Weak<VideoPicture>>>> = Default::default();
    #[cfg(target_os = "macos")]
    let buffers: std::cell::RefCell<Vec<Vec<core_video::pixel_buffer::CVPixelBuffer>>> =
        Default::default();
    let mut give = |window: &mut Window, shells: &[&gpui::Entity<Shell>], shade: u8| {
        let camera = |own: bool| {
            let pixels = two_colours(64, 36, (shade, 0, 0), (0, 0, shade));
            Arc::new(VideoPicture::new(&pixels, own, (64, 36)).unwrap())
        };
        let small = || {
            let pixels = two_colours(64, 36, (shade, shade, 0), (0, shade, shade));
            Arc::new(VideoPicture::new(&pixels, false, (64, 36)).unwrap())
        };
        let watched = two_colours(64, 36, (0, shade, 0), (0, 0, shade));
        let pictures = [
            Arc::new(VideoPicture::new(&watched, false, (64, 36)).unwrap()),
            camera(true),
            camera(false),
            small(),
            small(),
        ];
        given
            .borrow_mut()
            .push(pictures.iter().map(Arc::downgrade).collect());
        calls.show_main_picture_for_test(Some(MainPicture {
            share: ShareId::Of(other),
            frame: pictures[0].clone(),
        }));
        calls.show_small_pictures_for_test(HashMap::from([
            (ShareId::Own, pictures[3].clone()),
            (ShareId::Of(other), pictures[4].clone()),
        ]));
        calls.show_camera_pictures_for_test(HashMap::from([
            (ShareId::Own, pictures[1].clone()),
            (ShareId::Of(other), pictures[2].clone()),
        ]));
        window.until_pictures_taken();
        // The window made each pixel buffer when it took the picture; the test holds one
        // reference of each.
        #[cfg(target_os = "macos")]
        buffers.borrow_mut().push(
            pictures
                .iter()
                .filter_map(|picture| picture.surface())
                .collect(),
        );
        drop(pictures);
        for shell in shells {
            for image in shell.read_with(&window.cx, |shell, _| shell.picture_images()) {
                if !seen.iter().any(|known| Arc::ptr_eq(known, &image)) {
                    seen.push(image);
                }
            }
        }
        seen.clone()
    };
    // How many of these images have a GPU image in the window.
    let in_gpu = |cx: &mut VisualTestContext, images: &[Arc<RenderImage>]| {
        cx.update(|window, _| {
            (images.iter())
                .filter(|image| window.has_image_atlas_entry(image))
                .count()
        })
    };
    let held = |window: &Window, shell: &gpui::Entity<Shell>| {
        shell.read_with(&window.cx, |shell, _| shell.picture_images())
    };
    // The pictures that are not freed, and on macOS their pixel buffers that something other
    // than the test holds, among the pictures
    // given before the last `newest` ones. A frame of the display draws first, so that the
    // scene of the window holds only what the window draws now.
    let not_freed = |window: &mut Window, newest: usize| {
        window.settle();
        let given = given.borrow();
        let earlier = &given[..given.len().saturating_sub(newest)];
        let pictures = (earlier.iter().flatten())
            .filter(|picture| picture.strong_count() > 0)
            .count();
        #[cfg(target_os = "macos")]
        let buffers = {
            use core_foundation::base::TCFType;
            let buffers = buffers.borrow();
            let earlier = &buffers[..buffers.len().saturating_sub(newest)];
            (earlier.iter().flatten())
                .filter(|buffer| buffer.retain_count() > 1)
                .count()
        };
        #[cfg(not(target_os = "macos"))]
        let buffers = 0;
        (pictures, buffers)
    };
    // The images of earlier pictures that still have a GPU image in the window: every
    // image with an atlas entry must be one that the window holds now (ADR 0774).
    let kept =
        |cx: &mut VisualTestContext, images: &[Arc<RenderImage>], now: &[Arc<RenderImage>]| {
            cx.update(|window, _| {
                (images.iter())
                    .filter(|image| window.has_image_atlas_entry(image))
                    .filter(|image| !now.iter().any(|held| Arc::ptr_eq(held, image)))
                    .count()
            })
        };

    let main_shell = window.shell.clone();
    let mut images = Vec::new();
    for shade in 1..=PICTURES {
        images = give(&mut window, &[&main_shell], shade);
    }
    let shown = held(&window, &main_shell);
    // On other systems than macOS every picture is an image. A camera whose tile is not in
    // view is held and not drawn; the two small pictures of the shares are drawn.
    #[cfg(not(target_os = "macos"))]
    {
        assert!(
            images.len() >= usize::from(PICTURES),
            "{} images for {PICTURES} pictures of each source",
            images.len()
        );
        assert!(
            in_gpu(&mut window.cx, &shown) >= 2,
            "the window did not draw the small pictures of the shares"
        );
    }
    assert_eq!(
        kept(&mut window.cx, &images, &shown),
        0,
        "the window holds GPU images of {} pictures that it no longer draws",
        images.len()
    );
    assert_eq!(
        not_freed(&mut window, 1),
        (0, 0),
        "(pictures, pixel buffers) that were replaced and not freed"
    );

    // The call moves to its own window, which draws the pictures from then on.
    let main = window.cx.update(|window, _| window.window_handle());
    window.click("detach-call");
    window.settle();
    let handle = (cx.windows().into_iter())
        .find(|handle| *handle != main)
        .unwrap();
    let detached_shell = cx
        .update_window(handle, |root, _, cx| {
            let root = root.downcast::<Root>().unwrap();
            root.read(cx).view().clone().downcast::<Shell>().unwrap()
        })
        .unwrap();
    let mut detached = window.window_of(handle);
    for shade in 1..=PICTURES {
        images = give(&mut window, &[&main_shell, &detached_shell], shade);
    }
    let shown = held(&window, &detached_shell);
    #[cfg(not(target_os = "macos"))]
    assert!(
        in_gpu(&mut detached.cx, &shown) >= 2,
        "the call window did not draw the small pictures of the shares"
    );
    assert_eq!(
        kept(&mut detached.cx, &images, &shown),
        0,
        "the call window holds GPU images of {} pictures that it no longer draws",
        images.len()
    );
    let main_shown = held(&window, &main_shell);
    assert_eq!(
        kept(&mut window.cx, &images, &main_shown),
        0,
        "the main window holds GPU images of {} pictures that it no longer draws",
        images.len()
    );
    assert_eq!(
        not_freed(&mut window, 1),
        (0, 0),
        "(pictures, pixel buffers) not freed after the call moved to its own window"
    );

    // The shares and the cameras end: no window holds a GPU image of their pictures, and
    // every picture and pixel buffer is freed.
    calls.show_main_picture_for_test(None);
    calls.show_small_pictures_for_test(HashMap::new());
    calls.show_camera_pictures_for_test(HashMap::new());
    window.until_pictures_taken();
    assert_eq!(
        (
            in_gpu(&mut window.cx, &images),
            in_gpu(&mut detached.cx, &images)
        ),
        (0, 0),
        "GPU images of the main window and of the call window after the end of the shares"
    );
    assert_eq!(
        not_freed(&mut window, 0),
        (0, 0),
        "(pictures, pixel buffers) not freed after the end of the shares and the cameras"
    );
}

/// ADR 0774: the watched share is drawn as a camera picture is. On macOS the window draws it
/// from a pixel buffer of Core Video and holds no image of gpui for it, so the renderer
/// copies nothing into its atlas; on the other systems it holds one image, which the
/// renderer draws from its atlas.
#[gpui::test]
fn the_watched_share_is_drawn_in_the_form_of_a_camera_picture(cx: &mut TestAppContext) {
    use std::sync::Arc;

    use junto_client_core::protocol::call::ShareAudio;
    use junto_media::engine::{MainPicture, ShareId, ShareInfo, ShareState};
    use junto_media::share::ShareKind;
    use junto_media::share::picture::VideoPicture;
    use junto_media::share::synthetic::two_colours;

    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    voice_channel(&world, &admin, &mut window, "Daily");
    window.click_channel("Daily");
    // The person shares and watches the own share.
    let mut call = connected_call(&mut window, "Daily");
    call.share = ShareState::Running(ShareInfo {
        kind: ShareKind::Window,
        sound: ShareAudio::None,
        sound_held: false,
        size: (64, 36),
        frame_rate: 30,
    });
    call.watched = Some(ShareId::Own);
    show_call(&mut window, call);
    let calls = window
        .state
        .read_with(&window.cx, |state, _| state.calls().clone());
    let pixels = two_colours(64, 36, (200, 30, 30), (30, 30, 200));
    calls.show_main_picture_for_test(Some(MainPicture {
        share: ShareId::Own,
        frame: Arc::new(VideoPicture::new(&pixels, false, (64, 36)).unwrap()),
    }));
    window.until_pictures_taken();
    let (drawn, images) = window.shell.read_with(&window.cx, |shell, _| {
        (shell.draws().2, shell.picture_images())
    });
    assert_eq!(
        drawn,
        Some(64),
        "the window does not hold the watched share"
    );
    #[cfg(target_os = "macos")]
    {
        assert!(
            images.is_empty(),
            "the window holds {} images of gpui for the watched share",
            images.len()
        );
        // The window draws again, and paints the pixel buffer inside the picture of the
        // share.
        let record = junto_client::ui::record_painted_surfaces();
        window.cx.update(|window, _| window.refresh());
        window.settle();
        let painted = junto_client::ui::take_painted_surfaces();
        drop(record);
        let viewport = window.cx.debug_bounds("share-picture").unwrap();
        let inside = |bounds: gpui::Bounds<gpui::Pixels>| {
            let near = |one: gpui::Pixels, other: gpui::Pixels| f32::from(one - other) >= -1.0;
            near(bounds.left(), viewport.left())
                && near(bounds.top(), viewport.top())
                && near(viewport.right(), bounds.right())
                && near(viewport.bottom(), bounds.bottom())
                && bounds.size.width > gpui::px(0.0)
        };
        assert!(
            painted
                .iter()
                .any(|surface| surface.size == (64, 36) && inside(surface.bounds)),
            "no pixel buffer of the watched share was painted inside {viewport:?}: {painted:?}"
        );
    }
    #[cfg(not(target_os = "macos"))]
    {
        assert_eq!(images.len(), 1);
        let in_gpu = window
            .cx
            .update(|window, _| window.has_image_atlas_entry(&images[0]));
        assert!(
            in_gpu,
            "the window did not draw the image of the watched share"
        );
    }
}

/// The small pictures of the shares are drawn as camera pictures are (ADR 0774): on macOS
/// from a pixel buffer of Core Video without an image of gpui, and on the other systems as an
/// image that holds the converted pixels.
#[gpui::test]
fn the_small_pictures_of_the_shares_are_drawn_in_the_form_of_a_camera_picture(
    cx: &mut TestAppContext,
) {
    use std::collections::HashMap;
    use std::sync::Arc;

    use junto_client_core::protocol::call::ShareAudio;
    use junto_client_core::protocol::ids::AccountId;
    use junto_media::engine::{SeenShare, ShareId, ShareInfo, ShareState};
    use junto_media::share::ShareKind;
    use junto_media::share::picture::VideoPicture;
    use junto_media::share::synthetic::two_colours;

    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    voice_channel(&world, &admin, &mut window, "Daily");
    window.click_channel("Daily");
    let other: AccountId = "0198f1c2-7d3a-7b1e-9c55-0a1b2c3d4e5f".parse().unwrap();
    // Two shares: the bar of the shares shows a small picture of each.
    let mut call = connected_call(&mut window, "Daily");
    call.share = ShareState::Running(ShareInfo {
        kind: ShareKind::Window,
        sound: ShareAudio::None,
        sound_held: false,
        size: (64, 36),
        frame_rate: 30,
    });
    call.shares = vec![SeenShare {
        account_id: other,
        kind: Some(ShareKind::Screen),
        frame_rate: Some(30),
        size: Some((64, 36)),
    }];
    call.watched = Some(ShareId::Of(other));
    show_call(&mut window, call);
    let calls = window
        .state
        .read_with(&window.cx, |state, _| state.calls().clone());
    let small = || {
        let pixels = two_colours(64, 36, (200, 30, 30), (30, 30, 200));
        Arc::new(VideoPicture::new(&pixels, false, (64, 36)).unwrap())
    };
    calls.show_small_pictures_for_test(HashMap::from([
        (ShareId::Own, small()),
        (ShareId::Of(other), small()),
    ]));
    window.until_pictures_taken();
    let images = window
        .shell
        .read_with(&window.cx, |shell, _| shell.picture_images());
    #[cfg(target_os = "macos")]
    {
        assert!(
            images.is_empty(),
            "the window holds {} images of gpui for the small pictures",
            images.len()
        );
        let record = junto_client::ui::record_painted_surfaces();
        window.cx.update(|window, _| window.refresh());
        window.settle();
        let painted = junto_client::ui::take_painted_surfaces();
        drop(record);
        // The selectors of gpui live as long as the program.
        let other_card: &'static str = format!("share-card:{other}").leak();
        for card in ["share-card:own", other_card] {
            let bounds = window.cx.debug_bounds(card).unwrap();
            assert!(
                painted
                    .iter()
                    .any(|surface| surface.size == (64, 36)
                        && bounds.contains(&surface.bounds.center())),
                "no pixel buffer of a small picture was painted inside {card} {bounds:?}: {painted:?}"
            );
        }
    }
    #[cfg(not(target_os = "macos"))]
    assert_eq!(images.len(), 2);
}

/// Sound settings that cannot be read are not replaced by an open microphone without a
/// statement: the person is told, and the microphone setting is closed.
#[gpui::test]
fn sound_settings_that_cannot_be_read_are_told_and_close_the_microphone(cx: &mut TestAppContext) {
    // A first start makes the data directory. The file of the sound settings is damaged
    // before the second start.
    let directory = tempfile::tempdir().unwrap();
    open(cx, directory.path()).close();
    let file = voice_file(directory.path());
    std::fs::write(&file, "{ this is not the file of the sound settings").unwrap();
    let window = open(cx, directory.path());
    let (failure, muted, told) = window.state.read_with(&window.cx, |state, _| {
        (
            state.voice_error().map(str::to_owned),
            state.calls().view().muted,
            state.pending().call_told.clone(),
        )
    });
    assert!(
        failure.is_some(),
        "nothing tells that the settings were not read"
    );
    assert!(
        muted,
        "the microphone setting is open with settings that were not read"
    );
    // The person reads why the microphone is closed, outside the settings too.
    assert!(
        told.is_some_and(|told| told.contains("sound settings")),
        "no notice tells that the microphone was closed"
    );
}

/// A share dialog belongs to the call that the window showed when the person opened it.
/// When that call ends while the person chooses, the dialog closes, the window tells that
/// nothing is shared, and nothing is captured (ADR 0736).
#[gpui::test]
fn a_share_dialog_closes_and_tells_when_its_call_ends(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    voice_channel(&world, &admin, &mut window, "Daily");
    window.click_channel("Daily");
    let call = connected_call(&mut window, "Daily");
    let media = junto_client::state::view::MediaSnapshot {
        calls: junto_media::calls::CallsView {
            call: Some(call),
            ..junto_media::calls::CallsView::default()
        },
        can_share: true,
        ..junto_client::state::view::MediaSnapshot::default()
    };
    let state = window.state.clone();
    window.cx.update(|_, cx| {
        state.update(cx, |state, cx| state.show_media_fixture(media, cx));
    });
    window.settle();
    window.click("call-share");
    assert!(window.shows("share-picker"));
    assert!(window.in_shell(|shell, _, _| shell.share_picker_is_open()));

    // The call ends while the dialog is open.
    window.cx.update(|_, cx| {
        state.update(cx, |state, cx| {
            state.show_media_fixture(junto_client::state::view::MediaSnapshot::default(), cx)
        });
    });
    window.settle();
    assert!(!window.shows("share-picker"));
    assert!(!window.in_shell(|shell, _, _| shell.share_picker_is_open()));
    assert!(window.shows("call-told"));
    assert_eq!(
        window
            .state
            .read_with(&window.cx, |state, _| state.pending().call_told.clone())
            .as_deref(),
        Some(junto_client::text::SHARE_DIALOG_CLOSED)
    );
    assert!(window.capture.started().is_empty());
    window.click("dismiss-call-told");
    assert!(!window.shows("call-told"));
}

/// ADR 0793: without a graphics processor the window takes the pictures of the cameras at
/// most once in 200 ms, and the pictures of the shares as they arrive. With one it takes
/// each newer picture of a camera.
#[gpui::test]
fn without_a_graphics_processor_the_cameras_are_taken_at_most_once_in_200_ms(
    cx: &mut TestAppContext,
) {
    use junto_client_core::protocol::ids::AccountId;
    use junto_media::engine::{MainPicture, ShareId};
    use junto_media::share::picture::VideoPicture;
    use junto_media::share::synthetic::two_colours;
    use std::collections::HashMap;
    use std::sync::Arc;

    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    voice_channel(&world, &admin, &mut window, "Daily");
    window.click_channel("Daily");
    let call = connected_call(&mut window, "Daily");
    show_call(&mut window, call);
    let calls = window
        .state
        .read_with(&window.cx, |state, _| state.calls().clone());
    let frame = |width: u32| {
        let pixels = two_colours(width, 2, (255, 255, 255), (255, 255, 255));
        Arc::new(VideoPicture::new(&pixels, false, (width, 2)).unwrap())
    };
    let camera = ShareId::Of(AccountId::generate());
    let show_camera = |width: u32| {
        calls.show_camera_pictures_for_test(HashMap::from([(camera, frame(width))]));
    };
    let held = |window: &mut Window| {
        window.until_pictures_taken();
        let (_, _, main) = window.shell.read_with(&window.cx, |shell, _| shell.draws());
        let cameras = window
            .shell
            .read_with(&window.cx, |shell, _| shell.camera_widths());
        (main, cameras.get(&camera).copied())
    };

    // With a graphics processor each newer picture of a camera is taken.
    show_camera(10);
    assert_eq!(held(&mut window).1, Some(10));
    show_camera(11);
    assert_eq!(held(&mut window).1, Some(11));

    // Without one, a picture of a camera is taken, and the cameras rest for 200 ms.
    window.in_shell(|shell, window, cx| shell.set_software_video(true, window, cx));
    // The follower starts again and can take the picture that the window holds: a rest that
    // this begins ends first.
    window.until_pictures_taken();
    window
        .cx
        .executor()
        .advance_clock(Duration::from_millis(200));
    window.park();
    show_camera(20);
    assert_eq!(held(&mut window).1, Some(20));
    // A picture of a camera that arrives in the rest waits; one of the share is taken.
    show_camera(21);
    calls.show_main_picture_for_test(Some(MainPicture {
        share: ShareId::Own,
        frame: frame(30),
    }));
    assert_eq!(held(&mut window), (Some(30), Some(20)));
    window
        .cx
        .executor()
        .advance_clock(Duration::from_millis(199));
    window.park();
    assert_eq!(held(&mut window), (Some(30), Some(20)));
    // At the end of the rest the picture that waited is taken, and the next rest begins.
    window.cx.executor().advance_clock(Duration::from_millis(1));
    window.park();
    assert_eq!(held(&mut window).1, Some(21));
    show_camera(22);
    assert_eq!(held(&mut window).1, Some(21));
    window
        .cx
        .executor()
        .advance_clock(Duration::from_millis(200));
    window.park();
    assert_eq!(held(&mut window).1, Some(22));
}
