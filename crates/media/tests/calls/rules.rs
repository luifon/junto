//! The call rules of the server as the calls of a client follow them: a move between voice
//! channels (ADR 0511) and meeting mode (ADR 0505, ADR 0512).

use junto_client_core::protocol::call::{BlockReason, CallPlace};
use junto_media::calls::{Outcome, Phase, Unused};
use junto_media::engine::ShareState;
use junto_media::share::{CaptureRequest, PictureSource, ShareQuality, SoundChoice};

use crate::scene::{ABSENT_DBFS, MEDIA_SERVER, PATIENCE, Person, Room, account, model_when};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs a LiveKit server: run through tools/with-test-livekit.sh"]
async fn a_moved_person_joins_the_destination_first_and_is_in_its_call_afterwards() {
    let _media = MEDIA_SERVER.read().await;
    let (ann, ben) = (Person::new(440.0), Person::new(880.0));
    let (room, ann_at) = Room::new("Acme", &ann, "Ann").await;
    let ben_at = room.member(&ann_at, &ben, "Ben").await;
    let (ann_id, ben_id) = (account(&ann_at), account(&ben_at));
    let focus = room.second_voice(&ann_at, "Focus").await;
    let CallPlace::Channel {
        channel_id: focus_id,
        ..
    } = focus.place
    else {
        panic!("a voice channel");
    };
    ann.enter(room.target).await;
    ben.enter(room.target).await;
    ben.until_heard(&ann).await;
    let before = ben.calls.view().call.unwrap().identity;

    // Ann, who holds "move members" as the owner, moves Ben. The device of Ben joins the
    // room of the destination, the server completes the move, and Ben is in that call.
    ann_at
        .api()
        .move_participant(room.voice, ben_id, focus_id)
        .await
        .unwrap();
    let view = ben
        .until("Ben is in the call of the destination", |view| {
            view.call
                .as_ref()
                .is_some_and(|call| call.target == focus && call.phase == Phase::Connected)
        })
        .await;
    assert_ne!(view.call.unwrap().identity, before);
    assert_eq!(view.outcome, Some(Outcome::Moved { target: focus }));
    model_when(&ann_at, "Ben is in Focus and Ann alone in Daily", |model| {
        let inside = |place: &CallPlace, account_id| {
            model
                .call_at(place)
                .is_some_and(|call| call.participant(account_id).is_some())
        };
        inside(&focus.place, ben_id)
            && !inside(&room.target.place, ben_id)
            && inside(&room.target.place, ann_id)
    })
    .await;
    // The two are in two rooms: neither hears the other.
    ben.until_silent(&ann).await;
    ann.until_silent(&ben).await;
    // Ann follows by herself, and they hear each other in the destination.
    ann.enter(focus).await;
    ben.until_heard_afresh(&ann).await;
    ann.until_heard_afresh(&ben).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs a LiveKit server: run through tools/with-test-livekit.sh"]
async fn a_person_whom_the_presenter_allows_to_speak_opens_the_own_microphone() {
    let _media = MEDIA_SERVER.read().await;
    let (ann, ben) = (Person::new(440.0), Person::new(880.0));
    let (room, ann_at) = Room::new("Acme", &ann, "Ann").await;
    let ben_at = room.member(&ann_at, &ben, "Ben").await;
    let (ann_id, ben_id) = (account(&ann_at), account(&ben_at));
    let place = room.target.place;
    ann.enter(room.target).await;
    ben.enter(room.target).await;
    ann.until_heard(&ben).await;

    // Ann shares and turns meeting mode on: the microphone of Ben is closed by it.
    ann.calls.start_share(CaptureRequest {
        picture: PictureSource::Display(1),
        sound: SoundChoice::None,
        quality: ShareQuality::default(),
    });
    ann.until("the share runs", |view| {
        view.call
            .as_ref()
            .is_some_and(|call| matches!(call.share, ShareState::Running(_)))
    })
    .await;
    model_when(&ann_at, "the server shows the share", |model| {
        model.call_at(&place).is_some_and(|call| {
            call.participant(ann_id)
                .is_some_and(|ann| ann.screen_share.is_some())
        })
    })
    .await;
    // The server takes the start when the media server has listed the picture track,
    // which its check observes a moment after the report.
    let started = tokio::time::Instant::now();
    while let Err(error) = ann_at.api().start_meeting(room.voice).await {
        assert!(
            started.elapsed() < PATIENCE,
            "meeting mode did not start: {error}"
        );
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
    ben.until("the meeting closed the microphone of Ben", |view| {
        view.call.as_ref().is_some_and(|call| {
            call.microphone_block
                .as_ref()
                .is_some_and(|block| block.reason == BlockReason::Meeting)
        })
    })
    .await;
    ann.until_silent(&ben).await;
    assert!(!ben.calls.view().muted);

    // Ben raises a hand, and Ann allows him to speak. The microphone of Ben does not open
    // by itself: his own setting is "closed" from that moment, and his hand is lowered.
    ben_at.api().raise_hand().await.unwrap();
    model_when(&ann_at, "the hand of Ben", |model| {
        model
            .call_at(&place)
            .is_some_and(|call| call.participant(ben_id).is_some_and(|ben| ben.hand_raised))
    })
    .await;
    ann_at
        .api()
        .unblock_microphone(room.voice, ben_id)
        .await
        .unwrap();
    let view = ben
        .until("Ben may speak", |view| {
            view.call
                .as_ref()
                .is_some_and(|call| call.microphone_block.is_none())
        })
        .await;
    assert!(view.muted, "the microphone opened by itself");
    model_when(
        &ann_at,
        "Ben is allowed, muted, and his hand is down",
        |model| {
            model.call_at(&place).is_some_and(|call| {
                call.meeting.is_some()
                    && call.participant(ben_id).is_some_and(|ben| {
                        ben.microphone_block.is_none() && ben.muted && !ben.hand_raised
                    })
            })
        },
    )
    .await;
    ann.host.clear_played();
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    assert!(ann.hears(ben.hz) < ABSENT_DBFS, "{}", ann.hears(ben.hz));
    // Ben opens the microphone himself.
    ben.calls.set_muted(false);
    ann.until_heard(&ben).await;

    // The presenter removes a block whatever its reason (ADR 0512), and the client is not
    // told who removed it: the block of a moderator ends in the same way while meeting
    // mode goes on.
    let held_by = |reason: Option<BlockReason>| {
        move |view: &junto_media::calls::CallsView| {
            view.call.as_ref().is_some_and(|call| {
                call.microphone_block.as_ref().map(|block| block.reason) == reason
            })
        }
    };
    ann_at
        .api()
        .block_microphone(room.voice, ben_id)
        .await
        .unwrap();
    let view = ben
        .until(
            "a moderator closed the microphone of Ben",
            held_by(Some(BlockReason::Moderator)),
        )
        .await;
    assert!(!view.muted);
    ann_at
        .api()
        .unblock_microphone(room.voice, ben_id)
        .await
        .unwrap();
    let view = ben
        .until("the block of the moderator is removed", held_by(None))
        .await;
    assert!(view.muted, "the microphone opened by itself");
    ben.calls.set_muted(false);
    ann.until_heard_afresh(&ben).await;

    // The end of the share ends meeting mode (ADR 0512).
    ann.calls.stop_share();
    model_when(&ben_at, "meeting mode has ended", |model| {
        model
            .call_at(&place)
            .is_some_and(|call| call.meeting.is_none())
    })
    .await;
    // Without meeting mode the end of a block returns the microphone to the person's own
    // setting (ADR 0503): it is open, and Ann hears Ben again.
    ann_at
        .api()
        .block_microphone(room.voice, ben_id)
        .await
        .unwrap();
    ben.until(
        "a moderator closed the microphone of Ben",
        held_by(Some(BlockReason::Moderator)),
    )
    .await;
    ann.until_silent(&ben).await;
    ann_at
        .api()
        .unblock_microphone(room.voice, ben_id)
        .await
        .unwrap();
    let view = ben
        .until("the block of the moderator is removed", held_by(None))
        .await;
    assert!(!view.muted);
    ann.until_heard_afresh(&ben).await;
}

/// ADR 0788: a share that was asked for at one seat does not start at another.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs a LiveKit server: run through tools/with-test-livekit.sh"]
async fn a_share_asked_for_in_one_call_does_not_start_in_the_next() {
    let _media = MEDIA_SERVER.read().await;
    let (ann, ben) = (Person::new(440.0), Person::new(880.0));
    let (room, ann_at) = Room::new("Acme", &ann, "Ann").await;
    let ben_at = room.member(&ann_at, &ben, "Ben").await;
    let ben_id = account(&ben_at);
    let focus = room.second_voice(&ann_at, "Focus").await;
    let CallPlace::Channel {
        channel_id: focus_id,
        ..
    } = focus.place
    else {
        panic!("a voice channel");
    };
    // Ann is in the call of the destination and gets what is shared there.
    ann.enter(focus).await;
    ben.enter(room.target).await;
    let asked_at = ben.calls.seat().unwrap();
    let request = || CaptureRequest {
        picture: PictureSource::Display(1),
        sound: SoundChoice::System,
        quality: ShareQuality::default(),
    };

    // Ann moves Ben. The share that Ben chose in the first call is confirmed after it.
    ann_at
        .api()
        .move_participant(room.voice, ben_id, focus_id)
        .await
        .unwrap();
    ben.until("Ben is in the call of the destination", |view| {
        view.call
            .as_ref()
            .is_some_and(|call| call.target == focus && call.phase == Phase::Connected)
    })
    .await;
    ben.calls.start_share_in(&asked_at, request());
    let view = ben
        .until("the share has its result", |view| {
            view.unused.is_some()
                || view
                    .call
                    .as_ref()
                    .is_some_and(|call| call.share != ShareState::Off)
        })
        .await;
    assert_eq!(
        view.call.unwrap().share,
        ShareState::Off,
        "the share of the first call started in the call of the destination"
    );
    assert_eq!(view.unused, Some(Unused::Share));
    ben.calls.dismiss_unused();
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    assert_eq!(ben.calls.view().call.unwrap().share, ShareState::Off);
    assert!(ben.capture.started().is_empty());
    assert!(ann.calls.view().call.unwrap().shares.is_empty());

    // A share that is asked for at the seat that Ben holds starts.
    let held = ben.calls.seat().unwrap();
    assert_ne!(held, asked_at);
    ben.calls.start_share_in(&held, request());
    ben.until("the share runs", |view| {
        view.call
            .as_ref()
            .is_some_and(|call| matches!(call.share, ShareState::Running(_)))
    })
    .await;
    ann.until("Ann sees the share", |view| {
        view.call
            .as_ref()
            .is_some_and(|call| call.shares.len() == 1)
    })
    .await;
    assert_eq!(ben.calls.view().unused, None);
}
