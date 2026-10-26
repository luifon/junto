//! Camera pictures use generated pixels through the real call and media servers.

use crate::limit::limited;
use crate::scene::{PATIENCE, Person, Room, account, media_server_shared};
use junto_client_core::protocol::call::BlockReason;
use junto_media::camera::{CameraAccess, CameraError, CameraState};
use junto_media::engine::Person as Camera;

async fn picture(person: &Person, id: Camera, left: (u8, u8, u8), right: (u8, u8, u8)) {
    let mut pictures = person.calls.camera_pictures();
    tokio::time::timeout(
        PATIENCE,
        pictures.wait_for(|pictures| {
            pictures.get(&id).is_some_and(|frame| {
                let near = |seen: Option<(u8, u8, u8)>, wanted: (u8, u8, u8)| {
                    seen.is_some_and(|seen| {
                        seen.0.abs_diff(wanted.0) < 45
                            && seen.1.abs_diff(wanted.1) < 45
                            && seen.2.abs_diff(wanted.2) < 45
                    })
                };
                near(frame.pixel(frame.width / 4, frame.height / 2), left)
                    && near(frame.pixel(frame.width * 3 / 4, frame.height / 2), right)
            })
        }),
    )
    .await
    .unwrap()
    .unwrap();
}

#[tokio::test]
#[ignore = "needs PostgreSQL and LiveKit"]
async fn two_clients_see_each_others_generated_camera() {
    limited(async {
        let _media = media_server_shared().await;
        let one = Person::new(440.0);
        let two = Person::new(880.0);
        let (room, owner) = Room::new("Cameras", &one, "One").await;
        let peer = room.member(&owner, &two, "Two").await;
        one.enter(room.target).await;
        two.enter(room.target).await;
        assert_eq!(one.camera.active(), 0);
        assert_eq!(two.camera.active(), 0);
        one.calls.start_camera(None);
        two.camera.set_colours((30, 160, 60), (210, 190, 40));
        two.calls.start_camera(None);
        picture(
            &two,
            Camera::Of(account(&owner)),
            (200, 30, 30),
            (30, 30, 200),
        )
        .await;
        picture(
            &one,
            Camera::Of(account(&peer)),
            (30, 160, 60),
            (210, 190, 40),
        )
        .await;
        // Mirroring belongs only to the own preview.
        picture(&one, Camera::Own, (30, 30, 200), (200, 30, 30)).await;
        one.until_heard(&two).await;
        two.until_heard(&one).await;
        one.calls.stop_camera();
        one.until("camera stopped", |view| {
            view.call
                .as_ref()
                .is_some_and(|call| call.camera == CameraState::Off)
        })
        .await;
        assert_eq!(one.camera.active(), 0);
        let mut pictures = two.calls.camera_pictures();
        tokio::time::timeout(
            PATIENCE,
            pictures.wait_for(|pictures| !pictures.contains_key(&Camera::Of(account(&owner)))),
        )
        .await
        .unwrap()
        .unwrap();
        one.calls.leave().await;
        two.calls.leave().await;
        assert_eq!(two.camera.active(), 0);
    })
    .await;
}

#[tokio::test]
#[ignore = "needs PostgreSQL and LiveKit"]
async fn camera_respects_permission_and_survives_microphone_block() {
    limited(async {
        let _media = media_server_shared().await;
        let one = Person::new(440.0);
        let two = Person::new(880.0);
        let (room, owner) = Room::new("Camera permission", &one, "One").await;
        let peer = room.member(&owner, &two, "Two").await;
        one.enter(room.target).await;
        two.enter(room.target).await;
        two.camera.set_access(CameraAccess::Refused);
        two.calls.start_camera(None);
        two.until("camera permission refused", |view| {
            view.call.as_ref().is_some_and(|call| {
                call.camera == CameraState::Failed(CameraError::NoPermission(CameraAccess::Refused))
            })
        })
        .await;
        assert_eq!(two.camera.active(), 0);
        two.camera.set_access(CameraAccess::Allowed);
        two.calls.start_camera(None);
        picture(
            &one,
            Camera::Of(account(&peer)),
            (200, 30, 30),
            (30, 30, 200),
        )
        .await;
        owner
            .api()
            .block_microphone(room.voice, account(&peer))
            .await
            .unwrap();
        two.until("microphone blocked", |view| {
            view.call.as_ref().is_some_and(|call| {
                call.microphone_block
                    .as_ref()
                    .is_some_and(|block| block.reason == BlockReason::Moderator)
            })
        })
        .await;
        two.camera.set_colours((30, 160, 60), (210, 190, 40));
        picture(
            &one,
            Camera::Of(account(&peer)),
            (30, 160, 60),
            (210, 190, 40),
        )
        .await;
        two.camera.set_devices(vec![]);
        two.until("camera disconnected", |view| {
            view.call
                .as_ref()
                .is_some_and(|call| call.camera == CameraState::Failed(CameraError::DeviceGone))
        })
        .await;
        one.calls.leave().await;
        two.calls.leave().await;
    })
    .await;
}

