//! A voice channel that hides who is inside (ADR 0514, ADR 0517). The server gives a new
//! place a join token for pictures only and nothing of the call, until the admission check
//! has seen the device in the room.

use std::time::Duration;

use junto_client_core::protocol::call::UpdateCallSettings;
use junto_media::audio::synthetic::tone_level;
use junto_media::engine::MicrophoneState;

use crate::scene::{ABSENT_DBFS, MEDIA_SERVER, Person, Room, account, model_when};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs a LiveKit server: run through tools/with-test-livekit.sh"]
async fn a_person_who_enters_a_hidden_voice_channel_speaks_and_reads_the_call_afterwards() {
    let _media = MEDIA_SERVER.read().await;
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
}
