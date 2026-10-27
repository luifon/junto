//! A channel whose history the person may not read, in the window (ADR 0790): it shows
//! the messages of the current connection, says that earlier messages are not available
//! in place of the start of the channel, asks for no earlier page, and offers no pins and
//! no search in it.

use gpui::TestAppContext;
use junto_client_core::protocol::WorkspacePermission;

use crate::admin_pages::{default_workspace, text_channel};
use crate::channel_use::{everyone, rule};
use crate::harness::{World, member};
use crate::messages::document;
use crate::support::FaultProxy;

#[gpui::test]
fn without_read_message_history_a_channel_shows_only_the_messages_that_arrive(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let proxy = world.run(FaultProxy::start(&server));
    let admin = world.owner(&server);
    let workspace_id = default_workspace(&world, &admin.handle);
    let role = everyone(&world, &admin, workspace_id);
    let quiet = text_channel(&world, &admin.handle, workspace_id, "quiet");
    admin.send(&world, quiet, document("Earlier message"));
    rule(
        &world,
        &admin,
        quiet,
        role,
        &[],
        &[WorkspacePermission::ReadMessageHistory],
    );
    let (_directory, mut window) = member(cx, &world, &proxy.address(), &admin, "Bruno");
    window.until("the channel is listed", |window| {
        window.shows_channel("quiet")
    });
    assert!(window.shows("pins-button") && !window.shows("history-not-permitted"));
    window.click_channel("quiet");
    window.until("the channel is shown without its history", |window| {
        window
            .feed()
            .is_some_and(|feed| feed.loaded && !feed.more_before)
            && window.shows("history-not-permitted")
    });
    assert!(window.texts().is_empty());
    assert!(window.shows("composer"));

    // A message that arrives is shown, and the person can answer it.
    admin.send(&world, quiet, document("Live message"));
    window.until_message("Live message");
    assert_eq!(window.texts(), ["Live message"]);
    window.write("An answer");
    window.until_message("An answer");

    // The client asked for no earlier page: the one page that it asked for is the first
    // one, which the server refuses without the permission.
    let history = format!("/api/v1/channels/{quiet}/messages");
    let pages: Vec<(String, Option<u16>)> = proxy
        .all()
        .into_iter()
        .filter(|seen| {
            seen.method == "GET"
                && seen.path.starts_with(&history)
                && !seen.path.starts_with(&format!("{history}/read"))
        })
        .map(|seen| (seen.path, seen.answer.map(|(status, _)| status)))
        .collect();
    assert!(
        pages.len() <= 1 && pages.iter().all(|(_, status)| *status == Some(403)),
        "{pages:?}"
    );

    // The pinned messages and the search of the channel are read from the history.
    assert!(window.shows("pins-button-refused") && !window.shows("pins-button"));
    window.search_for("message");
    window.until_searched();
    window.click("search-channel");
    assert!(window.shows_named(format!("search-channel-refused:{quiet}")));
    assert!(!window.shows_named(format!("search-channel:{quiet}")));
}
