//! A voice channel that hides who is inside (ADR 0514, ADR 0517). The server gives a new
//! place a join token for pictures only and nothing of the call, until the admission check
//! has seen the device in the room.

use std::time::Duration;

use junto_client_core::protocol::call::UpdateCallSettings;
use junto_media::audio::synthetic::tone_level;
use junto_media::engine::MicrophoneState;

use crate::limit::limited;
use crate::scene::{ABSENT_DBFS, Person, Room, account, media_server_shared, model_when};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs a LiveKit server: run through tools/with-test-livekit.sh"]
async fn a_person_who_enters_a_hidden_voice_channel_speaks_and_reads_the_call_afterwards() {
    limited(async {
        let _media = media_server_shared().await;
        let (ann, ben, cal) = (Person::new(440.0), Person::new(880.0), Person::new(1320.0));
        let (room, ann_at) = Room::new("Acme", &ann, "Ann").await;
        let ben_at = room.member(&ann_at, &ben, "Ben").await;
        let cal_at = room.member(&ann_at, &cal, "Cal").await;
        let (ann_id, ben_id) = (account(&ann_at), account(&ben_at));
        let place = room.target.place;
        // A call keeps the setting that its channel has when the call begins.
        ann_at
            .api()
            .update_call_settings(
                room.voice,
                &UpdateCallSettings {
                    user_limit: None,
                    hides_occupants: Some(true),
                },
            )
            .await
            .unwrap();

        ann.enter(room.target).await;
        ben.enter(room.target).await;

        // The token of Ben allowed pictures only. The check that saw the device of Ben in the
        // room gave it the microphone, and the engine opened the microphone then: each hears
        // the other.
        ann.until_heard(&ben).await;
        ben.until_heard(&ann).await;
        let view = ben
            .until("the microphone of Ben runs", |view| {
                view.call
                    .as_ref()
                    .is_some_and(|call| call.microphone == MicrophoneState::Running)
            })
            .await;
        let call = view.call.unwrap();
        assert_eq!((call.microphone_block, call.sound_block), (None, None));

        // Ben reads the call from the event of the own entry on. A person outside learns
        // that the channel is in use, and nothing else.
        model_when(&ben_at, "Ben reads who is inside", |model| {
            model.call_at(&place).is_some_and(|call| {
                call.hides_occupants
                    && call.participant(ann_id).is_some()
                    && call.participant(ben_id).is_some()
            })
        })
        .await;
        let outside = model_when(&cal_at, "the channel is in use", |model| {
            model.channel(room.voice).is_some_and(|held| held.in_use)
        })
        .await;
        assert_eq!(outside.channel(room.voice).unwrap().call, None);

        // Ann blocks the sound of Ben, and Ben leaves and enters again. The answer of the
        // server to a join states no block in a call that hides who is inside, so the calls
        // of Ben keep the sound off until the server has listed Ben: nothing is played at
        // any moment.
        ann_at.api().block_sound(room.voice, ben_id).await.unwrap();
        ben.until("Ben has the sound block", |view| {
            view.call
                .as_ref()
                .is_some_and(|call| call.sound_block.is_some())
        })
        .await;
        ben.calls.leave().await;
        ben.host.clear_played();
        let cleared = tokio::time::Instant::now();
        ben.enter(room.target).await;
        ben.until("the server tells the sound block again", |view| {
            view.call
                .as_ref()
                .is_some_and(|call| call.sound_block.is_some())
        })
        .await;
        // Two seconds of the call: the tone of Ann is in none of the sound that was played
        // since the join, from its first sample.
        tokio::time::sleep(Duration::from_secs(2)).await;
        let (played, rate) = ben.host.played_since_clear();
        assert!(played.len() > rate as usize, "{}", played.len());
        // The sink keeps the last 12 seconds: everything since the join is examined.
        assert!(cleared.elapsed() < Duration::from_secs(10));
        for part in played.chunks(rate as usize / 4) {
            let heard = tone_level(part, rate, ann.hz);
            assert!(heard < ABSENT_DBFS, "{heard}");
        }

        // Without the block Ben hears the call, and the new place has its microphone.
        ann_at
            .api()
            .unblock_sound(room.voice, ben_id)
            .await
            .unwrap();
        ben.until_heard_afresh(&ann).await;
        ann.until_heard_afresh(&ben).await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs a LiveKit server: run through tools/with-test-livekit.sh"]
async fn a_media_connection_waits_for_the_server_listing_before_becoming_ready() {
    limited(async {
        use crate::scene::until;
        use crate::support::FaultProxy;
        use junto_media::calls::Phase;

        let _media = media_server_shared().await;
        let (ann, ben) = (Person::new(440.0), Person::new(880.0));
        let (room, ann_at) = Room::new("Acme", &ann, "Ann").await;
        let proxy = FaultProxy::start(&room.server).await;
        let ben_at = room.member_at(&ann_at, &ben, "Ben", &proxy.address()).await;
        let cal = Person::new(1320.0);
        let cal_at = room.member(&ann_at, &cal, "Cal").await;
        ann.enter(room.target).await;
        let path = Room::join_path(&room.target);
        proxy.hold(&path);
        let joining = ben.calls.join(room.target);
        until("the join waits before reaching the server", || {
            !proxy.seen(&path).is_empty()
        })
        .await;
        proxy.block_streams(true);
        proxy.cut_streams();
        proxy.release();
        proxy.hold("/api/v1/snapshot");
        proxy.hold("/api/v1/call");
        joining.await.unwrap();
        let view = ben
            .until(
                "the media transport is connected but admission is not known",
                |view| {
                    view.call
                        .as_ref()
                        .is_some_and(|call| call.phase == Phase::Entering)
                },
            )
            .await;
        let call = view.call.unwrap();
        assert_ne!(call.phase, Phase::Connected);
        assert_eq!(call.microphone, MicrophoneState::Closed);
        assert!(
            ben.host.opened_inputs().is_empty(),
            "capture opened before admission"
        );
        assert!(!ben_at.live().view().model.as_ref().is_some_and(|model| {
            model
                .call_at(&room.target.place)
                .is_some_and(|call| call.participant(account(&ben_at)).is_some())
        }));
        // The same holds for as long as the state does not list the person: it is read again
        // after a time in which an open gate would have started the microphone.
        tokio::time::sleep(Duration::from_millis(500)).await;
        let call = ben.calls.view().call.unwrap();
        assert_eq!(call.phase, Phase::Entering);
        assert_eq!(call.microphone, MicrophoneState::Closed);
        assert!(
            ben.host.opened_inputs().is_empty(),
            "capture opened before admission"
        );
        proxy.block_streams(false);
        proxy.release();
        ben.until(
            "the server lists this place and the call becomes ready",
            |view| {
                view.call
                    .as_ref()
                    .is_some_and(|call| call.phase == Phase::Connected)
            },
        )
        .await;
        let view = ben.calls.view();
        let identity = &view.call.as_ref().unwrap().identity;
        let model = ben_at.live().view().model.unwrap();
        assert_eq!(
            &model
                .call_at(&room.target.place)
                .unwrap()
                .participant(account(&ben_at))
                .unwrap()
                .identity,
            identity
        );
        ben.until("admission opens the microphone", |view| {
            view.call
                .as_ref()
                .is_some_and(|call| call.microphone == MicrophoneState::Running)
        })
        .await;
        ann.until_heard(&ben).await;
        ben.until_heard(&ann).await;
        ben_at
            .api()
            .invite_to_call(room.voice, account(&cal_at))
            .await
            .unwrap();
    })
    .await;
}

/// A person in a call that hides who is inside enters the same call on a second device. The
/// server ends the place of the first device, and the place of the second one has the rules
/// of a new place of that call (ADR 0514, ADR 0517): a token for pictures only until the
/// admission check has seen the device in the room, and the sound block of a moderator from
/// its first moment, although the server states no block in its answer.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs a LiveKit server: run through tools/with-test-livekit.sh"]
async fn a_place_that_moves_to_another_device_in_a_hidden_call_keeps_its_rules() {
    limited(async {
        use junto_media::calls::Outcome;

        let _media = media_server_shared().await;
        let (ann, phone, laptop, cal) = (
            Person::new(440.0),
            Person::new(880.0),
            Person::new(660.0),
            Person::new(1320.0),
        );
        let (room, ann_at) = Room::new("Acme", &ann, "Ann").await;
        let phone_at = room.member(&ann_at, &phone, "Ben").await;
        let laptop_at = room.second_device(&phone_at, &laptop).await;
        let cal_at = room.member(&ann_at, &cal, "Cal").await;
        let (ann_id, ben_id) = (account(&ann_at), account(&phone_at));
        let place = room.target.place;
        ann_at
            .api()
            .update_call_settings(
                room.voice,
                &UpdateCallSettings {
                    user_limit: None,
                    hides_occupants: Some(true),
                },
            )
            .await
            .unwrap();
        ann.enter(room.target).await;
        phone.enter(room.target).await;
        ann.until_heard(&phone).await;
        phone.until_heard(&ann).await;
        ann_at.api().block_sound(room.voice, ben_id).await.unwrap();
        phone
            .until("the phone has the sound block", |view| {
                view.call
                    .as_ref()
                    .is_some_and(|call| call.sound_block.is_some())
            })
            .await;

        // Ben enters the call on the laptop. The call of the phone ends, and its microphone
        // and output close.
        laptop.host.clear_played();
        let cleared = tokio::time::Instant::now();
        laptop.enter(room.target).await;
        let identity = laptop.calls.view().call.unwrap().identity;
        let ended = phone
            .until("the call of the phone has ended", |view| {
                view.call.is_none()
            })
            .await;
        assert!(
            matches!(ended.outcome, Some(Outcome::Ended { target, .. }) if target == room.target),
            "{:?}",
            ended.outcome
        );
        ann.until_silent(&phone).await;

        // The laptop has the block from its first moment: the tone of Ann is in none of the
        // sound that it played since the join.
        laptop
            .until(
                "the server tells the sound block of the new place",
                |view| {
                    view.call
                        .as_ref()
                        .is_some_and(|call| call.sound_block.is_some())
                },
            )
            .await;
        let (played, rate) = laptop.host.played_since_clear();
        // The sink keeps the last 12 seconds: everything since the join is examined.
        assert!(cleared.elapsed() < Duration::from_secs(10));
        for part in played.chunks(rate as usize / 4) {
            let heard = tone_level(part, rate, ann.hz);
            assert!(heard < ABSENT_DBFS, "{heard}");
        }

        // The admission check gives the place of the laptop its microphone: Ann hears it.
        laptop
            .until("the microphone of the laptop runs", |view| {
                view.call
                    .as_ref()
                    .is_some_and(|call| call.microphone == MicrophoneState::Running)
            })
            .await;
        ann.until_heard(&laptop).await;

        // The server lists Ben once, at the place of the laptop, to the people inside; a
        // person outside learns only that the channel is in use.
        model_when(
            &ann_at,
            "Ann reads Ben at the place of the laptop",
            |model| {
                model.call_at(&place).is_some_and(|call| {
                    call.participants.len() == 2
                        && call
                            .participant(ben_id)
                            .is_some_and(|ben| ben.identity == identity)
                        && call.participant(ann_id).is_some()
                })
            },
        )
        .await;
        model_when(&laptop_at, "the laptop reads who is inside", |model| {
            model.call_at(&place).is_some_and(|call| {
                call.participant(ann_id).is_some()
                    && call
                        .participant(ben_id)
                        .is_some_and(|ben| ben.identity == identity)
            })
        })
        .await;
        let outside = model_when(&cal_at, "the channel is in use", |model| {
            model.channel(room.voice).is_some_and(|held| held.in_use)
        })
        .await;
        assert_eq!(outside.channel(room.voice).unwrap().call, None);

        // Without the block the laptop hears the call.
        ann_at
            .api()
            .unblock_sound(room.voice, ben_id)
            .await
            .unwrap();
        laptop.until_heard_afresh(&ann).await;
    })
    .await;
}
