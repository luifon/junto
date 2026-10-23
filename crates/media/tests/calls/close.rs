//! The end of a media connection.

use std::time::Duration;

use junto_media::engine::ShareId;

use crate::limit::limited;
use crate::scene::{
    PATIENCE, Person, Room, account, media_server_shared, negotiation_failures,
    negotiations_ended_by_close, offers_in_a_closed_connection,
};

/// Each leave closes a room in which the microphone and the camera were published.
const LEAVES: usize = 5;

/// The close of a room unpublishes its tracks, each of which asks for a negotiation of the
/// publisher, and closes the connection of the publisher at once. With two tracks a
/// negotiation is still asked for when the connection is closed. The closed session makes no
/// offer for it and ends it without a failure (ADR 0733); each leave ends at least one so.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs a LiveKit server: run through tools/with-test-livekit.sh"]
async fn a_leave_ends_the_negotiation_of_the_publisher_without_a_failure() {
    limited(async {
        let _media = media_server_shared().await;
        let (ann, ben) = (Person::new(440.0), Person::new(880.0));
        let (room, ann_at) = Room::new("Acme", &ann, "Ann").await;
        let _ben_at = room.member(&ann_at, &ben, "Ben").await;
        let ann_id = ShareId::Of(account(&ann_at));
        ben.enter(room.target).await;
        let mut cameras = ben.calls.camera_pictures();
        let (ended, failed, offered) = (
            negotiations_ended_by_close(),
            negotiation_failures(),
            offers_in_a_closed_connection(),
        );
        let outcomes = move || {
            negotiations_ended_by_close() - ended + negotiation_failures() - failed
                + offers_in_a_closed_connection()
                - offered
        };
        for _ in 0..LEAVES {
            ann.enter(room.target).await;
            ann.calls.start_camera(None);
            ben.until_heard_afresh(&ann).await;
            let shown = tokio::time::timeout(
                PATIENCE,
                cameras.wait_for(|pictures| pictures.contains_key(&ann_id)),
            )
            .await
            .is_ok_and(|shown| shown.is_ok());
            assert!(shown, "no camera picture of Ann arrived");
            ann.calls.leave().await;
            ben.until_silent(&ann).await;
            let gone = tokio::time::timeout(
                PATIENCE,
                cameras.wait_for(|pictures| !pictures.contains_key(&ann_id)),
            )
            .await
            .is_ok_and(|gone| gone.is_ok());
            assert!(gone, "the camera picture of Ann stayed");
        }
        // A negotiation that waits for the answer of the media server ends after 10 seconds.
        let ends = tokio::time::timeout(PATIENCE, async {
            while outcomes() < LEAVES {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
        .await;
        ben.calls.leave().await;
        assert_eq!(negotiation_failures() - failed, 0, "failed negotiations");
        assert_eq!(
            offers_in_a_closed_connection() - offered,
            0,
            "offers in a closed connection"
        );
        assert!(
            ends.is_ok(),
            "{} negotiations ended by the close of {LEAVES} rooms",
            negotiations_ended_by_close() - ended
        );
    })
    .await;
}
