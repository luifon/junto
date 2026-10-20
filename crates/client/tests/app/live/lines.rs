//! The lines of a direct call in the conversation of its two accounts (ADR 0518), in a
//! window that starts after the call: the conversation, its unread sign, and the lines.

use gpui::TestAppContext;
use junto_client::state::feed::FeedRow;

use crate::harness::{World, member, open};
use crate::support::logged_in;

#[gpui::test]
#[ignore = "needs a LiveKit server: run through tools/with-test-livekit.sh"]
fn a_window_that_starts_after_a_call_that_was_not_answered_shows_its_lines_and_an_unread_sign(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server_with_media("Acme Studio");
    let ada = world.owner(&server);
    let ada_id = logged_in(&ada.handle);
    let (directory, window) = member(cx, &world, &server.address(), &ada, "Bruno");
    let bruno = logged_in(
        &window
            .state
            .read_with(&window.cx, |state, _| state.client().servers())
            .remove(0),
    );
    // The application of Bruno does not run while Ada calls and hangs up.
    window.close();
    world.run(async {
        let api = ada.handle.api();
        api.join_direct_call(bruno).await.unwrap();
        api.leave_call().await.unwrap();
    });

    // Bruno starts the application afterwards: the conversation with Ada is in the
    // channel column with one unread message. The row has no name until the window has
    // read the profile of the other account, which it does for every row of this list.
    let mut window = open(cx, directory.path());
    window.until_ready();
    let mut conversation = None;
    window.until(
        "the conversation of the call with its unread sign and the name of the caller",
        |window| {
            conversation = window.view().lists().and_then(|lists| {
                lists
                    .directs
                    .iter()
                    .find(|row| {
                        row.account_id == ada_id
                            && row.unread.messages == 1
                            && row.name.as_deref() == Some("Ada")
                    })
                    .cloned()
            });
            conversation.is_some()
        },
    );
    let conversation = conversation.unwrap();
    assert!(conversation.unread.any());

    // The conversation holds the two lines of the call, as sentences.
    window.click_named(format!("direct-row:{}", conversation.id));
    let lines = |window: &mut crate::harness::Window| -> Vec<String> {
        window
            .feed()
            .map(|feed| {
                feed.rows
                    .iter()
                    .filter_map(|row| match row {
                        FeedRow::System(row) => {
                            Some(row.parts.iter().map(|part| part.text.as_str()).collect())
                        }
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default()
    };
    window.until("the lines of the call", |window| {
        lines(window) == ["Ada started a call", "Bruno did not answer the call of Ada"]
    });
}
