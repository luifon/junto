//! The controls of calls that need no media server: the person's own microphone and sound
//! settings, the sound settings of the application, and a join that the server refuses.

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
/// their number. The follower converts the newest picture on a thread of the runtime before
/// it asks for a frame. After a burst of pictures a frame draws the picture whose
/// conversion started first, which can already be the newest one, and the next frame the
/// newest one: one or two drawings.
#[gpui::test]
fn received_pictures_cause_one_drawing_per_frame_of_the_display(cx: &mut TestAppContext) {
    use junto_media::engine::{MainPicture, ShareId};
    use junto_media::share::picture::VideoPicture;
    use junto_media::share::synthetic::two_colours;

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
    // A picture of a share, `width` pixels wide.
    let picture = |width: u32| {
        let pixels = two_colours(width, 2, (255, 255, 255), (255, 255, 255));
        MainPicture {
            share: ShareId::Own,
            frame: std::sync::Arc::new(VideoPicture::new(&pixels, false, (width, 2)).unwrap()),
        }
    };
    // (every drawing of the window, the drawings that pictures asked for, the width held)
    let draws = |window: &Window| window.shell.read_with(&window.cx, |shell, _| shell.draws());
    // The window can still hold work for a frame from what it loaded before: it is given
    // some frames, until it holds none.
    for _ in 0..20 {
        if window.frame() == 0 {
            break;
        }
        window.park();
    }

    // Fifty pictures arrive, and the application does its work after each one. No frame
    // of the display passes: no picture asks for a drawing, and the application comes to
    // rest each time. Another cause can draw the window in this time (an event of the
    // server); the pictures do not, so the drawings stay far below one per picture.
    let (all, before, _) = draws(&window);
    for width in 1..=50 {
        calls.show_main_picture_for_test(Some(picture(width)));
        window.park();
    }
    let (all_after, asked, _) = draws(&window);
    assert_eq!(
        asked, before,
        "a picture was drawn without a frame of the display"
    );
    assert!(
        all_after - all < 10,
        "the window was drawn {} times for fifty pictures without a frame",
        all_after - all
    );
    // The picture whose conversion started first, and then the newest one.
    window.until_pictures_taken();
    let (_, after_burst, width) = draws(&window);
    assert!(
        (before + 1..=before + 2).contains(&after_burst),
        "{} drawings for a burst of fifty pictures",
        after_burst - before
    );
    assert_eq!(width, Some(50));
    // Without a newer picture a frame draws nothing more.
    window.frame();
    window.park();
    assert_eq!(draws(&window).1, after_burst);
    // Five pictures between two waits for frames: at most two drawings each time, and the
    // last picture is drawn.
    for width in 51..=60 {
        calls.show_main_picture_for_test(Some(picture(width)));
        window.park();
        if width % 5 == 0 {
            let drawn = draws(&window).1;
            window.until_pictures_taken();
            let (_, now, shown) = draws(&window);
            assert!(
                now - drawn <= 2,
                "{} drawings for five pictures",
                now - drawn
            );
            assert_eq!(shown, Some(width));
        }
    }
    let (all_end, _, _) = draws(&window);
    eprintln!(
        "drawings of the window for any cause: {} for fifty pictures without a frame, {} in the whole test",
        all_after - all,
        all_end - all
    );
}

/// ADR 0774: a picture of a call that a newer one replaced is removed from the GPU images of
/// every window and is freed. Many pictures of the watched share, of the small pictures of
/// the shares and of the cameras are drawn in the window and then in the separate call
/// window; each window then holds GPU images only for the pictures that it draws now,
/// whatever their number. The pictures of the watched share and of the cameras that were
/// replaced are freed, and on macOS so are their pixel buffers of Core Video, which the
/// atlas does not show.
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
    use junto_media::share::picture::{PictureFrame, VideoPicture};
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
    // keeps every image that a window held for a picture, a weak reference of each picture
    // of the watched share and of the cameras, and on macOS their pixel buffers.
    let mut seen: Vec<Arc<RenderImage>> = Vec::new();
    let given: std::cell::RefCell<Vec<Vec<std::sync::Weak<VideoPicture>>>> = Default::default();
    #[cfg(target_os = "macos")]
    let buffers: std::cell::RefCell<Vec<Vec<core_video::pixel_buffer::CVPixelBuffer>>> =
        Default::default();
    let mut give = |window: &mut Window, shells: &[&gpui::Entity<Shell>], shade: u8| {
        let share = || {
            Arc::new(PictureFrame {
                width: 64,
                height: 36,
                bgra: vec![shade; 64 * 36 * 4],
                source: (64, 36),
            })
        };
        let camera = |own: bool| {
            let pixels = two_colours(64, 36, (shade, 0, 0), (0, 0, shade));
            Arc::new(VideoPicture::new(&pixels, own, (64, 36)).unwrap())
        };
        let watched = two_colours(64, 36, (0, shade, 0), (0, 0, shade));
        let pictures = [
            Arc::new(VideoPicture::new(&watched, false, (64, 36)).unwrap()),
            camera(true),
            camera(false),
        ];
        given
            .borrow_mut()
            .push(pictures.iter().map(Arc::downgrade).collect());
        calls.show_main_picture_for_test(Some(MainPicture {
            share: ShareId::Of(other),
            frame: pictures[0].clone(),
        }));
        calls.show_small_pictures_for_test(HashMap::from([
            (ShareId::Own, share()),
            (ShareId::Of(other), share()),
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
    // The pictures of the watched share and of the cameras that are not freed, and on macOS
    // their pixel buffers that something other than the test holds, among the pictures
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
    assert!(
        images.len() >= usize::from(PICTURES),
        "{} images for {PICTURES} pictures of each source",
        images.len()
    );
    // The small pictures of the shares are images on every system, and so are the watched
    // share and the cameras on other systems than macOS. A camera whose tile is not in view
    // is held and not drawn.
    assert!(
        in_gpu(&mut window.cx, &shown) >= 2,
        "the window did not draw the small pictures of the shares"
    );
    assert_eq!(
        kept(&mut window.cx, &images, &shown),
        0,
        "the window holds GPU images of {} pictures that it no longer draws",
        images.len()
    );
    assert_eq!(
        not_freed(&mut window, 1),
        (0, 0),
        "(pictures, pixel buffers) of the watched share and the cameras that were replaced and not freed"
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
    let mut detached = VisualTestContext::from_window(handle, cx);
    for shade in 1..=PICTURES {
        images = give(&mut window, &[&main_shell, &detached_shell], shade);
    }
    let shown = held(&window, &detached_shell);
    // The small pictures of the shares are images on every system, and so are the watched
    // share and the cameras on other systems than macOS. A camera whose tile is not in view
    // is held and not drawn.
    assert!(
        in_gpu(&mut detached, &shown) >= 2,
        "the call window did not draw the small pictures of the shares"
    );
    assert_eq!(
        kept(&mut detached, &images, &shown),
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
            in_gpu(&mut detached, &images)
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
        junto_client::ui::take_painted_surfaces();
        window.cx.update(|window, _| window.refresh());
        window.settle();
        let painted = junto_client::ui::take_painted_surfaces();
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
