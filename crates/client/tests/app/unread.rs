//! The read position of a place in the window against the real server (ADR 0740): the end
//! of a list that an active window shows is read without an action of the person, and two
//! windows of one account show one read position.

use gpui::TestAppContext;
use junto_client::state::feed::FeedRow;
use junto_client_core::flow::access::AccessState;
use junto_client_core::model::PlaceId;
use junto_client_core::protocol::ids::ChannelId;

use crate::harness::{Window, World, member, named};
use crate::messages::document;
use crate::support::TestServer;

impl Window {
    /// The unread messages and mentions of the row of a channel.
    fn unread_of(&mut self, channel_id: ChannelId) -> (u64, u64) {
        self.channels()
            .into_iter()
            .find(|row| row.id == channel_id)
            .map(|row| (row.unread.messages, row.unread.mentions))
            .unwrap()
    }

    fn activate(&mut self) {
        self.cx.update(|window, _| window.activate_window());
        self.settle();
    }

    fn is_active(&mut self) -> bool {
        self.cx.update(|window, _| window.is_window_active())
    }
}

/// A second window of the account of `first`, on a device that the first window links.
fn linked_window(
    cx: &mut TestAppContext,
    world: &World,
    server: &TestServer,
    first: &mut Window,
) -> (tempfile::TempDir, Window) {
    let (directory, mut second) = named(cx, "Luca");
    second.find_server(&server.address());
    second.click("request-link");
    let (code, public_key) = second.until_value("the second device waits for its link", |second| {
        let handle = second
            .state
            .read_with(&second.cx, |state, _| state.client().servers())
            .into_iter()
            .next()?;
        match handle.access().state {
            AccessState::WaitingForLink {
                code, public_key, ..
            } => Some((code, public_key)),
            _ => None,
        }
    });
    world
        .run(first.handle().api().approve_device_link(code, public_key))
        .unwrap();
    second.until_ready();
    assert_eq!(
        second.view().person.account_id,
        first.view().person.account_id
    );
    (directory, second)
}

#[gpui::test]
fn the_end_of_a_list_in_the_active_window_is_read_without_an_action(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    window.until("the messages are read", |window| {
        window.feed().is_some_and(|feed| feed.loaded)
    });
    let random = admin.text_channel(&world, &mut window, "random");
    let own = window.handle();
    let stored = |world: &World| {
        world
            .run(own.api().message_state(PlaceId::Channel(random)))
            .unwrap()
            .unread
    };
    admin.send(&world, random, document("first news"));
    admin.send(&world, random, document("second news"));
    window.until("the unread messages of the other channel", |window| {
        window.unread_of(random) == (2, 0)
    });

    // The window is not the active window of the system: it shows the messages, and they
    // stay unread.
    assert!(!window.is_active());
    window.click_channel("random");
    window.until("the messages of the other channel are drawn", |window| {
        window
            .feed()
            .is_some_and(|feed| feed.loaded && feed.place == PlaceId::Channel(random))
            && window.texts() == ["first news", "second news"]
    });
    assert!(window.in_shell(|shell, _, _| shell.feed_is_at_end(junto_client::shell::Pane::Main)));
    assert_eq!(window.unread_of(random), (2, 0));
    assert_eq!(stored(&world), 2);

    // The person looks at the window: the end of the list is shown, and the messages are
    // read on the server.
    window.activate();
    window.until("the messages are read", |window| {
        window.unread_of(random) == (0, 0)
    });
    window.until("the server holds the read position", |_| {
        stored(&world) == 0
    });

    // A message that arrives at the end of the shown list is read at once.
    admin.send(&world, random, document("third news"));
    window.until_message("third news");
    window.until("the new message is read", |window| {
        window.unread_of(random) == (0, 0) && stored(&world) == 0
    });
}

#[gpui::test]
fn two_windows_of_one_account_show_one_read_position(
    cx: &mut TestAppContext,
    second_cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut first) = member(cx, &world, &server.address(), &admin, "Luca");
    let (_second_directory, mut second) = linked_window(second_cx, &world, &server, &mut first);
    let random = admin.text_channel(&world, &mut first, "random");
    second.until("the second window lists the channel", |second| {
        second.channels().iter().any(|row| row.id == random)
    });
    admin.send(&world, random, document("first news"));
    admin.send(&world, random, document("second news"));
    for window in [&mut first, &mut second] {
        window.until("the unread messages", |window| {
            window.unread_of(random) == (2, 0)
        });
    }

    // The first window shows the channel to the person: its messages are read.
    first.click_channel("random");
    first.activate();
    first.until("the first window read the messages", |first| {
        first.unread_of(random) == (0, 0)
    });

    // The second window shows no unread message there, and its list of the channel has
    // no line of the first unread message. It is not the active window, so it moved
    // nothing itself.
    assert!(!second.is_active());
    second.until("the second window has the read position", |second| {
        second.unread_of(random) == (0, 0)
    });
    second.click_channel("random");
    second.until("the messages in the second window", |second| {
        second.texts() == ["first news", "second news"]
    });
    let feed = second.feed().unwrap();
    assert!(
        feed.rows.iter().all(|row| !matches!(row, FeedRow::Unread)),
        "the second window marks a message as the first unread one"
    );
    assert_eq!(
        world
            .run(
                second
                    .handle()
                    .api()
                    .message_state(PlaceId::Channel(random))
            )
            .unwrap()
            .unread,
        0
    );
}