#[tokio::test]
#[ignore = "needs PostgreSQL and LiveKit"]
async fn stopping_camera_discards_late_start_and_leaving_closes_capture() {
    limited(async {
        let _media = media_server_shared().await;
        let one = Person::new(440.0);
        let (room, _) = Room::new("Camera cancellation", &one, "One").await;
        one.enter(room.target).await;
        one.camera
            .set_start_delay(std::time::Duration::from_millis(300));
        one.calls.start_camera(None);
        one.until("camera starting", |view| {
            view.call
                .as_ref()
                .is_some_and(|call| call.camera == CameraState::Starting)
        })
        .await;
        one.calls.stop_camera();
        one.until("camera stopped", |view| {
            view.call
                .as_ref()
                .is_some_and(|call| call.camera == CameraState::Off)
        })
        .await;
        tokio::time::sleep(std::time::Duration::from_millis(600)).await;
        assert_eq!(one.camera.active(), 0);
        assert!(one.calls.camera_pictures().borrow().is_empty());
        one.camera.set_start_delay(std::time::Duration::ZERO);
        one.calls.start_camera(None);
        one.until("camera running", |view| {
            view.call
                .as_ref()
                .is_some_and(|call| call.camera == CameraState::Running)
        })
        .await;
        assert_eq!(one.camera.active(), 1);
        assert_eq!(one.camera.started(), 1);
        one.calls.leave().await;
        assert_eq!(one.camera.active(), 0);
        assert!(one.calls.camera_pictures().borrow().is_empty());
    })
    .await;
}

#[tokio::test]
#[ignore = "needs PostgreSQL and LiveKit"]
async fn camera_returns_after_full_reconnect_without_reopening_capture() {
    limited(async {
        let _media = media_server_shared().await;
        let one = Person::new(440.0);
        let two = Person::new(880.0);
        let (room, owner) = Room::new("Camera reconnect", &one, "One").await;
        let _peer = room.member(&owner, &two, "Two").await;
        one.enter(room.target).await;
        two.enter(room.target).await;
        one.calls.start_camera(None);
        picture(
            &two,
            Camera::Of(account(&owner)),
            (220, 30, 30),
            (30, 50, 210),
        )
        .await;
        one.calls.simulate_full_reconnect();
        two.until("remote camera was withdrawn", |view| {
            view.call
                .as_ref()
                .is_some_and(|call| call.cameras.is_empty())
        })
        .await;
        two.until("remote camera returned", |view| {
            view.call
                .as_ref()
                .is_some_and(|call| call.cameras.len() == 1)
        })
        .await;
        one.camera.set_colours((30, 160, 60), (210, 190, 40));
        picture(
            &two,
            Camera::Of(account(&owner)),
            (30, 160, 60),
            (210, 190, 40),
        )
        .await;
        one.until("camera runs after reconnect", |view| {
            view.call.as_ref().is_some_and(|call| {
                call.camera == CameraState::Running
                    && call.phase == junto_media::calls::Phase::Connected
            })
        })
        .await;
        assert_eq!(one.camera.active(), 1);
        assert_eq!(one.camera.started(), 1);
        one.calls.leave().await;
        two.calls.leave().await;
    })
    .await;
}

#[tokio::test]
#[ignore = "needs PostgreSQL and LiveKit"]
async fn camera_returns_after_connection_loss_without_reopening_capture() {
    limited(async {
        let _media = media_server_shared().await;
        let one = Person::new(440.0);
        let two = Person::new(880.0);
        let (room, owner) = Room::new("Camera reconnect", &one, "One").await;
        let _peer = room.member(&owner, &two, "Two").await;
        one.enter(room.target).await;
        two.enter(room.target).await;
        one.calls.start_camera(None);
        picture(
            &two,
            Camera::Of(account(&owner)),
            (220, 30, 30),
            (30, 50, 210),
        )
        .await;
        one.calls.simulate_connection_loss();
        two.until("remote camera was withdrawn", |view| {
            view.call
                .as_ref()
                .is_some_and(|call| call.cameras.is_empty())
        })
        .await;
        two.until("remote camera returned", |view| {
            view.call
                .as_ref()
                .is_some_and(|call| call.cameras.len() == 1)
        })
        .await;
        one.camera.set_colours((30, 160, 60), (210, 190, 40));
        picture(
            &two,
            Camera::Of(account(&owner)),
            (30, 160, 60),
            (210, 190, 40),
        )
        .await;
        one.until("camera runs after reconnect", |view| {
            view.call.as_ref().is_some_and(|call| {
                call.camera == CameraState::Running
                    && call.phase == junto_media::calls::Phase::Connected
            })
        })
        .await;
        assert_eq!(one.camera.active(), 1);
        assert_eq!(one.camera.started(), 1);
        one.calls.leave().await;
        two.calls.leave().await;
    })
    .await;
}

