//! Two people in one call: what each hears of the other, and what the server is told.

use junto_client_core::protocol::call::{BlockReason, LeaveReason};
use junto_media::calls::{EndReason, Outcome, Phase};
use junto_media::engine::MicrophoneState;
use livekit_protocol::{MuteTrackRequest, signal_request};

use crate::limit::limited;
use crate::scene::{
    ABSENT_DBFS, PATIENCE, Person, Room, account, media_server_alone, media_server_shared,
    model_when, plain_voice, until,
};
use crate::signal_proxy::SignalProxy;
use crate::support::FaultProxy;

const REASON: &str = "needs a LiveKit server: run through tools/with-test-livekit.sh";

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs a LiveKit server: run through tools/with-test-livekit.sh"]
async fn two_people_hear_each_other_and_a_mute_stops_the_sound() {
    limited(async {
        let _ = REASON;
        let _media = media_server_shared().await;
        let (ann, ben, cal) = (Person::new(440.0), Person::new(880.0), Person::new(1320.0));
        let (room, ann_at) = Room::new("Acme", &ann, "Ann").await;
        let ben_at = room.member(&ann_at, &ben, "Ben").await;
        let cal_at = room.member(&ann_at, &cal, "Cal").await;
        let (ann_id, ben_id) = (account(&ann_at), account(&ben_at));
        let place = room.target.place;

        ann.enter(room.target).await;
        ben.enter(room.target).await;

        // Each hears the tone of the other at about the level at which it was sent, and not
        // the own one.
        let heard = ben.until_heard(&ann).await;
        assert!(heard < -8.0, "{heard}");
        ann.until_heard(&ben).await;
        assert!(ben.hears(ben.hz) < ABSENT_DBFS, "{}", ben.hears(ben.hz));
        assert!(ann.hears(ann.hz) < ABSENT_DBFS, "{}", ann.hears(ann.hz));
        let call = ben.calls.view().call.unwrap();
        assert_eq!(call.microphone, MicrophoneState::Running);
        assert_eq!(call.input_device.unwrap().name, "Tone microphone");
        assert_eq!(call.output_device.unwrap().name, "Measuring sink");

        // The levels of the call: the own microphone and the other person.
        let mut levels = ben.calls.levels();
        // The value that `wait_for` returns holds a lock of the levels and is not kept.
        let seen = tokio::time::timeout(
            crate::scene::PATIENCE,
            levels.wait_for(|levels| {
                levels.own.speaking
                    && levels.input > 0.5
                    && levels
                        .people
                        .get(&ann_id)
                        .is_some_and(|ann| ann.speaking && ann.level > 0.5)
            }),
        )
        .await
        .is_ok();
        assert!(seen, "{:?}", *ben.calls.levels().borrow());

        // A person outside the voice channel sees who is inside and who speaks (ADR 0513).
        model_when(&cal_at, "both speak", |model| {
            model.call_at(&place).is_some_and(|call| {
                call.participants.len() == 2
                    && call.speaking.contains(&ann_id)
                    && call.speaking.contains(&ben_id)
            })
        })
        .await;

        // A mute stops the sound, and the server shows the person as muted.
        ann.calls.set_muted(true);
        ben.until_silent(&ann).await;
        model_when(&cal_at, "Ann is muted and does not speak", |model| {
            model.call_at(&place).is_some_and(|call| {
                call.participant(ann_id).is_some_and(|ann| ann.muted)
                    && !call.speaking.contains(&ann_id)
                    && call.speaking.contains(&ben_id)
            })
        })
        .await;
        ann.until_heard(&ben).await;
        ann.calls.set_muted(false);
        ben.until_heard(&ann).await;
        model_when(&cal_at, "Ann is not muted", |model| {
            model
                .call_at(&place)
                .is_some_and(|call| call.participant(ann_id).is_some_and(|ann| !ann.muted))
        })
        .await;

        // The volume of one person for one listener: half is 6 dB less, and 200 % is the most.
        let full = ben.until_steady(&ann).await;
        ben.calls.set_volume(room.target.server_id, ann_id, 0.5);
        ben.until_level(ann.hz, "half the volume", |level| level < full - 4.0)
            .await;
        let half = ben.until_steady(&ann).await;
        assert!((full - half - 6.0).abs() < 1.5, "{full} {half}");
        ben.calls.set_volume(room.target.server_id, ann_id, 1.0);
        ben.until_level(ann.hz, "the full volume", |level| level > full - 1.5)
            .await;

        // Sound off: nothing is played, and the server shows it.
        ben.calls.set_deafened(true);
        ben.until_silent(&ann).await;
        model_when(&cal_at, "Ben has the sound off", |model| {
            model
                .call_at(&place)
                .is_some_and(|call| call.participant(ben_id).is_some_and(|ben| ben.deafened))
        })
        .await;
        ben.calls.set_deafened(false);
        ben.until_heard(&ann).await;

        // Ann leaves: her tone stops for Ben, the server lists Ben alone, and Ann has no call.
        ann.calls.leave().await;
        assert_eq!(ann.calls.view().call, None);
        assert_eq!(ann.calls.view().outcome, None);
        ben.until_silent(&ann).await;
        model_when(&cal_at, "Ann has left", |model| {
            model.call_at(&place).is_some_and(|call| {
                call.participants.len() == 1 && call.participant(ben_id).is_some()
            })
        })
        .await;
        model_when(&ann_at, "Ann is in no call", |model| {
            model.own_call.is_none()
        })
        .await;
        ben.calls.leave().await;
        model_when(&cal_at, "the call has ended", |model| {
            model.channel(room.voice).is_some_and(|held| !held.in_use)
        })
        .await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs a LiveKit server: run through tools/with-test-livekit.sh"]
async fn push_to_talk_sends_only_while_the_key_is_held() {
    limited(async {
        let _media = media_server_shared().await;
        let (ann, ben) = (Person::new(440.0), Person::new(880.0));
        let (room, ann_at) = Room::new("Acme", &ann, "Ann").await;
        room.member(&ann_at, &ben, "Ben").await;
        let mut voice = plain_voice();
        voice.push_to_talk = true;
        ann.calls.set_voice(voice);

        ann.enter(room.target).await;
        ben.enter(room.target).await;
        ann.until_heard(&ben).await;
        // Ann is in the call for as long as Ben's tone took to arrive, and sent nothing.
        assert!(ben.hears(ann.hz) < ABSENT_DBFS, "{}", ben.hears(ann.hz));
        assert!(!ann.calls.levels().borrow().own.speaking);

        ann.calls.set_talking(true);
        ben.until_heard(&ann).await;
        ann.calls.set_talking(false);
        ben.until_silent(&ann).await;

        // Voice activation sends the open microphone.
        ann.calls.set_voice(plain_voice());
        ben.until_heard(&ann).await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs a LiveKit server: run through tools/with-test-livekit.sh"]
async fn a_microphone_block_of_the_server_stops_the_publication_until_it_is_removed() {
    limited(async {
        let _media = media_server_shared().await;
        let (ann, ben) = (Person::new(440.0), Person::new(880.0));
        let (room, ann_at) = Room::new("Acme", &ann, "Ann").await;
        let ben_at = room.member(&ann_at, &ben, "Ben").await;
        let (ann_id, ben_id) = (account(&ann_at), account(&ben_at));
        ann.enter(room.target).await;
        ben.enter(room.target).await;
        ann.until_heard(&ben).await;
        model_when(&ann_at, "Ben is in the call", |model| {
            model
                .call_at(&room.target.place)
                .is_some_and(|call| call.participant(ben_id).is_some())
        })
        .await;

        // The owner closes the microphone of Ben. His client closes the device and tells why.
        ann_at
            .api()
            .block_microphone(room.voice, ben_id)
            .await
            .unwrap();
        let view = ben
            .until("the block is shown", |view| {
                view.call.as_ref().is_some_and(|call| {
                    call.microphone_block.is_some() && call.microphone == MicrophoneState::Closed
                })
            })
            .await;
        let block = view.call.unwrap().microphone_block.unwrap();
        assert_eq!((block.by, block.reason), (ann_id, BlockReason::Moderator));
        ann.until_silent(&ben).await;
        // Ben stays in the call and hears Ann. His own mute setting is unchanged.
        ben.until_heard(&ann).await;
        assert!(!ben.calls.view().muted);
        assert_eq!(ben.calls.view().call.unwrap().phase, Phase::Connected);

        // Without the block the microphone returns to the setting of the person: it was open.
        ann_at
            .api()
            .unblock_microphone(room.voice, ben_id)
            .await
            .unwrap();
        ben.until("the block is gone", |view| {
            view.call.as_ref().is_some_and(|call| {
                call.microphone_block.is_none() && call.microphone == MicrophoneState::Running
            })
        })
        .await;
        ann.until_heard(&ben).await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs a LiveKit server: run through tools/with-test-livekit.sh"]
async fn a_blocked_microphone_is_not_opened_when_the_person_enters_again() {
    limited(async {
        let _media = media_server_shared().await;
        let (ann, ben) = (Person::new(440.0), Person::new(880.0));
        let (room, ann_at) = Room::new("Acme", &ann, "Ann").await;
        let ben_at = room.member(&ann_at, &ben, "Ben").await;
        let ben_id = account(&ben_at);
        ann.enter(room.target).await;
        ben.enter(room.target).await;
        ann.until_heard(&ben).await;
        ann_at
            .api()
            .block_microphone(room.voice, ben_id)
            .await
            .unwrap();
        ben.until("the block is shown", |view| {
            view.call.as_ref().is_some_and(|call| {
                call.microphone_block.is_some() && call.microphone == MicrophoneState::Closed
            })
        })
        .await;
        ann.until_silent(&ben).await;

        // Ben leaves and enters again while Ann stays. The block still holds, and the
        // microphone stays closed from the first moment of the new media connection.
        ben.calls.leave().await;
        let opened = ben.host.opened_inputs().len();
        ben.enter(room.target).await;
        ben.until("the block is shown again", |view| {
            view.call.as_ref().is_some_and(|call| {
                call.microphone_block.is_some() && call.microphone == MicrophoneState::Closed
            })
        })
        .await;
        ben.until_heard(&ann).await;
        assert_eq!(ben.host.opened_inputs().len(), opened);
        assert!(ann.hears(ben.hz) < ABSENT_DBFS, "{}", ann.hears(ben.hz));

        ann_at
            .api()
            .unblock_microphone(room.voice, ben_id)
            .await
            .unwrap();
        ben.until("the block is gone", |view| {
            view.call.as_ref().is_some_and(|call| {
                call.microphone_block.is_none() && call.microphone == MicrophoneState::Running
            })
        })
        .await;
        assert_eq!(ben.host.opened_inputs().len(), opened + 1);
        ann.until_heard(&ben).await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs a LiveKit server: run through tools/with-test-livekit.sh"]
async fn a_person_whom_a_moderator_removes_is_told_and_does_not_return() {
    limited(async {
        let _media = media_server_shared().await;
        let (ann, ben) = (Person::new(440.0), Person::new(880.0));
        let (room, ann_at) = Room::new("Acme", &ann, "Ann").await;
        let ben_at = room.member(&ann_at, &ben, "Ben").await;
        let ben_id = account(&ben_at);
        ann.enter(room.target).await;
        ben.enter(room.target).await;
        ann.until_heard(&ben).await;

        ann_at
            .api()
            .disconnect_participant(room.voice, ben_id)
            .await
            .unwrap();
        let view = ben
            .until("the call has ended", |view| view.call.is_none())
            .await;
        assert_eq!(
            view.outcome,
            Some(Outcome::Ended {
                target: room.target,
                reason: EndReason::Server(Some(LeaveReason::Disconnected)),
            })
        );
        ann.until_silent(&ben).await;
        // The client asks for no new place: the time of two reconnects passes, and the server
        // still lists Ann alone.
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        assert_eq!(ben.calls.view().call, None);
        let model = ann_at.live().view().model.unwrap();
        assert_eq!(
            model
                .call_at(&room.target.place)
                .unwrap()
                .participants
                .len(),
            1
        );
        assert!(ben_at.live().view().model.unwrap().own_call.is_none());

        // The person can enter again by an action of their own.
        ben.calls.dismiss_outcome();
        ben.enter(room.target).await;
        ann.until_heard(&ben).await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs a LiveKit server: run through tools/with-test-livekit.sh"]
async fn the_call_returns_after_the_media_server_was_restarted() {
    limited(async {
        // No other test of this run uses the media server while it is stopped.
        let _media = media_server_alone().await;
        let container = std::env::var("JUNTO_TEST_LIVEKIT_CONTAINER")
            .expect("JUNTO_TEST_LIVEKIT_CONTAINER names the LiveKit server of the test run");
        let (ann, ben) = (Person::new(440.0), Person::new(880.0));
        let (room, ann_at) = Room::new("Acme", &ann, "Ann").await;
        room.member(&ann_at, &ben, "Ben").await;
        ann.enter(room.target).await;
        ben.enter(room.target).await;
        ann.until_heard(&ben).await;
        ben.until_heard(&ann).await;

        // A task listens for the end of the tone from before the media server stops.
        let quiet = {
            let (host, hz) = (ann.host.clone(), ben.hz);
            tokio::spawn(async move {
                let started = tokio::time::Instant::now();
                while started.elapsed() < PATIENCE {
                    let (played, rate) = host.played(std::time::Duration::from_millis(500));
                    if junto_media::audio::synthetic::tone_level(&played, rate, hz) < ABSENT_DBFS {
                        return true;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                }
                false
            })
        };

        // The media server is stopped and started in two steps. While it is stopped, the
        // interruption is a state that lasts, so no client can pass through it unseen.
        docker(&["stop", "--time", "1", &container]).await;
        for person in [&ann, &ben] {
            let view = person
                .until("the interruption is shown", |view| {
                    view.call
                        .as_ref()
                        .is_some_and(|call| call.phase == Phase::Reconnecting)
                })
                .await;
            assert!(view.call.unwrap().interrupted_at.is_some());
        }
        assert!(quiet.await.unwrap(), "the tone of Ben did not stop for Ann");
        docker(&["start", &container]).await;

        // Each client restores the call, and the person stays in it.
        for person in [&ann, &ben] {
            let view = person
                .until("the call is back", |view| {
                    view.call
                        .as_ref()
                        .is_some_and(|call| call.phase == Phase::Connected)
                })
                .await;
            assert_eq!(view.call.unwrap().interrupted_at, None);
            assert_eq!(view.outcome, None);
        }
        ann.until_heard_afresh(&ben).await;
        ben.until_heard_afresh(&ann).await;
    })
    .await;
}

/// The call library sends each mute and each open of the microphone track to the media
/// server. While it restores its signal connection, these requests wait, and they go out in
/// the order in which the person made them: the media server holds the last one.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs a LiveKit server: run through tools/with-test-livekit.sh"]
async fn a_mute_and_an_open_during_a_restored_connection_reach_the_media_server_in_order() {
    limited(async {
        // No other test of this run uses the media server while it is paused.
        let _media = media_server_alone().await;
        let container = std::env::var("JUNTO_TEST_LIVEKIT_CONTAINER")
            .expect("JUNTO_TEST_LIVEKIT_CONTAINER names the LiveKit server of the test run");
        let (ann, ben) = (Person::new(440.0), Person::new(880.0));
        let (room, ann_at) = Room::new("Acme", &ann, "Ann").await;
        room.member(&ann_at, &ben, "Ben").await;
        ann.enter(room.target).await;
        ben.enter(room.target).await;
        ben.until_heard(&ann).await;

        // The paused media server answers nothing, and the clients start to restore their
        // signal connection when its pings are not answered. Ann closes and opens her
        // microphone while hers is restored, so both requests wait for the connection.
        docker(&["pause", &container]).await;
        ann.until("the interruption is shown", |view| {
            view.call
                .as_ref()
                .is_some_and(|call| call.phase == Phase::Reconnecting)
        })
        .await;
        // The changes are apart in time, as the changes of a person are, so that each one
        // waits for the connection on its own.
        for _ in 0..3 {
            ann.calls.set_muted(true);
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            ann.calls.set_muted(false);
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }
        docker(&["unpause", &container]).await;
        ann.until("the call is back", |view| {
            view.call
                .as_ref()
                .is_some_and(|call| call.phase == Phase::Connected)
        })
        .await;

        // The media server holds the track of Ann as open, and Ben hears her.
        ben.until_heard_afresh(&ann).await;
        let check = ben.calls.sound_check().await.unwrap();
        assert!(
            check
                .remote_tracks
                .iter()
                .all(|(_, source, _, muted)| source != "Microphone" || !muted),
            "the media server holds a microphone track as muted: {check}"
        );
    })
    .await;
}

/// A network that drops the signal connection keeps the last bytes that a client wrote to it
/// and can deliver them after the client has restored its connection on a new one. The call
/// library counts a mute request as sent when it is written, and the media server discards a
/// request that arrives on the replaced connection. The resume carries the mute state of each
/// track at the time of publishing, and the media server does not take it. After each resume
/// the call library sends the mute state of each local track again, so the media server
/// holds the last one.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs a LiveKit server: run through tools/with-test-livekit.sh"]
async fn an_open_written_to_a_lost_signal_connection_reaches_the_media_server_after_the_resume() {
    limited(async {
        let _media = media_server_shared().await;
        let proxy = SignalProxy::start().await;
        let (ann, ben) = (Person::new(440.0), Person::new(880.0));
        let (room, ann_at) = Room::with_media_at("Acme", &ann, "Ann", &proxy.url()).await;
        room.member(&ann_at, &ben, "Ben").await;
        ann.enter(room.target).await;
        ben.enter(room.target).await;
        ben.until_heard(&ann).await;
        ann.calls.set_muted(true);
        until_microphone_muted(&ben, true).await;

        // The signal connections of both people stop passing bytes. Ann opens her microphone
        // before her client finds that its pings are not answered, so the request is written to
        // the lost connection. The clients then restore their connections through new ones.
        proxy.hold_open();
        ann.calls.set_muted(false);
        ann.until("the interruption is shown", |view| {
            view.call
                .as_ref()
                .is_some_and(|call| call.phase == Phase::Reconnecting)
        })
        .await;
        ann.until("the call is back", |view| {
            view.call
                .as_ref()
                .is_some_and(|call| call.phase == Phase::Connected)
        })
        .await;
        let released = proxy.release().await;
        // The open was written to the held connection. A client that had found the loss first
        // would hold the request and send it on the new connection, and the media server would
        // hold the track as open without the request after the resume.
        assert!(
            released.requests.iter().any(|request| matches!(
                &request.message,
                Some(signal_request::Message::Mute(MuteTrackRequest {
                    muted: false,
                    ..
                }))
            )),
            "the open was not written to a held connection; the held requests: {:?}",
            released.requests
        );
        eprintln!(
            "{} bytes of the lost connections reached the media server late",
            released.sent
        );

        // The media server holds the track of Ann as open, and Ben hears her.
        until_microphone_muted(&ben, false).await;
        ben.until_heard_afresh(&ann).await;
    })
    .await;
}

/// Waits until the media server tells `listener` that the microphone track of the other
/// person is muted or open.
async fn until_microphone_muted(listener: &Person, muted: bool) {
    let started = tokio::time::Instant::now();
    loop {
        let check = listener.calls.sound_check().await.unwrap();
        let microphones: Vec<bool> = check
            .remote_tracks
            .iter()
            .filter(|(_, source, _, _)| source == "Microphone")
            .map(|(_, _, _, held)| *held)
            .collect();
        if !microphones.is_empty() && microphones.iter().all(|held| *held == muted) {
            return;
        }
        assert!(
            started.elapsed() < PATIENCE,
            "after {PATIENCE:?} the media server does not hold the microphone track as {}: \
             {check}",
            if muted { "muted" } else { "open" }
        );
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
}

/// ADR 0904: a test that ends while it holds the media server paused, here cancelled as the
/// limit of a test cancels it, gives the media server back running: the tests after it
/// need it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs a LiveKit server: run through tools/with-test-livekit.sh"]
async fn a_test_that_ends_while_the_media_server_is_paused_gives_it_back_running() {
    limited(async {
        let container = std::env::var("JUNTO_TEST_LIVEKIT_CONTAINER")
            .expect("JUNTO_TEST_LIVEKIT_CONTAINER names the LiveKit server of the test run");
        let (paused, is_paused) = tokio::sync::oneshot::channel();
        let pausing = tokio::spawn(async move {
            let _media = media_server_alone().await;
            docker(&["pause", &container]).await;
            let _ = paused.send(());
            std::future::pending::<()>().await;
        });
        is_paused.await.unwrap();
        pausing.abort();
        assert!(pausing.await.unwrap_err().is_cancelled());
        let _media = media_server_shared().await;
        until("the media server answers", crate::scene::livekit_answers).await;
    })
    .await;
}

async fn docker(arguments: &[&str]) {
    // A test that a limit cancels while the command runs ends the command too.
    let done = tokio::process::Command::new("docker")
        .args(arguments)
        .kill_on_drop(true)
        .output()
        .await
        .unwrap();
    assert!(done.status.success(), "{done:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs a LiveKit server: run through tools/with-test-livekit.sh"]
async fn a_new_token_that_arrives_after_the_person_left_is_given_up() {
    limited(async {
        let _media = media_server_shared().await;
        let (ann, ben) = (Person::new(440.0), Person::new(880.0));
        let (room, ann_at) = Room::new("Acme", &ann, "Ann").await;
        let proxy = FaultProxy::start(&room.server).await;
        let ben_at = room.member_at(&ann_at, &ben, "Ben", &proxy.address()).await;
        ann.enter(room.target).await;
        ben.enter(room.target).await;
        ann.until_heard(&ben).await;

        // A second connection with the identity of Ben enters the room. The media server ends
        // the connection of his client for it, and the server still holds his place: the
        // client asks for a new token. That request waits on its way to the server.
        let again = ben_at.api().join_channel_call(room.voice).await.unwrap();
        assert_eq!(again.identity, ben.calls.view().call.unwrap().identity);
        let path = Room::renew_path(&room.target, &again.identity);
        let before = proxy.seen(&path).len();
        proxy.hold(&path);
        let (second, _events) = livekit::Room::connect(
            &again.media_url,
            again.token.expose(),
            livekit::RoomOptions::default(),
        )
        .await
        .unwrap();
        until("Ben asks for a new token", || {
            proxy.seen(&path).len() > before
        })
        .await;
        assert_eq!(ben.calls.view().call.unwrap().phase, Phase::Reconnecting);

        // Ben leaves while the request waits. The requests of the device to the server go one
        // after the other, so the server gets the renewal of the held seat first and renews it;
        // the leave that follows gives the seat up.
        let leaving = ben.calls.leave();
        ben.until("the call has ended", |view| view.call.is_none())
            .await;
        proxy.release();
        leaving.await;
        until("every request for a place has its answer", || {
            proxy
                .seen(&path)
                .iter()
                .skip(before)
                .all(|seen| seen.answer.is_some())
        })
        .await;
        let _ = second.close().await;
        assert!(ben_at.api().own_call().await.unwrap().call.is_none());
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        assert!(ben_at.api().own_call().await.unwrap().call.is_none());
        let view = ben.calls.view();
        assert_eq!((view.call, view.outcome), (None, None));
        assert_eq!(proxy.seen(&path).len(), before + 1);
    })
    .await;
}
