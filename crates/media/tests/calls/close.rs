//! The end of a media connection.

use std::time::Duration;

use livekit::webrtc::stats::RtcStats;

use crate::limit::limited;
use crate::scene::{
    PATIENCE, Person, Room, media_server_alone, negotiation_failures, negotiation_tasks_running,
    negotiations_started, offers_in_a_closed_connection,
};

/// Each leave closes a room in which the microphone and the camera were published.
const LEAVES: usize = 5;

/// Waits until no task that negotiates a publisher runs.
async fn until_no_negotiation_runs() {
    let idle = tokio::time::timeout(PATIENCE, async {
        while negotiation_tasks_running() > 0 {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await;
    assert!(idle.is_ok(), "a negotiation of the publisher did not end");
}

/// The close of a room unpublishes its tracks, each of which would ask for a negotiation of
/// the publisher, and then closes the connection of the publisher. The session stops its
/// negotiations before the tracks are unpublished, and the engine leaves the unpublishing to
/// the close, so a leave starts none, and no offer is made in a closed connection (ADR 0733).
/// The test holds the media server alone and Ann is alone in the room: the counts of the
/// negotiations are of the whole test program, so they hold only hers.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs a LiveKit server: run through tools/with-test-livekit.sh"]
async fn a_leave_starts_no_negotiation_of_the_publisher() {
    limited(async {
        let _media = media_server_alone().await;
        let ann = Person::new(440.0);
        let (room, _ann_at) = Room::new("Acme", &ann, "Ann").await;
        let (failed, offered) = (negotiation_failures(), offers_in_a_closed_connection());
        for _ in 0..LEAVES {
            ann.enter(room.target).await;
            ann.calls.start_camera(None);
            // The microphone and the camera are published and send.
            let sending = tokio::time::timeout(PATIENCE, async {
                loop {
                    if let Ok(stats) = ann.calls.statistics().await {
                        let kinds: Vec<&str> = (stats.publisher_stats.iter())
                            .filter_map(|stat| match stat {
                                RtcStats::OutboundRtp(stat) if stat.sent.packets_sent > 0 => {
                                    Some(stat.stream.kind.as_str())
                                }
                                _ => None,
                            })
                            .collect();
                        if kinds.contains(&"audio") && kinds.contains(&"video") {
                            break;
                        }
                    }
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            })
            .await;
            assert!(
                sending.is_ok(),
                "the microphone and the camera of Ann did not send"
            );
            // A negotiation that the publications asked for ends before the leave, so that
            // the count holds only what the leave starts.
            until_no_negotiation_runs().await;
            let started = negotiations_started();
            ann.calls.leave().await;
            until_no_negotiation_runs().await;
            assert_eq!(
                negotiations_started() - started,
                0,
                "negotiations of the publisher started by the leave"
            );
        }
        assert_eq!(negotiation_failures() - failed, 0, "failed negotiations");
        assert_eq!(
            offers_in_a_closed_connection() - offered,
            0,
            "offers in a closed connection"
        );
    })
    .await;
}
