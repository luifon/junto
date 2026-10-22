//! The end of a media connection.

use crate::limit::limited;
use crate::scene::{Person, Room, media_server_shared, negotiation_failures};

/// Each leave closes a room in which the microphone was published.
const LEAVES: usize = 5;

/// The call library unpublishes the tracks of a room that it closes, which starts a
/// negotiation of the publisher, and closes the connection of the publisher at once. The
/// negotiation that the close ends is no failure (ADR 0733).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs a LiveKit server: run through tools/with-test-livekit.sh"]
async fn a_leave_ends_the_negotiation_of_the_publisher_without_a_failure() {
    limited(async {
        let _media = media_server_shared().await;
        let (ann, ben) = (Person::new(440.0), Person::new(880.0));
        let (room, ann_at) = Room::new("Acme", &ann, "Ann").await;
        let _ben_at = room.member(&ann_at, &ben, "Ben").await;
        ben.enter(room.target).await;
        for _ in 0..LEAVES {
            ann.enter(room.target).await;
            ben.until_heard_afresh(&ann).await;
            ann.calls.leave().await;
            ben.until_silent(&ann).await;
        }
        ben.calls.leave().await;
        assert_eq!(negotiation_failures(), 0);
    })
    .await;
}