#[tokio::test]
#[ignore = "needs PostgreSQL and LiveKit"]
async fn camera_frames_follow_the_visible_tile_size_and_stop_when_hidden() {
    limited(async {
        let _media = media_server_shared().await;
        let one = Person::new(440.0);
        let two = Person::new(880.0);
        let (room, owner) = Room::new("Visible camera sizes", &one, "One").await;
        room.member(&owner, &two, "Two").await;
        one.enter(room.target).await;
        two.enter(room.target).await;
        one.calls.start_camera(None);
        let id = Camera::Of(account(&owner));
        picture(&two, id, (200, 30, 30), (30, 30, 200)).await;
        two.calls
            .set_camera_views([(id, (192, 120))].into_iter().collect());
        let mut frames = two.calls.camera_pictures();
        tokio::time::timeout(
            PATIENCE,
            frames.wait_for(|frames| {
                frames
                    .get(&id)
                    .is_some_and(|frame| frame.width <= 192 && frame.height <= 120)
            }),
        )
        .await
        .unwrap()
        .unwrap();
        two.calls.set_camera_views(Default::default());
        tokio::time::timeout(PATIENCE, frames.wait_for(|frames| frames.is_empty()))
            .await
            .unwrap()
            .unwrap();
        two.calls
            .set_camera_views([(id, (640, 360))].into_iter().collect());
        tokio::time::timeout(
            PATIENCE,
            frames.wait_for(|frames| frames.get(&id).is_some_and(|frame| frame.width >= 480)),
        )
        .await
        .unwrap()
        .unwrap();
        one.calls.leave().await;
        two.calls.leave().await;
    })
    .await;
}

/// A camera whose media connection is lost is captured for a bounded time, and the
/// camera of a device is published only at a seat that the server lists.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs a LiveKit server: run through tools/with-test-livekit.sh"]
async fn a_camera_without_a_media_connection_ends_after_its_limit() {
    limited(async {
        use crate::support::FaultProxy;

        let _media = media_server_shared().await;
        let one = Person::new(440.0);
        let two = Person::new(880.0);
        let (room, owner) = Room::new("Camera limit", &one, "One").await;
        let proxy = FaultProxy::start(&room.server).await;
        let _peer = room.member_at(&owner, &two, "Two", &proxy.address()).await;
        one.enter(room.target).await;
        two.enter(room.target).await;
        two.calls.start_camera(None);
        two.until("the camera runs", |view| {
            view.call
                .as_ref()
                .is_some_and(|call| call.camera == CameraState::Running)
        })
        .await;
        assert_eq!(two.camera.active(), 1);
        // The media connection ends, and the request for a new token of the seat gets no
        // answer.
        let seat = two.calls.view().call.unwrap().identity;
        proxy.hold(&Room::renew_path(&room.target, &seat));
        two.calls.simulate_connection_loss();
        let view = two
            .until("the camera has ended", |view| {
                view.call
                    .as_ref()
                    .is_some_and(|call| matches!(call.camera, CameraState::Failed(_)))
            })
            .await;
        assert_eq!(
            view.call.unwrap().camera,
            CameraState::Failed(CameraError::Interrupted)
        );
        crate::scene::until("the capture of the camera has stopped", || {
            two.camera.active() == 0
        })
        .await;
        proxy.release();
    })
    .await;
}

