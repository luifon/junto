//! The call routes and the call state against the real server with a real LiveKit server
//! and a real participant (ADR 0902). The tests are ignored in the plain run and run with:
//!
//! ```sh
//! tools/with-test-postgres.sh tools/with-test-livekit.sh tools/serial.sh \
//!     cargo test -p junto-client-core --test e2e calls_live:: -- --ignored
//! ```
//!
//! The participant is the tool of `spikes/test-participant`, built as the tests of the
//! server describe, or the binary that `JUNTO_TEST_PARTICIPANT_BIN` names.

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};

use junto_client_core::ServerHandle;
use junto_client_core::api::ApiError;
use junto_client_core::book::ConnectionMode;
use junto_client_core::live::{Notice, StreamStatus};
use junto_client_core::protocol::call::{
    CallJoin, CallPlace, EntryOutcome, InvitationOutcome, UpdateCallSettings,
};
use junto_client_core::protocol::error::ErrorCode;
use junto_client_core::protocol::events::{Event, Level};
use junto_client_core::protocol::structure::ChannelKind;

use crate::stream::{arrived, channel, events, live, model_when, when};
use crate::support::{Device, TestServer, claim, join, logged_in};

const REASON: &str = "needs a LiveKit server: run through tools/with-test-livekit.sh";

fn participant_tool() -> PathBuf {
    let path = std::env::var_os("JUNTO_TEST_PARTICIPANT_BIN").map_or_else(
        || {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../spikes/test-participant/target/release/test-participant")
        },
        PathBuf::from,
    );
    assert!(
        path.exists(),
        "{} is missing. Build it once: (cd spikes/test-participant && ../../tools/serial.sh \
         cargo build --release), or set JUNTO_TEST_PARTICIPANT_BIN.",
        path.display()
    );
    path
}

/// A process that joins the room of a call with the token of a join and publishes sound.
struct Participant(Child);

