//! The end of a media connection: the negotiations of the publisher of a room that closes
//! (ADR 0733). The test is a test program of its own, so that the lines of the call library
//! that it reads are only those of its own person: no other test negotiates in the process,
//! and no task of another test is cancelled with its lines half written.

#![allow(clippy::unwrap_used, clippy::expect_used)]

#[allow(dead_code)]
#[path = "../../../client-core/tests/e2e/support.rs"]
mod support;

#[allow(dead_code)]
#[path = "../../../client-core/tests/e2e/limit.rs"]
mod limit;

#[allow(dead_code)]
#[path = "../calls/scene.rs"]
mod scene;

use std::time::Duration;

use livekit::webrtc::stats::RtcStats;

use limit::limited;
use scene::{
    NegotiationLine, PATIENCE, Person, Room, docker, media_server_alone, negotiation_line_count,
    negotiation_lines, until,
};

/// Each leave closes a room in which the microphone and the camera were published.
const LEAVES: usize = 5;

/// The SDK waits this long for the answer to an offer of the publisher.
const ANSWER_LIMIT: Duration = Duration::from_secs(10);

/// The lines of one kind from position `from` on.
fn count(from: usize, kind: NegotiationLine) -> usize {
    (negotiation_lines(from).iter())
        .filter(|(_, line)| *line == kind)
        .count()
}

/// The tasks of the fast publish mode that started from position `from` on and have not ended.
fn tasks_running(from: usize) -> usize {
    count(from, NegotiationLine::TaskStarted)
        .saturating_sub(count(from, NegotiationLine::TaskEnded))
}

/// Waits until the microphone and the camera of `person` are published and send.
async fn until_sending(person: &Person) {
    let sending = tokio::time::timeout(PATIENCE, async {
        loop {
            if let Ok(stats) = person.calls.statistics().await {
                let kinds: Vec<&str> = (stats.publisher_stats.iter())
                    .filter_map(|stat| match stat {
                        RtcStats::OutboundRtp(stat) if stat.sent.packets_sent > 0 => {
                            Some(stat.stream.kind.as_str())
                        }
                        _ => None,
                    })
                    .collect();
                if kinds.contains(&"audio") && kinds.contains(&"video") {
                    return;
                }
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await;
    assert!(
        sending.is_ok(),
        "the microphone and the camera did not send"
    );
}

/// The close of a room unpublishes its tracks, each of which would ask for a negotiation of
/// the publisher, and then closes the connection of the publisher.
///
/// First, five leaves while the microphone and the camera are published and no negotiation
/// runs: the session stops its negotiations before the tracks are unpublished, and the engine
/// leaves the unpublishing to the close, so a leave starts none, and no offer is made in a
/// closed connection.
///
/// Then a leave while a negotiation waits for its answer: the media server is paused, the
/// camera stops, which negotiates the publisher, and the person leaves once the offer is
/// sent. The close ends the wait at once; without that the task waits the 10 seconds of the
/// SDK for an answer of a closed session.
///
/// The test holds the media server alone, and Ann is alone in the room: another person's
/// publisher negotiates when she leaves, and the lines do not name their session.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs a LiveKit server: run through tools/with-test-livekit.sh"]
async fn a_closing_room_starts_no_negotiation_and_ends_the_wait_for_an_answer() {
    limited(async {
        let _media = media_server_alone().await;
        let start = negotiation_line_count();
        let ann = Person::new(440.0);
        let (room, _ann_at) = Room::new("Acme", &ann, "Ann").await;

        for _ in 0..LEAVES {
            ann.enter(room.target).await;
            ann.calls.start_camera(None);
            until_sending(&ann).await;
            // A negotiation that the publications asked for ends before the leave, so that
            // the count holds only what the leave starts.
            until("no negotiation of the publisher runs", || {
                tasks_running(start) == 0
            })
            .await;
            let before = negotiation_line_count();
            ann.calls.leave().await;
            until(
                "no negotiation of the publisher runs after the leave",
                || tasks_running(start) == 0,
            )
            .await;
            assert_eq!(
                count(before, NegotiationLine::Negotiating),
                0,
                "negotiations of the publisher started by the leave"
            );
        }

        let container = std::env::var("JUNTO_TEST_LIVEKIT_CONTAINER")
            .expect("JUNTO_TEST_LIVEKIT_CONTAINER names the LiveKit server of the test run");
        ann.enter(room.target).await;
        ann.calls.start_camera(None);
        until_sending(&ann).await;
        until("no negotiation of the publisher runs", || {
            tasks_running(start) == 0
        })
        .await;
        let before = negotiation_line_count();
        // The paused media server answers no offer.
        docker(&["pause", &container]).await;
        ann.calls.stop_camera();
        until("the offer of the stop of the camera is sent", || {
            count(before, NegotiationLine::OfferSent) > 0
        })
        .await;
        let leave = ann.calls.leave();
        until("the negotiation that waits for its answer ends", || {
            tasks_running(start) == 0
        })
        .await;
        let lines = negotiation_lines(before);
        let sent = lines
            .iter()
            .find(|(_, line)| *line == NegotiationLine::OfferSent)
            .map(|(at, _)| *at)
            .unwrap();
        let ended = lines
            .iter()
            .find(|(at, line)| *line == NegotiationLine::TaskEnded && *at >= sent)
            .map(|(at, _)| *at)
            .unwrap();
        docker(&["unpause", &container]).await;
        leave.await;
        let waited = ended.duration_since(sent);
        assert!(
            waited < ANSWER_LIMIT / 2,
            "the negotiation waited {waited:?} for an answer after the room began to close"
        );

        let all = negotiation_lines(start);
        let kinds = |kind| all.iter().filter(|(_, line)| *line == kind).count();
        assert_eq!(kinds(NegotiationLine::Failed), 0, "failed negotiations");
        assert_eq!(
            kinds(NegotiationLine::OfferInClosedConnection),
            0,
            "offers in a closed connection"
        );
    })
    .await;
}
