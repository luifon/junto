//! One call per client over all its servers (ADR 0107, ADR 0734).

use junto_client_core::flow::FlowError;
use junto_client_core::protocol::error::ErrorCode;
use junto_media::calls::{JoinError, Outcome, Phase};

use crate::scene::{MEDIA_SERVER, Person, Room, account, model_when, until};
use crate::support::FaultProxy;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs a LiveKit server: run through tools/with-test-livekit.sh"]
async fn a_call_on_another_server_replaces_the_first_and_a_refused_join_leaves_it() {
    let _media = MEDIA_SERVER.read().await;
    let (ann, ben) = (Person::new(440.0), Person::new(880.0));
    let (first, ben_at_first) = Room::new("First", &ben, "Ben").await;
    let ann_at_first = first.member(&ben_at_first, &ann, "Ann").await;
    let (second, ann_at_second) = Room::new("Second", &ann, "Ann").await;
    let ben_at_second = second.member(&ann_at_second, &ben, "Ben").await;
    let ann_id_first = account(&ann_at_first);

    // Ann and Ben are in a call on the first server.
    ann.device.client.select(Some(ann_at_first.id()));
    ben.device.client.select(Some(ben_at_first.id()));
    ann.enter(first.target).await;
    ben.enter(first.target).await;
    ben.until_heard(&ann).await;

    // Ann enters a call on the second server. Her device leaves the first call: the first
    // server lists Ben alone, and Ben does not hear her any more.
    ann.device.client.select(Some(ann_at_second.id()));
    ann.enter(second.target).await;
    assert_eq!(ann.calls.view().call.unwrap().target, second.target);
    assert_eq!(ann.calls.view().outcome, None);
    ben.until_silent(&ann).await;
    model_when(&ben_at_first, "Ann left the first call", |model| {
        model.call_at(&first.target.place).is_some_and(|call| {
            call.participants.len() == 1 && call.participant(ann_id_first).is_none()
        })
    })
    .await;
    model_when(&ann_at_second, "Ann is in the second call", |model| {
        model.own_call.is_some()
    })
    .await;

    // Ben locks the call on the first server. Ann asks for a place there and is refused:
    // her call on the second server goes on.
    ben_at_first.api().lock_call(first.voice).await.unwrap();
    model_when(&ben_at_first, "the lock", |model| {
        model
            .call_at(&first.target.place)
            .is_some_and(|call| call.lock.is_some())
    })
    .await;
    let refused = ann.calls.join(first.target).await.unwrap_err();
    assert!(
        matches!(
            &refused,
            JoinError::Refused(FlowError::Refused { code, .. }) if *code == ErrorCode::CallLocked
        ),
        "{refused:?}"
    );
    let view = ann.calls.view();
    let call = view.call.unwrap();
    assert_eq!((call.target, call.phase), (second.target, Phase::Connected));
    assert!(
        matches!(&view.outcome, Some(Outcome::JoinFailed { target, .. }) if *target == first.target)
    );
    assert_eq!(view.joining, None);

    // Ben comes to the second server: the two are in one call again, on the second server.
    ben.device.client.select(Some(ben_at_second.id()));
    ben.enter(second.target).await;
    ben.until_heard(&ann).await;
    ann.until_heard(&ben).await;
    model_when(&ben_at_first, "the first call has ended", |model| {
        model.own_call.is_none()
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs a LiveKit server: run through tools/with-test-livekit.sh"]
async fn a_second_voice_channel_of_one_server_replaces_the_call_without_a_notice() {
    let _media = MEDIA_SERVER.read().await;
    let (ann, ben) = (Person::new(440.0), Person::new(880.0));
    let (room, ann_at) = Room::new("Acme", &ann, "Ann").await;
    room.member(&ann_at, &ben, "Ben").await;
    let other = ann_at
        .api()
        .create_channel(
            room.workspace_id,
            &junto_client_core::protocol::structure::CreateChannel {
                name: "Focus".to_owned(),
                kind: junto_client_core::protocol::structure::ChannelKind::Voice,
                category_id: None,
                position: None,
            },
        )
        .await
        .unwrap()
        .id;
    let mut focus = room.target;
    focus.place = junto_client_core::protocol::call::CallPlace::Channel {
        workspace_id: room.workspace_id,
        channel_id: other,
    };

    ann.enter(room.target).await;
    ben.enter(room.target).await;
    ben.until_heard(&ann).await;
    ann.enter(focus).await;
    ben.until_silent(&ann).await;
    let view = ann.calls.view();
    assert_eq!(view.call.unwrap().target, focus);
    assert_eq!(view.outcome, None);
    model_when(&ann_at, "Ann is in the second channel", |model| {
        model
            .own_call
            .as_ref()
            .is_some_and(|own| own.place == focus.place)
    })
    .await;
    // The state of the server settles, and the call is still the second one.
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;
    let view = ann.calls.view();
    assert_eq!(view.call.unwrap().target, focus);
    assert_eq!(view.outcome, None);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs a LiveKit server: run through tools/with-test-livekit.sh"]
async fn a_leave_while_the_server_is_asked_for_a_place_ends_the_call_and_starts_no_other() {
    let _media = MEDIA_SERVER.read().await;
    let (ann, ben) = (Person::new(440.0), Person::new(880.0));
    let (room, ann_at) = Room::new("Acme", &ann, "Ann").await;
    let proxy = FaultProxy::start(&room.server).await;
    let ben_at = room.member_at(&ann_at, &ben, "Ben", &proxy.address()).await;
    assert_eq!(ben_at.id(), room.target.server_id);
    let focus = room.second_voice(&ann_at, "Focus").await;
    let junto_client_core::protocol::call::CallPlace::Channel {
        channel_id: focus_id,
        ..
    } = focus.place
    else {
        unreachable!("a voice channel");
    };
    model_when(&ben_at, "the second voice channel", |model| {
        model.channel(focus_id).is_some()
    })
    .await;
    ann.enter(room.target).await;
    ben.enter(room.target).await;
    ann.until_heard(&ben).await;

    // Ben asks for a place in the second channel. The request waits on its way to the
    // server.
    let path = Room::join_path(&focus);
    proxy.hold(&path);
    let joining = ben.calls.join(focus);
    until("the request for the place is on its way", || {
        !proxy.seen(&path).is_empty()
    })
    .await;
    assert_eq!(ben.calls.view().joining, Some(focus));
    let opened = ben.host.opened_inputs().len();

    // Ben leaves. The call that he is in ends, and nothing is shown as asked for.
    let leaving = ben.calls.leave();
    let left = ben
        .until("the call has ended", |view| view.call.is_none())
        .await;
    ann.until_silent(&ben).await;

    // The server gets the request only now and gives the place. The client gives it up.
    proxy.release();
    assert_eq!(joining.await, Err(JoinError::Cancelled));
    assert_eq!(left.joining, None);
    leaving.await;
    assert!(ben_at.api().own_call().await.unwrap().call.is_none());
    let view = ben.calls.view();
    assert_eq!((view.call, view.joining, view.outcome), (None, None, None));
    // No media connection follows, and the microphone is not opened.
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    assert_eq!(ben.calls.view().call, None);
    assert_eq!(ben.host.opened_inputs().len(), opened);
    assert!(ben_at.api().own_call().await.unwrap().call.is_none());

    // A join after the leave is a new one.
    ben.enter(focus).await;
    assert!(ben_at.api().own_call().await.unwrap().call.is_some());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs a LiveKit server: run through tools/with-test-livekit.sh"]
async fn a_leave_does_not_end_the_call_that_another_device_of_the_account_took() {
    const OWN_CALL: &str = "/api/v1/call";
    let _media = MEDIA_SERVER.read().await;
    let (ann, phone, laptop) = (Person::new(440.0), Person::new(880.0), Person::new(660.0));
    let (room, ann_at) = Room::new("Acme", &ann, "Ann").await;
    let proxy = FaultProxy::start(&room.server).await;
    let phone_at = room
        .member_at(&ann_at, &phone, "Ben", &proxy.address())
        .await;
    let laptop_at = room.second_device(&phone_at, &laptop).await;
    ann.enter(room.target).await;
    phone.enter(room.target).await;
    ann.until_heard(&phone).await;

    // Ben leaves on the phone. What the phone tells the server waits on its way: one
    // request that names the identity of the place of the phone (ADR 0510).
    let seat = phone.calls.view().call.unwrap().identity;
    let named: String = url::form_urlencoded::byte_serialize(seat.as_bytes()).collect();
    let leave = format!("{OWN_CALL}?identity={named}");
    proxy.hold(&leave);
    let reads = proxy.seen(OWN_CALL).len();
    let leaving = phone.calls.leave();
    phone
        .until("the call has ended on the phone", |view| {
            view.call.is_none()
        })
        .await;
    until("the phone tells the server", || {
        !proxy.seen(&leave).is_empty()
    })
    .await;
    assert_eq!(proxy.seen(&leave)[0].method, "DELETE");
    // The phone does not read the place first: between a read and the request another
    // device could take the place.
    assert_eq!(proxy.seen(OWN_CALL).len(), reads);

    // Ben enters the call on the laptop before the server hears from the phone.
    laptop.enter(room.target).await;
    let identity = laptop.calls.view().call.unwrap().identity;
    ann.until_heard(&laptop).await;

    // The server hears from the phone now. The place is the one of the laptop, and stays.
    proxy.release();
    leaving.await;
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    let own = laptop_at.api().own_call().await.unwrap().call;
    assert_eq!(own.map(|call| call.own.identity), Some(identity));
    let view = laptop.calls.view();
    assert_eq!(view.call.map(|call| call.phase), Some(Phase::Connected));
    assert_eq!(view.outcome, None);
    ann.until_heard_afresh(&laptop).await;
}