/// Waits until the engine tells this of the camera of its device.
async fn until_camera(
    engine: &junto_media::engine::Engine,
    what: &str,
    holds: impl Fn(&junto_media::engine::CameraCheck) -> bool,
) -> junto_media::engine::CameraCheck {
    let started = tokio::time::Instant::now();
    loop {
        let check = engine.camera_check().await.unwrap();
        if holds(&check) {
            return check;
        }
        assert!(
            started.elapsed() < PATIENCE,
            "after {PATIENCE:?} this did not hold: {what}; the camera is {check:?}"
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
}

/// The engine publishes the camera only while its controls do not hold it (ADR 0785): a
/// hold withdraws the track and keeps the capture, and the end of the hold publishes it
/// again. A publication that the media server completes after a hold is not the publication
/// of the camera, and the engine withdraws it (`camera_published`, `hold_camera`).
#[tokio::test]
#[ignore = "needs PostgreSQL and LiveKit"]
async fn the_engine_publishes_the_camera_only_while_its_controls_do_not_hold_it() {
    limited(async {
        use junto_media::engine::{
            CameraPublication, Controls, Engine, EngineOptions, MediaJoin, Watching,
        };
        use std::sync::Arc;
        let _media = media_server_shared().await;
        let one = Person::new(440.0);
        let two = Person::new(880.0);
        let (room, owner) = Room::new("Camera hold", &one, "One").await;
        let peer = room.member(&owner, &two, "Two").await;
        let two_id = Camera::Of(account(&peer));
        one.enter(room.target).await;
        let answer = peer.api().join_channel_call(room.voice).await.unwrap();
        let held = Controls {
            camera_held: true,
            ..Controls::default()
        };
        let engine = Engine::start(
            &tokio::runtime::Handle::current(),
            EngineOptions {
                camera: Arc::new(two.camera.clone()),
                host: Arc::new(two.host.clone()),
                permission: Arc::new(two.permission.clone()),
                capture: Arc::new(two.capture.clone()),
            },
            MediaJoin {
                url: answer.media_url,
                token: answer.token,
                shares: answer.shares.unwrap_or(1),
            },
            crate::scene::plain_voice(),
            held,
            Default::default(),
            Watching::default(),
        );
        let mut state = engine.state();
        tokio::time::timeout(
            PATIENCE,
            state.wait_for(|state| state.connection == junto_media::engine::Connection::Connected),
        )
        .await
        .unwrap()
        .unwrap();
        let peer_id = account(&peer);
        let without_camera = |what: &'static str| {
            one.until(what, move |view| {
                view.call
                    .as_ref()
                    .is_some_and(|call| !call.cameras.contains(&peer_id))
            })
        };

        // A camera that starts while it is held captures and is not published.
        two.camera.set_colours((30, 160, 60), (210, 190, 40));
        engine.start_camera(None);
        let check = until_camera(&engine, "the capture of the camera runs", |check| {
            check.capture
        })
        .await;
        assert_eq!(
            (check.publication, check.answers, check.tracks),
            (CameraPublication::None, 0, 0)
        );

        // The end of the hold publishes it.
        engine.set_controls(Controls::default());
        picture(&one, two_id, (30, 160, 60), (210, 190, 40)).await;
        let check = until_camera(&engine, "the camera is published", |check| {
            check.publication == CameraPublication::Published
        })
        .await;
        assert_eq!((check.answers, check.tracks), (1, 1));

        // A hold withdraws the track and keeps the capture.
        engine.set_controls(held);
        without_camera("the camera of Two is withdrawn").await;
        let check = until_camera(&engine, "the room holds no camera track", |check| {
            check.tracks == 0
        })
        .await;
        assert_eq!(check.publication, CameraPublication::None);
        assert!(check.capture);
        assert_eq!(two.camera.active(), 1);

        // The hold ends and returns while the task of the engine does not run: the engine
        // asks for a publication, and the hold comes before the answer of the media server.
        let (waits, task) = engine.hold();
        waits.await.unwrap();
        engine.set_controls(Controls::default());
        engine.set_controls(held);
        drop(task);
        let check = until_camera(&engine, "the answer of the publication is taken", |check| {
            check.answers == 2
        })
        .await;
        assert_eq!(
            check.publication,
            CameraPublication::None,
            "a publication from before the hold is the publication of the camera"
        );
        until_camera(
            &engine,
            "the track of that publication is withdrawn",
            |check| check.tracks == 0,
        )
        .await;
        without_camera("the camera of Two stays withdrawn").await;

        // The next end of the hold publishes the camera with what it captures now.
        two.camera.set_colours((200, 30, 30), (30, 30, 200));
        engine.set_controls(Controls::default());
        picture(&one, two_id, (200, 30, 30), (30, 30, 200)).await;
        let check = until_camera(&engine, "the camera is published again", |check| {
            check.publication == CameraPublication::Published
        })
        .await;
        assert_eq!((check.answers, check.tracks), (3, 1));
        assert_eq!(two.camera.started(), 1);

        engine.close().await;
        peer.api().leave_call_as(&answer.identity).await.unwrap();
        one.calls.leave().await;
    })
    .await;
}