impl Participant {
    fn start(join: &CallJoin) -> Self {
        let child = Command::new(participant_tool())
            .arg("publish")
            .args(["--url", &join.media_url, "--token", join.token.expose()])
            .args(["--leave-after", "300"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        Self(child)
    }
}

impl Drop for Participant {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn refused(error: ApiError) -> ErrorCode {
    error.code().expect("a refusal of the server")
}

struct Three {
    _server: TestServer,
    devices: [Device; 3],
    ann: ServerHandle,
    ben: ServerHandle,
    cal: ServerHandle,
}

async fn three() -> Three {
    let server = TestServer::start_with_media("Acme").await;
    let devices = [Device::new(), Device::new(), Device::new()];
    let ann = devices[0].add(&server.address()).await;
    ann.dispatch(claim(&server, "Ann")).await.unwrap();
    let code = super::invite(&ann, 2).await;
    let ben = devices[1].add(&server.address()).await;
    ben.dispatch(join(&code, "Ben")).await.unwrap();
    let cal = devices[2].add(&server.address()).await;
    cal.dispatch(join(&code, "Cal")).await.unwrap();
    Three {
        _server: server,
        devices,
        ann,
        ben,
        cal,
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a LiveKit server: run through tools/with-test-livekit.sh"]
async fn a_call_with_a_real_participant_is_shown_to_the_accounts_that_may_see_it() {
    let _ = REASON;
    let scene = three().await;
    let (ann, ben, cal) = (&scene.ann, &scene.ben, &scene.cal);
    let (ann_id, cal_id) = (logged_in(ann), logged_in(cal));
    let workspace_id = live(&scene.devices[0], ann).await.workspaces[0]
        .workspace
        .id;
    let voice = channel(ann, workspace_id, "Daily", ChannelKind::Voice)
        .await
        .id;
    let place = CallPlace::Channel {
        workspace_id,
        channel_id: voice,
    };
    live(&scene.devices[1], ben).await;
    live(&scene.devices[2], cal).await;
    for handle in [ben, cal] {
        model_when(handle, "the voice channel", |model| {
            model.channel(voice).is_some()
        })
        .await;
    }

    // Ann asks for a place and gets the token of the media server. Her state names the
    // call at once; the others see nobody until the media server lists her.
    let joined = ann.api().join_channel_call(voice).await.unwrap();
    assert_eq!(joined.call.place, place);
    assert!(!joined.token.expose().is_empty() && !joined.media_url.is_empty());
    let model = model_when(ann, "the own call", |model| model.own_call.is_some()).await;
    let own = model.own_call.clone().unwrap();
    assert_eq!(
        (own.call_id, &own.identity),
        (joined.call.id, &joined.identity)
    );
    assert!(
        !ben.live()
            .view()
            .model
            .unwrap()
            .channel(voice)
            .unwrap()
            .in_use
    );
    assert_eq!(ann.api().own_call().await.unwrap().call.unwrap().own, own);

    let _participant = Participant::start(&joined);
    for handle in [ann, ben, cal] {
        let model = model_when(handle, "Ann in the call", |model| {
            model.channel(voice).is_some_and(|held| {
                held.in_use
                    && held
                        .call
                        .as_ref()
                        .is_some_and(|call| call.participant(ann_id).is_some())
            })
        })
        .await;
        let call = model.call_at(&place).unwrap();
        assert_eq!(call.id, joined.call.id);
        assert!(call.started_at.is_some());
        assert_eq!(call.participants.len(), 1);
    }

    // Her own settings and her hand reach the people outside.
    ann.api().report_call_state(true, false).await.unwrap();
    ann.api().raise_hand().await.unwrap();
    model_when(ben, "the mute and the hand", |model| {
        model
            .call_at(&place)
            .and_then(|call| call.participant(ann_id))
            .is_some_and(|ann| ann.muted && ann.hand_raised && !ann.deafened)
    })
    .await;

    // A lock: Ben is refused with its code, asks to enter, and Ann, who is inside, gets
    // the request and accepts it.
    ann.api().lock_call(voice).await.unwrap();
    model_when(ben, "the lock", |model| {
        model
            .call_at(&place)
            .is_some_and(|call| call.lock.as_ref().is_some_and(|lock| lock.by == ann_id))
    })
    .await;
    assert_eq!(
        refused(ben.api().join_channel_call(voice).await.unwrap_err()),
        ErrorCode::CallLocked
    );
    let mut told_to_ben = ben.live().notices();
    let request = ben.api().request_entry(voice).await.unwrap();
    let model = model_when(ann, "the request", |model| !model.entry_requests.is_empty()).await;
    assert_eq!(model.entry_requests, std::slice::from_ref(&request));
    assert!(cal.live().view().model.unwrap().entry_requests.is_empty());
    ann.api()
        .answer_entry_request(request.id, true)
        .await
        .unwrap();
    model_when(ann, "the request is answered", |model| {
        model.entry_requests.is_empty()
    })
    .await;
    let told = arrived(&mut told_to_ben).await;
    let accepted = |event: &Event| {
        matches!(event, Event::CallEntryAnswered { request_id, outcome }
            if *request_id == request.id && *outcome == EntryOutcome::Accepted)
    };
    assert!(
        events(&told).iter().any(|(_, event)| accepted(event)),
        "{told:?}"
    );
    // The accepted person enters past the lock, and leaves again.
    let entered = ben.api().join_channel_call(voice).await.unwrap();
    assert_eq!(entered.call.id, joined.call.id);
    model_when(ben, "the own call of Ben", |model| model.own_call.is_some()).await;
    ben.api().leave_call().await.unwrap();
    model_when(ben, "Ben has left", |model| model.own_call.is_none()).await;
    ann.api().unlock_call(voice).await.unwrap();

    // An invitation is held by the invited account and by the one that made it until
    // its outcome.
    let invitation = ann.api().invite_to_call(voice, cal_id).await.unwrap();
    let model = model_when(cal, "the invitation", |model| {
        model.incoming_invitations().count() == 1
    })
    .await;
    assert_eq!(model.invitations, std::slice::from_ref(&invitation));
    model_when(ann, "the invitation that Ann made", |model| {
        model.invitations.len() == 1 && model.incoming_invitations().count() == 0
    })
    .await;
    assert!(ben.live().view().model.unwrap().invitations.is_empty());
    let mut told_to_ann = ann.live().notices();
    cal.api()
        .refuse_call_invitation(invitation.id)
        .await
        .unwrap();
    for handle in [ann, cal] {
        model_when(handle, "the invitation has ended", |model| {
            model.invitations.is_empty()
        })
        .await;
    }
    let told = arrived(&mut told_to_ann).await;
    let declined = |event: &Event| {
        matches!(event, Event::CallInvitationResolved { invitation_id, outcome }
            if *invitation_id == invitation.id && *outcome == InvitationOutcome::Declined)
    };
    assert!(
        events(&told).iter().any(|(_, event)| declined(event)),
        "{told:?}"
    );

    // A call keeps the setting "hides who is inside" that its channel had when the call
    // began (ADR 0517). The channel hides from its next call on, and the people outside
    // still see who is in the call that runs.
    let hide = |hides| UpdateCallSettings {
        user_limit: None,
        hides_occupants: Some(hides),
    };
    ann.api()
        .update_call_settings(voice, &hide(true))
        .await
        .unwrap();
    let model = model_when(ben, "the setting of the channel", |model| {
        model
            .channel(voice)
            .is_some_and(|held| held.channel.hides_occupants)
    })
    .await;
    let held = model.channel(voice).unwrap();
    assert!(held.in_use);
    let call = held.call.as_ref().unwrap();
    assert!(!call.hides_occupants && call.participant(ann_id).is_some());
    let shown = ben.api().channel_call(voice).await.unwrap().call.unwrap();
    assert!(!shown.hides_occupants && shown.participants.len() == 1);
    ann.api()
        .update_call_settings(voice, &hide(false))
        .await
        .unwrap();
    model_when(ben, "the setting is off again", |model| {
        model.channel(voice).is_some_and(|held| {
            !held.channel.hides_occupants
                && held
                    .call
                    .as_ref()
                    .is_some_and(|call| call.participant(ann_id).is_some())
        })
    })
    .await;

    // The server of a call keeps its connection when another server is shown, in the mode
    // "connect only when selected" too. It ends with the call.
    ann.set_connection_mode(ConnectionMode::WhenSelected)
        .unwrap();
    scene.devices[0].client.select(None);
    when(ann, "the background level", |view| {
        view.status == StreamStatus::Live && view.level == Some(Level::Background)
    })
    .await;
    ann.api().leave_call().await.unwrap();
    let view = when(ann, "off", |view| view.status == StreamStatus::Off).await;
    assert!(view.model.is_none());
    model_when(ben, "the call has ended", |model| {
        model
            .channel(voice)
            .is_some_and(|held| !held.in_use && held.call.is_none())
    })
    .await;
    crate::stream::agrees_with_a_snapshot(ben).await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a LiveKit server: run through tools/with-test-livekit.sh"]
async fn a_direct_call_rings_a_server_that_is_not_selected_and_a_decline_pauses_the_rings() {
    let scene = three().await;
    let (ann, ben) = (&scene.ann, &scene.ben);
    let (ann_id, ben_id) = (logged_in(ann), logged_in(ben));
    live(&scene.devices[0], ann).await;
    scene.devices[1].client.start_event_streams().unwrap();
    when(ben, "the background level", |view| {
        view.status == StreamStatus::Live && view.level == Some(Level::Background)
    })
    .await;
    let place = CallPlace::direct(ann_id, ben_id);

    let joined = ann.api().join_direct_call(ben_id).await.unwrap();
    assert_eq!(joined.call.place, place);
    let model = model_when(ben, "the ring", |model| {
        model.incoming_invitations().count() == 1
    })
    .await;
    let ring = model.invitations[0].clone();
    assert_eq!((ring.place, ring.invited_by), (place, ann_id));
    model_when(ann, "the ring that Ann made", |model| {
        model.invitations == [ring.clone()]
    })
    .await;

    let mut told_to_ann = ann.live().notices();
    ben.api().refuse_direct_call(ann_id).await.unwrap();
    for handle in [ann, ben] {
        model_when(handle, "the ring has ended", |model| {
            model.invitations.is_empty()
        })
        .await;
    }
    let told = arrived(&mut told_to_ann).await;
    assert!(
        told.iter()
            .any(|notice| matches!(notice, Notice::Event { event, .. }
            if matches!(event.as_ref(), Event::CallInvitationResolved { outcome, .. }
                if *outcome == InvitationOutcome::Declined))),
        "{told:?}"
    );

    // After a decline the caller cannot ring for 5 minutes (ADR 0515): the refusal names
    // the limit and the time.
    ann.api().leave_call().await.unwrap();
    model_when(ann, "Ann has left", |model| model.own_call.is_none()).await;
    let error = ann.api().join_direct_call(ben_id).await.unwrap_err();
    assert_eq!(error.code(), Some(ErrorCode::RateLimited));
    assert!(error.retry_after().is_some());

    // A muted conversation gets no ring.
    ben.api()
        .mute_direct_conversation(ann_id, None)
        .await
        .unwrap();
    model_when(ben, "the mute", |model| {
        model
            .direct_mutes
            .iter()
            .any(|mute| mute.account_id == ann_id)
    })
    .await;
}
